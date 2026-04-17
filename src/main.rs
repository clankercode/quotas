use clap::Parser;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use quotas::auth::cursor::CursorAuthResolver;
use quotas::auth::env::EnvResolver;
use quotas::auth::file::{CookieFileResolver, FileResolver};
use quotas::auth::oauth::OAuthFileResolver;
use quotas::auth::opencode::{KimiCliResolver, OpencodeAuthResolver, OpencodeSlot};
use quotas::auth::refresh;
use quotas::auth::{AuthResolver, MultiResolver};
use quotas::cache;
use quotas::config::Config;
use quotas::output::json::JsonOutput;
use quotas::output::statusline::{self, StatusLineConfig};
use quotas::providers::{Provider, ProviderKind, ProviderResult};
use quotas::tui::Dashboard;
use quotas::tui::Direction;
use quotas::tui::HitResult;
use quotas::tui::ProviderEntry;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "quotas")]
#[command(about = "Check AI provider usage quotas from your configured credentials", long_about = None)]
struct Args {
    #[arg(long)]
    json: bool,

    #[arg(long)]
    pretty: bool,

    /// Print the raw JSON response from each provider (pretty) and exit.
    /// Useful for inspecting fields we may not be parsing yet.
    #[arg(long)]
    raw: bool,

    /// Compact single-line output for shell prompts / statuslines.
    /// Reads from cache (instant, no network). If cache is stale (>30 min),
    /// forks a background refresh unless --no-bg-refresh is set.
    #[arg(long)]
    statusline: bool,

    /// Disable Nerd Font icons in statusline output.
    #[arg(long)]
    no_icons: bool,

    /// Don't fork a background cache refresh even if cache is stale.
    #[arg(long)]
    no_bg_refresh: bool,

    /// Read exclusively from cache — disables all automatic refreshes.
    /// Manual refresh (R key in TUI, refresh button) remains available.
    #[arg(long)]
    cached: bool,

    /// Fetch fresh data only when cache is stale or missing (cache-first).
    /// Unlike --cached, this will make API calls if data is stale.
    /// Uses per-provider staleness thresholds from config (default 5 min).
    #[arg(long)]
    ensure_current: bool,

    /// Custom format template for statusline.
    /// Placeholders: %provider %remaining %limit %used %window %reset
    #[arg(long)]
    format: Option<String>,

    /// Hidden: fetch all providers, merge into cache, exit silently.
    #[arg(long, hide = true)]
    update_cache: bool,

    #[arg(long, value_delimiter = ',')]
    provider: Vec<String>,

    /// Render the TUI grid to a text snapshot and exit (no tmux needed).
    /// Useful for automated layout testing at specific resolutions.
    #[arg(long, hide = true)]
    snap: bool,

    /// Width for --snap mode (default: 160).
    #[arg(long, default_value = "160", hide = true)]
    snap_width: u16,

    /// Height for --snap mode (default: 50).
    #[arg(long, default_value = "50", hide = true)]
    snap_height: u16,

    /// Output file for --snap mode. Writes to stdout if omitted.
    #[arg(long, hide = true)]
    snap_output: Option<String>,
}

/// Parse a credentials file that may be either a raw token on the first
/// non-empty, non-comment line or a `key=value` file with `api_key=...` /
/// `token=...` entries.
fn parse_key_file(content: &str) -> Option<String> {
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("api_key=") {
            return Some(rest.trim().trim_matches('"').to_string());
        }
        if let Some(rest) = line.strip_prefix("token=") {
            return Some(rest.trim().trim_matches('"').to_string());
        }
        if line.contains('=') {
            continue;
        }
        return Some(line.to_string());
    }
    None
}

fn build_auth_resolver(kind: &ProviderKind) -> Box<dyn AuthResolver> {
    match kind {
        ProviderKind::Minimax => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(EnvResolver::new(vec![("MINIMAX_API_KEY", "minimax")])),
                Box::new(FileResolver::new(
                    vec![dirs::home_dir().unwrap_or_default().join(".minimax")],
                    parse_key_file,
                    "minimax",
                )),
                Box::new(OpencodeAuthResolver::new(OpencodeSlot::Minimax)),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::Zai => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(EnvResolver::new(vec![
                    ("ZHIPU_API_KEY", "zhipu"),
                    ("ZAI_API_KEY", "zai"),
                ])),
                Box::new(FileResolver::new(
                    vec![dirs::home_dir().unwrap_or_default().join(".api-zai")],
                    parse_key_file,
                    "zai",
                )),
                Box::new(OpencodeAuthResolver::new(OpencodeSlot::Zai)),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::Kimi => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(EnvResolver::new(vec![
                    ("MOONSHOT_API_KEY", "moonshot"),
                    ("KIMI_API_KEY", "kimi"),
                ])),
                Box::new(FileResolver::new(
                    vec![
                        dirs::home_dir().unwrap_or_default().join(".moonshot"),
                        dirs::home_dir().unwrap_or_default().join(".kimi"),
                    ],
                    parse_key_file,
                    "kimi",
                )),
                Box::new(KimiCliResolver::new()),
                Box::new(OpencodeAuthResolver::new(OpencodeSlot::Kimi)),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::Claude => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(OAuthFileResolver::claude()),
                Box::new(OpencodeAuthResolver::new(OpencodeSlot::Anthropic)),
                Box::new(EnvResolver::new(vec![("ANTHROPIC_API_KEY", "anthropic")])),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::Codex => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(OAuthFileResolver::codex()),
                Box::new(OpencodeAuthResolver::new(OpencodeSlot::Openai)),
                Box::new(EnvResolver::new(vec![("OPENAI_API_KEY", "openai")])),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::Cursor => Box::new(CursorAuthResolver::new()),
        ProviderKind::DeepSeek => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(EnvResolver::new(vec![("DEEPSEEK_API_KEY", "deepseek")])),
                Box::new(FileResolver::new(
                    vec![dirs::home_dir().unwrap_or_default().join(".deepseek")],
                    parse_key_file,
                    "deepseek",
                )),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::Gemini => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(OAuthFileResolver::gemini()),
                Box::new(EnvResolver::new(vec![("GEMINI_API_KEY", "gemini")])),
                Box::new(FileResolver::new(
                    vec![dirs::home_dir().unwrap_or_default().join(".gemini-api-key")],
                    parse_key_file,
                    "gemini",
                )),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::SiliconFlow => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(EnvResolver::new(vec![
                    ("SILICONFLOW_API_KEY", "siliconflow"),
                    ("SILICON_FLOW_API_KEY", "siliconflow"),
                ])),
                Box::new(FileResolver::new(
                    vec![dirs::home_dir().unwrap_or_default().join(".siliconflow")],
                    parse_key_file,
                    "siliconflow",
                )),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::OpenRouter => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(EnvResolver::new(vec![("OPENROUTER_API_KEY", "openrouter")])),
                Box::new(FileResolver::new(
                    vec![dirs::home_dir().unwrap_or_default().join(".openrouter")],
                    parse_key_file,
                    "openrouter",
                )),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::Mimo => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(CookieFileResolver::new(
                    vec![
                        dirs::home_dir()
                            .unwrap_or_default()
                            .join(".config/mimo/cookie"),
                        dirs::home_dir().unwrap_or_default().join(".mimo-cookie"),
                    ],
                    "mimo-cookie",
                )),
                Box::new(EnvResolver::new(vec![
                    ("XIAOMI_MIMO_API_KEY", "xiaomi_mimo"),
                    ("XIAOMI_API_KEY", "xiaomi"),
                    ("MIMO_API_KEY", "mimo"),
                ])),
                Box::new(FileResolver::new(
                    vec![
                        dirs::home_dir().unwrap_or_default().join(".mimo-key"),
                        dirs::home_dir().unwrap_or_default().join(".xiaomimimo"),
                        dirs::home_dir().unwrap_or_default().join(".mimo"),
                    ],
                    parse_key_file,
                    "mimo",
                )),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
    }
}

fn filter_kinds(names: &[String]) -> Vec<ProviderKind> {
    if names.is_empty() {
        return ProviderKind::all().to_vec();
    }
    names
        .iter()
        .filter_map(|n| match n.to_lowercase().as_str() {
            "claude" | "anthropic" => Some(ProviderKind::Claude),
            "codex" | "chatgpt" | "openai" => Some(ProviderKind::Codex),
            "cursor" => Some(ProviderKind::Cursor),
            "deepseek" | "deep-seek" | "deep_seek" => Some(ProviderKind::DeepSeek),
            "gemini" => Some(ProviderKind::Gemini),
            "kimi" | "moonshot" => Some(ProviderKind::Kimi),
            "minimax" => Some(ProviderKind::Minimax),
            "zai" | "zhipu" | "z.ai" | "glm" => Some(ProviderKind::Zai),
            "siliconflow" | "silicon-flow" | "silicon_flow" => Some(ProviderKind::SiliconFlow),
            "openrouter" | "open-router" | "open_router" => Some(ProviderKind::OpenRouter),
            "mimo" | "xiaomimimo" | "xiaomi-mimo" | "xiaomi_mimo" => Some(ProviderKind::Mimo),
            _ => None,
        })
        .collect()
}

async fn maybe_refresh_creds(kind: ProviderKind, config: &Config) {
    if !config.auto_refresh.enabled {
        return;
    }
    match kind {
        ProviderKind::Kimi => {
            let path = refresh::kimi_creds_path();
            let _ = refresh::refresh_kimi_if_expired(&path).await;
            if let Some(oc) = refresh::opencode_creds_path() {
                // opencode "kimi-for-coding" is type:api today; nothing to refresh.
                let _ = oc;
            }
        }
        ProviderKind::Claude => {
            let path = refresh::claude_creds_path();
            let _ = refresh::refresh_claude_if_expired(&path).await;
            if let Some(oc) = refresh::opencode_creds_path() {
                let _ = refresh::refresh_opencode_anthropic_if_expired(&oc).await;
            }
        }
        ProviderKind::Codex => {
            let path = refresh::codex_creds_path();
            let _ = refresh::refresh_codex_if_expired(&path).await;
            if let Some(oc) = refresh::opencode_creds_path() {
                let _ = refresh::refresh_opencode_openai_if_expired(&oc).await;
            }
        }
        _ => {}
    }
}

async fn fetch_one(kind: ProviderKind, config: &Config) -> ProviderResult {
    maybe_refresh_creds(kind, config).await;
    let auth = build_auth_resolver(&kind);
    let provider: Box<dyn Provider> = match kind {
        ProviderKind::Claude => Box::new(quotas::providers::claude::ClaudeProvider::new(auth)),
        ProviderKind::Codex => Box::new(quotas::providers::codex::CodexProvider::new(auth)),
        ProviderKind::Cursor => Box::new(quotas::providers::cursor::CursorProvider::new(auth)),
        ProviderKind::Minimax => Box::new(
            quotas::providers::minimax::MinimaxProvider::with_multi_resolver(MultiResolver::new(
                vec![auth],
            )),
        ),
        ProviderKind::Zai => Box::new(quotas::providers::zai::ZaiProvider::new(auth)),
        ProviderKind::Kimi => Box::new(quotas::providers::kimi::KimiProvider::new(auth)),
        ProviderKind::DeepSeek => {
            Box::new(quotas::providers::deepseek::DeepSeekProvider::new(auth))
        }
        ProviderKind::Gemini => Box::new(quotas::providers::gemini::GeminiProvider::new(auth)),
        ProviderKind::SiliconFlow => Box::new(
            quotas::providers::siliconflow::SiliconFlowProvider::new(auth),
        ),
        ProviderKind::OpenRouter => {
            Box::new(quotas::providers::openrouter::OpenRouterProvider::new(auth))
        }
        ProviderKind::Mimo => Box::new(quotas::providers::mimo::MimoProvider::new(auth)),
    };
    // Pre-resolve to capture the auth source string for the detail view.
    // This is a lightweight re-resolve (env var / file read) after any token
    // refresh that already happened in maybe_refresh_creds.
    let auth_source = provider
        .auth_resolver()
        .resolve()
        .await
        .ok()
        .map(|a| a.source);

    let mut result = match provider.fetch().await {
        Ok(r) => r,
        Err(quotas::Error::Auth(msg)) => auth_required_result(kind, msg),
        Err(e) => network_error_result(kind, e.to_string()),
    };
    result.auth_source = auth_source;
    result
}

fn fetch_provider_sync(kind: ProviderKind, config: &Config) -> ProviderResult {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return network_error_result(kind, e.to_string()),
    };
    rt.block_on(fetch_one(kind, config))
}

fn auth_required_result(kind: ProviderKind, _reason: String) -> ProviderResult {
    ProviderResult {
        kind,
        status: quotas::providers::ProviderStatus::AuthRequired,
        fetched_at: chrono::Utc::now(),
        raw_response: None,
        auth_source: None,
        cached_at: None,
    }
}

fn network_error_result(kind: ProviderKind, message: String) -> ProviderResult {
    ProviderResult {
        kind,
        status: quotas::providers::ProviderStatus::NetworkError { message },
        fetched_at: chrono::Utc::now(),
        raw_response: None,
        auth_source: None,
        cached_at: None,
    }
}

fn auto_refresh_interval(kind: ProviderKind) -> Duration {
    match kind {
        ProviderKind::Claude => Duration::from_secs(600), // 10 min — avoid rate-limiting
        _ => Duration::from_secs(300),                    // 5 min for everything else
    }
}

fn should_startup_fetch(cached: bool, refresh_on_start: bool, has_creds: bool) -> bool {
    !cached && refresh_on_start && has_creds
}

fn should_periodic_refresh(cached: bool, auto_refresh_enabled: bool) -> bool {
    !cached && auto_refresh_enabled
}

fn should_backdate_refresh_timer(refresh_on_start: bool, startup_fetching: bool) -> bool {
    refresh_on_start && !startup_fetching
}

fn fetch_all(kinds: Vec<ProviderKind>, config: &Config) -> Vec<ProviderResult> {
    let results: Vec<ProviderResult> = kinds
        .into_iter()
        .map(|k| fetch_provider_sync(k, config))
        .collect();
    cache::write_cache(&results);
    results
}

/// Fetch with cache-first semantics: return cached data if fresh, otherwise
/// fetch fresh data and update the cache. Uses per-provider staleness thresholds.
fn fetch_with_staleness_check(kinds: Vec<ProviderKind>, config: &Config) -> Vec<ProviderResult> {
    let cache = cache::read_cache();
    let now = chrono::Utc::now();
    let mut results = Vec::with_capacity(kinds.len());
    let mut fresh_results = Vec::new();

    for kind in kinds {
        let key = kind.slug().to_string();
        let staleness_threshold = config.staleness.staleness_threshold(&key);

        if let Some(entry) = cache.entries.get(&key) {
            let age_secs = (now - entry.cached_at).num_seconds() as u64;
            if age_secs < staleness_threshold {
                // Cache hit — fresh enough, use cached result with its cached_at.
                let mut result = entry.result.clone();
                result.cached_at = Some(entry.cached_at);
                results.push(result);
                continue;
            }
        }
        // Cache miss or stale — fetch fresh.
        let result = fetch_provider_sync(kind, config);
        fresh_results.push(result);
    }

    // Write all fresh results to cache.
    if !fresh_results.is_empty() {
        cache::write_cache(&fresh_results);
    }

    results.extend(fresh_results);
    results
}

/// Spawn fetches only for the specified provider indices.
fn spawn_fetches_for(
    rt: &tokio::runtime::Runtime,
    kinds: &[ProviderKind],
    indices: &[usize],
    config: Config,
    tx: tokio::sync::mpsc::UnboundedSender<(usize, ProviderResult)>,
) {
    for &idx in indices {
        let kind = kinds[idx];
        let tx = tx.clone();
        let config = config.clone();
        rt.spawn(async move {
            let result = fetch_one(kind, &config).await;
            let _ = tx.send((idx, result));
        });
    }
}

fn render_dashboard_text(dashboard: &Dashboard, width: u16, height: u16) -> String {
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| dashboard.render(f)).unwrap();

    let buffer = terminal.backend().buffer().clone();
    let mut lines = Vec::new();
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            line.push_str(buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n") + "\n"
}

fn snap_page_output_path(path: &str, page_number: usize) -> PathBuf {
    let original = PathBuf::from(path);
    let suffix = format!("page{page_number}");
    match (original.file_stem(), original.extension()) {
        (Some(stem), Some(ext)) => original.with_file_name(format!(
            "{}-{}.{}",
            stem.to_string_lossy(),
            suffix,
            ext.to_string_lossy()
        )),
        (Some(stem), None) => {
            original.with_file_name(format!("{}-{}", stem.to_string_lossy(), suffix))
        }
        _ => PathBuf::from(format!("{path}-{suffix}")),
    }
}

fn run_snap(
    kinds: Vec<ProviderKind>,
    config: Config,
    width: u16,
    height: u16,
    output: Option<&str>,
) {
    let disk_cache = cache::read_cache();
    let now = chrono::Utc::now();

    let entries: Vec<ProviderEntry> = kinds
        .iter()
        .map(|kind| {
            let key = kind.slug().to_string();
            if let Some(entry) = disk_cache.entries.get(&key) {
                let mut result = entry.result.clone();
                result.cached_at = Some(entry.cached_at);
                ProviderEntry::Done(result)
            } else {
                ProviderEntry::Done(ProviderResult {
                    kind: *kind,
                    status: quotas::providers::ProviderStatus::AuthRequired,
                    fetched_at: now,
                    raw_response: None,
                    auth_source: None,
                    cached_at: None,
                })
            }
        })
        .collect();

    let mut dashboard = Dashboard::new_with_entries(kinds, entries);

    // Apply config settings so snap respects the user's layout preferences.
    dashboard.show_all_windows = config.ui.show_all_windows;
    dashboard.vertical_spanning = config.ui.vertical_spanning;
    dashboard.auto_refresh_enabled = config.tui.auto_refresh;

    let out = render_dashboard_text(&dashboard, width, height);
    let pages = dashboard.page_count();
    if let Some(path) = output {
        if let Err(e) = std::fs::write(path, &out) {
            eprintln!("Error writing to {path}: {e}");
            std::process::exit(1);
        }
        eprintln!("Wrote {path}");
        for page in 1..pages {
            dashboard.select_page(page);
            let page_out = render_dashboard_text(&dashboard, width, height);
            let page_path = snap_page_output_path(path, page + 1);
            if let Err(e) = std::fs::write(&page_path, &page_out) {
                eprintln!("Error writing {}: {e}", page_path.display());
                std::process::exit(1);
            }
            eprintln!("Wrote {}", page_path.display());
        }
    } else {
        print!("{out}");
        for page in 1..pages {
            dashboard.select_page(page);
            print!("{}", render_dashboard_text(&dashboard, width, height));
        }
    }
}
fn run_tui(kinds: Vec<ProviderKind>, config: Config, cached: bool) -> io::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .map_err(io::Error::other)?;

    // Read cache and build auth resolvers for credential pre-checking.
    let disk_cache = cache::read_cache();
    let now = chrono::Utc::now();
    let auth_resolvers: Vec<Box<dyn AuthResolver>> =
        kinds.iter().map(|k| build_auth_resolver(k)).collect();

    // Build initial entries: prefer cached data when fresh, skip fetches for
    // providers without credentials, only spawn fetches where needed.
    let mut initial_entries = Vec::with_capacity(kinds.len());
    let mut fetch_indices: Vec<usize> = Vec::new();

    for (idx, kind) in kinds.iter().enumerate() {
        let key = kind.slug().to_string();
        let has_creds = auth_resolvers[idx].have_credentials();
        let fetch_on_start = should_startup_fetch(cached, config.tui.refresh_on_start, has_creds);

        if cached {
            // --cached: only show cache, never fetch.
            if let Some(entry) = disk_cache.entries.get(&key) {
                let mut result = entry.result.clone();
                result.cached_at = Some(entry.cached_at);
                initial_entries.push(ProviderEntry::Done(result));
            } else if has_creds {
                initial_entries.push(ProviderEntry::Done(ProviderResult {
                    kind: *kind,
                    status: quotas::providers::ProviderStatus::AuthRequired,
                    fetched_at: now,
                    raw_response: None,
                    auth_source: None,
                    cached_at: None,
                }));
            } else {
                initial_entries.push(ProviderEntry::Done(auth_required_result(
                    *kind,
                    "no credentials".into(),
                )));
            }
            continue;
        }

        if let Some(entry) = disk_cache.entries.get(&key) {
            // Show cached data immediately. By default the TUI still fetches
            // fresh data after load, even when cache is within staleness.
            let mut result = entry.result.clone();
            result.cached_at = Some(entry.cached_at);
            if fetch_on_start {
                initial_entries.push(ProviderEntry::Refreshing(result));
                fetch_indices.push(idx);
            } else {
                initial_entries.push(ProviderEntry::Done(result));
            }
        } else if has_creds {
            if fetch_on_start {
                // No cache but credentials exist — show loading spinner.
                initial_entries.push(ProviderEntry::Loading);
                fetch_indices.push(idx);
            } else {
                initial_entries.push(ProviderEntry::Done(ProviderResult {
                    kind: *kind,
                    status: quotas::providers::ProviderStatus::Unavailable {
                        info: quotas::providers::UnavailableInfo {
                            reason: "No cached data; startup refresh disabled".into(),
                            console_url: None,
                        },
                    },
                    fetched_at: now,
                    raw_response: None,
                    auth_source: None,
                    cached_at: None,
                }));
            }
        } else {
            // No cache, no credentials — show auth required.
            initial_entries.push(ProviderEntry::Done(auth_required_result(
                *kind,
                "no credentials".into(),
            )));
        }
    }

    let mut dashboard = Dashboard::new_with_entries(kinds.clone(), initial_entries);
    dashboard.show_all_windows = config.ui.show_all_windows;
    dashboard.vertical_spanning = config.ui.vertical_spanning;
    dashboard.auto_refresh_enabled = config.tui.auto_refresh;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (mut cur_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(usize, ProviderResult)>();
    if !fetch_indices.is_empty() {
        spawn_fetches_for(&rt, &kinds, &fetch_indices, config.clone(), cur_tx.clone());
    }

    // Per-provider last-refresh timestamps.  For entries we're about to fetch
    // the timer starts now; for cached entries we back-date the timer so the
    // auto-refresh interval fires at roughly the right time.
    let mut last_refresh: Vec<Instant> = kinds
        .iter()
        .enumerate()
        .map(|(idx, _)| {
            if fetch_indices.contains(&idx) {
                Instant::now()
            } else if should_backdate_refresh_timer(
                config.tui.refresh_on_start,
                fetch_indices.contains(&idx),
            ) {
                if let Some(entry) = disk_cache.entries.get(kinds[idx].slug()) {
                    let age_secs = (now - entry.cached_at).num_seconds().max(0) as u64;
                    let elapsed = Duration::from_secs(age_secs);
                    // Back-date so the next auto-refresh fires after the normal interval.
                    Instant::now()
                        .checked_sub(elapsed)
                        .unwrap_or(Instant::now())
                } else {
                    Instant::now()
                }
            } else {
                Instant::now()
            }
        })
        .collect();

    let tick = Duration::from_millis(80);
    let result: io::Result<()> = (|| loop {
        terminal.draw(|f| dashboard.render(f))?;

        while let Ok((idx, result)) = rx.try_recv() {
            cache::write_cache(std::slice::from_ref(&result));
            dashboard.update(idx, result);
            last_refresh[idx] = Instant::now();
        }

        // Per-provider auto-refresh: each provider refreshes on its own schedule.
        // Claude uses a longer interval to avoid rate-limiting.
        // Skip providers without credentials.
        if should_periodic_refresh(cached, dashboard.auto_refresh_enabled) {
            for (idx, kind) in kinds.iter().cloned().enumerate() {
                if !auth_resolvers[idx].have_credentials() {
                    continue;
                }
                if dashboard.is_entry_done(idx)
                    && last_refresh[idx].elapsed() >= auto_refresh_interval(kind)
                {
                    dashboard.reset_one(idx);
                    let tx2 = cur_tx.clone();
                    let config2 = config.clone();
                    rt.spawn(async move {
                        let result = fetch_one(kind, &config2).await;
                        let _ = tx2.send((idx, result));
                    });
                    last_refresh[idx] = Instant::now();
                }
            }
        }

        if crossterm::event::poll(tick)? {
            match crossterm::event::read()? {
                Event::Mouse(me) => match me.kind {
                    MouseEventKind::ScrollDown => {
                        if dashboard.show_detail {
                            dashboard.scroll_detail(3);
                        } else {
                            dashboard.navigate(Direction::Down);
                        }
                    }
                    MouseEventKind::ScrollUp => {
                        if dashboard.show_detail {
                            dashboard.scroll_detail(-3);
                        } else {
                            dashboard.navigate(Direction::Up);
                        }
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        match dashboard.hit_test(me.column, me.row) {
                            Some(HitResult::Refresh) => {
                                let (new_tx, new_rx) = tokio::sync::mpsc::unbounded_channel();
                                rx = new_rx;
                                cur_tx = new_tx;
                                // Only fetch providers that have credentials.
                                let fetchable: Vec<usize> = (0..kinds.len())
                                    .filter(|&i| auth_resolvers[i].have_credentials())
                                    .collect();
                                for &idx in &fetchable {
                                    dashboard.reset_one(idx);
                                }
                                spawn_fetches_for(
                                    &rt,
                                    &kinds,
                                    &fetchable,
                                    config.clone(),
                                    cur_tx.clone(),
                                );
                                for &idx in &fetchable {
                                    last_refresh[idx] = Instant::now();
                                }
                            }
                            Some(HitResult::AutoRefreshToggle) => {
                                dashboard.auto_refresh_enabled = !dashboard.auto_refresh_enabled;
                            }
                            Some(HitResult::Quit) => return Ok(()),
                            Some(HitResult::Card(vpos)) => {
                                if dashboard.selected_index == vpos && !dashboard.show_detail {
                                    // Second click on already-selected card → open detail.
                                    dashboard.show_detail = true;
                                    dashboard.detail_scroll = 0;
                                } else {
                                    dashboard.selected_index = vpos;
                                    dashboard.show_detail = false;
                                }
                            }
                            None => {}
                        }
                    }
                    MouseEventKind::Moved => {
                        dashboard.set_mouse_pos(me.column, me.row);
                    }
                    _ => {}
                },
                Event::Key(KeyEvent { code, .. }) => match code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(()),
                    KeyCode::Esc | KeyCode::Backspace => {
                        // Esc/Backspace acts as "go back" from detail view.
                        // From the grid it's a no-op (use Q to quit).
                        if dashboard.show_detail {
                            dashboard.show_detail = false;
                        }
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        let (new_tx, new_rx) = tokio::sync::mpsc::unbounded_channel();
                        rx = new_rx;
                        cur_tx = new_tx;
                        // Only fetch providers that have credentials.
                        let fetchable: Vec<usize> = (0..kinds.len())
                            .filter(|&i| auth_resolvers[i].have_credentials())
                            .collect();
                        for &idx in &fetchable {
                            dashboard.reset_one(idx);
                        }
                        spawn_fetches_for(&rt, &kinds, &fetchable, config.clone(), cur_tx.clone());
                        for &idx in &fetchable {
                            last_refresh[idx] = Instant::now();
                        }
                    }
                    KeyCode::Char('c') | KeyCode::Char('C') => {
                        if let Some(selected) = dashboard.selected_provider() {
                            if let Ok(json) = serde_json::to_string_pretty(selected) {
                                if let Ok(mut ctx) = arboard::Clipboard::new() {
                                    let _ = ctx.set_text(&json);
                                }
                            }
                        }
                    }
                    KeyCode::Char('a') | KeyCode::Char('A') => {
                        dashboard.auto_refresh_enabled = !dashboard.auto_refresh_enabled;
                    }
                    KeyCode::Enter => {
                        if dashboard.show_detail {
                            dashboard.show_detail = false;
                            dashboard.detail_scroll = 0;
                        } else {
                            dashboard.show_detail = true;
                            dashboard.detail_scroll = 0;
                        }
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        if dashboard.show_detail {
                            dashboard.detail_prev();
                        } else {
                            dashboard.navigate(Direction::Left);
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        if dashboard.show_detail {
                            dashboard.detail_next();
                        } else {
                            dashboard.navigate(Direction::Right);
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if dashboard.show_detail {
                            dashboard.scroll_detail(-3);
                        } else {
                            dashboard.navigate(Direction::Up);
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if dashboard.show_detail {
                            dashboard.scroll_detail(3);
                        } else {
                            dashboard.navigate(Direction::Down);
                        }
                    }
                    KeyCode::PageUp => {
                        if dashboard.show_detail {
                            dashboard.scroll_detail(-20);
                        } else {
                            dashboard.page_up();
                        }
                    }
                    KeyCode::PageDown => {
                        if dashboard.show_detail {
                            dashboard.scroll_detail(20);
                        } else {
                            dashboard.page_down();
                        }
                    }
                    _ => {}
                },
                _ => {}
            } // end outer match crossterm::event::read()
        } else if !dashboard.all_loaded() {
            dashboard.tick_spinner();
        }
    })();

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

fn main() {
    let args = Args::parse();
    let kinds = filter_kinds(&args.provider);
    let config = Config::load();

    // Hidden: fetch + write cache, exit silently (used by background refresh).
    if args.update_cache {
        let _ = fetch_all(kinds, &config);
        return;
    }

    if args.raw {
        let results = fetch_all(kinds, &config);
        let mut map = serde_json::Map::new();
        for r in results {
            let key = r.kind.slug().to_string();
            let value = match r.raw_response {
                Some(v) => v,
                None => serde_json::json!({
                    "status": match r.status {
                        quotas::providers::ProviderStatus::AuthRequired => "auth_required",
                        quotas::providers::ProviderStatus::NetworkError { .. } => "network_error",
                        quotas::providers::ProviderStatus::Unavailable { .. } => "unavailable",
                        quotas::providers::ProviderStatus::Available { .. } => "available_no_raw",
                    },
                    "note": "no raw response captured"
                }),
            };
            map.insert(key, value);
        }
        let wrapped = serde_json::Value::Object(map);
        println!(
            "{}",
            serde_json::to_string_pretty(&wrapped).unwrap_or_default()
        );
        return;
    }

    if args.json {
        let results = if args.cached {
            let cache = cache::read_cache();
            cache
                .entries
                .into_values()
                .map(|e| {
                    let mut result = e.result;
                    result.cached_at = Some(e.cached_at);
                    result
                })
                .collect()
        } else if args.ensure_current {
            fetch_with_staleness_check(kinds, &config)
        } else {
            fetch_all(kinds, &config)
        };
        let cache = cache::read_cache();
        println!(
            "{}",
            JsonOutput::from_results(results, &cache).to_json(args.pretty)
        );
        return;
    }

    if args.statusline {
        run_statusline(&args, &config);
        return;
    }

    if args.snap {
        run_snap(
            kinds,
            config,
            args.snap_width,
            args.snap_height,
            args.snap_output.as_deref(),
        );
        return;
    }

    if let Err(e) = run_tui(kinds.clone(), config, args.cached) {
        eprintln!("Error: {:?}", e);
    }
}

const BG_REFRESH_THRESHOLD_SECS: u64 = 30 * 60; // 30 minutes

fn run_statusline(args: &Args, config: &Config) {
    let cache = cache::read_cache();

    let sl_config = StatusLineConfig {
        icons: if args.no_icons {
            false
        } else {
            config.statusline.icons
        },
        providers: filter_kinds(&args.provider),
        format: args.format.clone(),
    };

    let line = statusline::render(&cache, &sl_config);
    if !line.is_empty() {
        println!("{line}");
    }

    // Background refresh: if cache is stale, fork off a background process
    // to re-fetch and merge into cache.
    let bg_enabled = if args.no_bg_refresh {
        false
    } else {
        config.statusline.bg_refresh
    };
    if bg_enabled && !args.cached {
        if let Some(age) = cache::cache_age(&cache) {
            if age.as_secs() >= BG_REFRESH_THRESHOLD_SECS {
                let _ = spawn_bg_refresh();
            }
        }
    }
}

fn spawn_bg_refresh() -> io::Result<()> {
    let self_exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(self_exe);
    cmd.arg("--update-cache");
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        cmd.arg("--"); // ensure clean arg boundary
    }
    cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_file_accepts_raw_token_on_first_line() {
        assert_eq!(parse_key_file("sk-cp-abc\n").as_deref(), Some("sk-cp-abc"));
    }

    #[test]
    fn parse_key_file_accepts_api_key_assignment() {
        assert_eq!(
            parse_key_file("# my key\napi_key=\"sk-cp-xyz\"\n").as_deref(),
            Some("sk-cp-xyz")
        );
    }

    #[test]
    fn parse_key_file_skips_comments_and_blanks() {
        assert_eq!(
            parse_key_file("\n# comment\n\nsk-live-0\n").as_deref(),
            Some("sk-live-0")
        );
    }

    #[test]
    fn parse_key_file_empty_returns_none() {
        assert_eq!(parse_key_file(""), None);
    }

    #[test]
    fn startup_fetch_runs_by_default_when_credentials_exist() {
        assert!(should_startup_fetch(false, true, true));
    }

    #[test]
    fn startup_fetch_stops_when_disabled() {
        assert!(!should_startup_fetch(false, false, true));
    }

    #[test]
    fn startup_fetch_stops_in_cached_mode() {
        assert!(!should_startup_fetch(true, true, true));
    }

    #[test]
    fn startup_fetch_requires_credentials() {
        assert!(!should_startup_fetch(false, true, false));
    }

    #[test]
    fn periodic_refresh_follows_tui_toggle() {
        assert!(should_periodic_refresh(false, true));
        assert!(!should_periodic_refresh(false, false));
        assert!(!should_periodic_refresh(true, true));
    }

    #[test]
    fn refresh_timer_backdates_only_when_startup_refresh_is_allowed() {
        assert!(should_backdate_refresh_timer(true, false));
        assert!(!should_backdate_refresh_timer(false, false));
        assert!(!should_backdate_refresh_timer(true, true));
    }

    #[test]
    fn snap_page_output_path_inserts_page_before_extension() {
        let path = snap_page_output_path("screenshots/snap.txt", 2);
        assert_eq!(path.to_string_lossy(), "screenshots/snap-page2.txt");
    }

    #[test]
    fn snap_page_output_path_handles_no_extension() {
        let path = snap_page_output_path("screenshots/snap", 3);
        assert_eq!(path.to_string_lossy(), "screenshots/snap-page3");
    }
}
