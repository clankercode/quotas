use clap::Parser;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseButton, MouseEvent,
    MouseEventKind,
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
use quotas::auth::{AuthResolver, MultiResolver, StaticResolver};
use quotas::cache;
use quotas::config::Config;
use quotas::output::json::JsonOutput;
use quotas::output::statusline::{self, StatusLineConfig};
use quotas::providers::{Provider, ProviderKind, ProviderResult};
use quotas::tui::Dashboard;
use quotas::tui::DetailMode;
use quotas::tui::Direction;
use quotas::tui::HitResult;
use quotas::tui::ProviderEntry;
use quotas::update;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "quotas")]
#[command(version = env!("CARGO_PKG_VERSION"))]
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

    /// Check crates.io for a newer quotas release and exit.
    #[arg(long)]
    check_update: bool,

    /// Disable automatic background crates.io update checks.
    #[arg(long)]
    no_update_check: bool,

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

    /// Hidden: refresh the crates.io version cache and exit silently.
    #[arg(long, hide = true)]
    update_version_cache: bool,

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

fn env_auth(vars: &[(&'static str, &'static str)]) -> Box<dyn AuthResolver> {
    Box::new(EnvResolver::new(vars.to_vec()))
}

fn home_paths(paths: &[&str]) -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    paths.iter().map(|path| home.join(path)).collect()
}

fn key_file_auth(paths: &[&str], source_name: &str) -> Box<dyn AuthResolver> {
    Box::new(FileResolver::new(
        home_paths(paths),
        parse_key_file,
        source_name,
    ))
}

fn cookie_file_auth(paths: &[&str], source_name: &str) -> Box<dyn AuthResolver> {
    Box::new(CookieFileResolver::new(home_paths(paths), source_name))
}

fn opencode_auth(slot: OpencodeSlot) -> Box<dyn AuthResolver> {
    Box::new(OpencodeAuthResolver::new(slot))
}

fn multi_auth(resolvers: Vec<Box<dyn AuthResolver>>) -> Box<dyn AuthResolver> {
    Box::new(MultiResolver::new(resolvers))
}

fn build_auth_resolver(kind: &ProviderKind, config: &Config) -> Box<dyn AuthResolver> {
    match kind {
        ProviderKind::Minimax => multi_auth(vec![
            env_auth(&[("MINIMAX_API_KEY", "minimax")]),
            key_file_auth(&[".minimax"], "minimax"),
            opencode_auth(OpencodeSlot::Minimax),
        ]),
        ProviderKind::Zai => multi_auth(vec![
            env_auth(&[("ZHIPU_API_KEY", "zhipu"), ("ZAI_API_KEY", "zai")]),
            key_file_auth(&[".api-zai"], "zai"),
            opencode_auth(OpencodeSlot::Zai),
        ]),
        ProviderKind::Kimi => multi_auth(vec![
            env_auth(&[("MOONSHOT_API_KEY", "moonshot"), ("KIMI_API_KEY", "kimi")]),
            key_file_auth(&[".moonshot", ".kimi"], "kimi"),
            Box::new(KimiCliResolver::new()),
            opencode_auth(OpencodeSlot::Kimi),
        ]),
        ProviderKind::Claude => multi_auth(vec![
            Box::new(OAuthFileResolver::claude()),
            opencode_auth(OpencodeSlot::Anthropic),
            env_auth(&[("ANTHROPIC_API_KEY", "anthropic")]),
        ]),
        ProviderKind::Codex => multi_auth(vec![
            Box::new(OAuthFileResolver::codex()),
            opencode_auth(OpencodeSlot::Openai),
            env_auth(&[("OPENAI_API_KEY", "openai")]),
        ]),
        ProviderKind::Cursor => Box::new(CursorAuthResolver::new()),
        ProviderKind::DeepSeek => multi_auth(vec![
            env_auth(&[("DEEPSEEK_API_KEY", "deepseek")]),
            key_file_auth(&[".deepseek"], "deepseek"),
        ]),
        ProviderKind::Gemini => multi_auth(vec![
            Box::new(OAuthFileResolver::gemini()),
            env_auth(&[("GEMINI_API_KEY", "gemini")]),
            key_file_auth(&[".gemini-api-key"], "gemini"),
        ]),
        ProviderKind::SiliconFlow => multi_auth(vec![
            env_auth(&[
                ("SILICONFLOW_API_KEY", "siliconflow"),
                ("SILICON_FLOW_API_KEY", "siliconflow"),
            ]),
            key_file_auth(&[".siliconflow"], "siliconflow"),
        ]),
        ProviderKind::OpenRouter => multi_auth(vec![
            env_auth(&[("OPENROUTER_API_KEY", "openrouter")]),
            key_file_auth(&[".openrouter"], "openrouter"),
        ]),
        ProviderKind::Grok => multi_auth(vec![
            // Prefer Grok Build's cached session (same auth as `grok login`).
            Box::new(OAuthFileResolver::grok()),
            env_auth(&[
                ("XAI_MANAGEMENT_KEY", "xai_management"),
                ("XAI_MGMT_KEY", "xai_mgmt"),
                ("GROK_MANAGEMENT_KEY", "grok_management"),
            ]),
            key_file_auth(
                &[
                    ".xai-management-key",
                    ".xai/management_key",
                    ".config/xai/management_key",
                ],
                "xai-management",
            ),
            // Inference API key is last-resort; billing may still reject it.
            env_auth(&[("XAI_API_KEY", "xai"), ("GROK_CODE_XAI_API_KEY", "grok_code")]),
            key_file_auth(&[".xai", ".xai-api-key"], "xai"),
        ]),
        ProviderKind::GitHubCopilot => {
            let mut resolvers = vec![
                opencode_auth(OpencodeSlot::GitHubCopilot),
                env_auth(&[("GITHUB_COPILOT_TOKEN", "github_copilot")]),
            ];
            if let Some(token) = config.github_copilot.token.clone() {
                resolvers.push(Box::new(StaticResolver {
                    token,
                    source: "config:github_copilot.token".into(),
                }));
            }
            multi_auth(resolvers)
        }
        ProviderKind::Mimo => multi_auth(vec![
            cookie_file_auth(&[".config/mimo/cookie", ".mimo-cookie"], "mimo-cookie"),
            env_auth(&[
                ("XIAOMI_MIMO_API_KEY", "xiaomi_mimo"),
                ("XIAOMI_API_KEY", "xiaomi"),
                ("MIMO_API_KEY", "mimo"),
            ]),
            key_file_auth(&[".mimo-key", ".xiaomimimo", ".mimo"], "mimo"),
        ]),
    }
}

fn filter_kinds(requested: &[String], config: &Config, cached: bool) -> Vec<ProviderKind> {
    // CLI --provider overrides config whitelist
    if !requested.is_empty() {
        let config_enabled = config.providers_enabled_kinds();
        let requested_kinds: Vec<_> = requested
            .iter()
            .filter_map(|s| normalize_provider(s))
            .filter(|k| config_enabled.contains(k))
            .collect();

        // --cached: further filter to only those present in cache
        if cached {
            let cache = cache::read_cache();
            return requested_kinds
                .into_iter()
                .filter(|k| cache.entries.contains_key(k.slug()))
                .collect();
        }
        return requested_kinds;
    }

    if cached {
        // No CLI filter with --cached: show whatever is in cache
        let cache = cache::read_cache();
        return cache
            .entries
            .keys()
            .filter_map(|s| normalize_provider(s))
            .collect();
    }

    // Use config's provider list
    config.providers_enabled_kinds()
}

fn normalize_provider(name: &str) -> Option<ProviderKind> {
    match name.to_lowercase().as_str() {
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
        "grok" | "xai" | "x.ai" | "x-ai" | "x_ai" => Some(ProviderKind::Grok),
        "copilot" | "github-copilot" | "github_copilot" | "githubcopilot" => {
            Some(ProviderKind::GitHubCopilot)
        }
        _ => None,
    }
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
    let auth = build_auth_resolver(&kind, config);
    let provider = build_provider(kind, auth);
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

fn build_provider(kind: ProviderKind, auth: Box<dyn AuthResolver>) -> Box<dyn Provider> {
    match kind {
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
        ProviderKind::Grok => Box::new(quotas::providers::grok::GrokProvider::new(auth)),
        ProviderKind::Mimo => Box::new(quotas::providers::mimo::MimoProvider::new(auth)),
        ProviderKind::GitHubCopilot => {
            Box::new(quotas::providers::github_copilot::GitHubCopilotProvider::new(auth))
        }
    }
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

struct PeriodicRefreshState<'a> {
    cached: bool,
    auto_refresh_enabled: bool,
    show_detail: bool,
    selected_entry: Option<usize>,
    kinds: &'a [ProviderKind],
    auth_ready: &'a [bool],
    entry_done: &'a [bool],
    elapsed: &'a [Duration],
}

fn periodic_refresh_candidates(state: PeriodicRefreshState<'_>) -> Vec<usize> {
    if !should_periodic_refresh(state.cached, state.auto_refresh_enabled) {
        return Vec::new();
    }

    let indices: Vec<usize> = if state.show_detail {
        state.selected_entry.into_iter().collect()
    } else {
        (0..state.kinds.len()).collect()
    };

    indices
        .into_iter()
        .filter(|&idx| state.auth_ready.get(idx).copied().unwrap_or(false))
        .filter(|&idx| state.entry_done.get(idx).copied().unwrap_or(false))
        .filter(|&idx| {
            state
                .elapsed
                .get(idx)
                .is_some_and(|elapsed| *elapsed >= auto_refresh_interval(state.kinds[idx]))
        })
        .collect()
}

fn should_backdate_refresh_timer(refresh_on_start: bool, startup_fetching: bool) -> bool {
    refresh_on_start && !startup_fetching
}

fn refresh_update_cache_sync() -> quotas::Result<update::UpdateInfo> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(update::refresh_update_cache(env!("CARGO_PKG_VERSION")))
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

struct TuiStartupPlan {
    entries: Vec<ProviderEntry>,
    fetch_indices: Vec<usize>,
    auth_ready: Vec<bool>,
}

struct PlannedProviderEntry {
    entry: ProviderEntry,
    fetch_on_start: bool,
}

fn build_tui_startup_plan(
    kinds: &[ProviderKind],
    config: &Config,
    disk_cache: &cache::CacheFile,
    now: chrono::DateTime<chrono::Utc>,
    cached: bool,
) -> TuiStartupPlan {
    let auth_resolvers: Vec<Box<dyn AuthResolver>> = kinds
        .iter()
        .map(|kind| build_auth_resolver(kind, config))
        .collect();
    let auth_ready: Vec<bool> = auth_resolvers
        .iter()
        .map(|resolver| resolver.have_credentials())
        .collect();

    let mut entries = Vec::with_capacity(kinds.len());
    let mut fetch_indices = Vec::new();
    for (idx, kind) in kinds.iter().enumerate() {
        let cached_entry = disk_cache.entries.get(kind.slug());
        let planned = plan_tui_provider_entry(
            *kind,
            cached_entry,
            cached,
            config.tui.refresh_on_start,
            auth_ready[idx],
            now,
        );
        if planned.fetch_on_start {
            fetch_indices.push(idx);
        }
        entries.push(planned.entry);
    }

    TuiStartupPlan {
        entries,
        fetch_indices,
        auth_ready,
    }
}

fn plan_tui_provider_entry(
    kind: ProviderKind,
    cached_entry: Option<&cache::CacheEntry>,
    cached_mode: bool,
    refresh_on_start: bool,
    has_creds: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> PlannedProviderEntry {
    let fetch_on_start = should_startup_fetch(cached_mode, refresh_on_start, has_creds);

    if cached_mode {
        let result = match cached_entry {
            Some(entry) => cached_provider_result(entry),
            None if has_creds => ProviderResult {
                kind,
                status: quotas::providers::ProviderStatus::AuthRequired,
                fetched_at: now,
                raw_response: None,
                auth_source: None,
                cached_at: None,
            },
            None => auth_required_result(kind, "no credentials".into()),
        };
        return PlannedProviderEntry {
            entry: ProviderEntry::Done(result),
            fetch_on_start: false,
        };
    }

    if let Some(entry) = cached_entry {
        let result = cached_provider_result(entry);
        return PlannedProviderEntry {
            entry: if fetch_on_start {
                ProviderEntry::Refreshing(result)
            } else {
                ProviderEntry::Done(result)
            },
            fetch_on_start,
        };
    }

    if has_creds {
        if fetch_on_start {
            return PlannedProviderEntry {
                entry: ProviderEntry::Loading,
                fetch_on_start: true,
            };
        }

        return PlannedProviderEntry {
            entry: ProviderEntry::Done(ProviderResult {
                kind,
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
            }),
            fetch_on_start: false,
        };
    }

    PlannedProviderEntry {
        entry: ProviderEntry::Done(auth_required_result(kind, "no credentials".into())),
        fetch_on_start: false,
    }
}

fn cached_provider_result(entry: &cache::CacheEntry) -> ProviderResult {
    let mut result = entry.result.clone();
    result.cached_at = Some(entry.cached_at);
    result
}

type FetchMessage = (usize, ProviderResult);
type FetchReceiver = tokio::sync::mpsc::UnboundedReceiver<FetchMessage>;
type FetchSender = tokio::sync::mpsc::UnboundedSender<FetchMessage>;

struct TuiFetchController {
    tx: FetchSender,
    rx: FetchReceiver,
    last_refresh: Vec<Instant>,
}

impl TuiFetchController {
    fn new(
        kinds: &[ProviderKind],
        disk_cache: &cache::CacheFile,
        now: chrono::DateTime<chrono::Utc>,
        startup_fetch_indices: &[usize],
        refresh_on_start: bool,
    ) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let last_refresh = initial_last_refresh(
            kinds,
            disk_cache,
            now,
            startup_fetch_indices,
            refresh_on_start,
        );
        Self {
            tx,
            rx,
            last_refresh,
        }
    }

    fn spawn_startup(
        &self,
        rt: &tokio::runtime::Runtime,
        kinds: &[ProviderKind],
        indices: &[usize],
        config: &Config,
    ) {
        self.spawn_fetches(rt, kinds, indices, config);
    }

    fn drain_completed(&mut self, dashboard: &mut Dashboard) {
        while let Ok((idx, result)) = self.rx.try_recv() {
            cache::write_cache(std::slice::from_ref(&result));
            dashboard.update(idx, result);
            self.mark_finished(idx);
        }
    }

    fn refresh_due(
        &mut self,
        rt: &tokio::runtime::Runtime,
        kinds: &[ProviderKind],
        config: &Config,
        cached: bool,
        auth_ready: &[bool],
        dashboard: &mut Dashboard,
    ) {
        if !should_periodic_refresh(cached, dashboard.auto_refresh_enabled) {
            return;
        }

        let elapsed: Vec<Duration> = self
            .last_refresh
            .iter()
            .map(|instant| instant.elapsed())
            .collect();
        let entry_done: Vec<bool> = (0..kinds.len())
            .map(|idx| dashboard.is_entry_done(idx))
            .collect();
        let targets = periodic_refresh_candidates(PeriodicRefreshState {
            cached,
            auto_refresh_enabled: dashboard.auto_refresh_enabled,
            show_detail: dashboard.show_detail,
            selected_entry: dashboard.selected_entry_index(),
            kinds,
            auth_ready,
            entry_done: &entry_done,
            elapsed: &elapsed,
        });

        self.refresh_indices(rt, kinds, &targets, config, dashboard);
    }

    fn refresh_all_fetchable(
        &mut self,
        rt: &tokio::runtime::Runtime,
        kinds: &[ProviderKind],
        config: &Config,
        auth_ready: &[bool],
        dashboard: &mut Dashboard,
    ) {
        self.replace_channel();
        let fetchable = fetchable_indices(kinds.len(), auth_ready);
        self.refresh_indices(rt, kinds, &fetchable, config, dashboard);
    }

    fn replace_channel(&mut self) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.tx = tx;
        self.rx = rx;
    }

    fn refresh_indices(
        &mut self,
        rt: &tokio::runtime::Runtime,
        kinds: &[ProviderKind],
        indices: &[usize],
        config: &Config,
        dashboard: &mut Dashboard,
    ) {
        for &idx in indices {
            dashboard.reset_one(idx);
        }
        self.spawn_fetches(rt, kinds, indices, config);
        self.mark_started(indices);
    }

    fn spawn_fetches(
        &self,
        rt: &tokio::runtime::Runtime,
        kinds: &[ProviderKind],
        indices: &[usize],
        config: &Config,
    ) {
        for &idx in indices {
            let Some(&kind) = kinds.get(idx) else {
                continue;
            };
            let tx = self.tx.clone();
            let config = config.clone();
            rt.spawn(async move {
                let result = fetch_one(kind, &config).await;
                let _ = tx.send((idx, result));
            });
        }
    }

    fn mark_started(&mut self, indices: &[usize]) {
        for &idx in indices {
            if let Some(last_refresh) = self.last_refresh.get_mut(idx) {
                *last_refresh = Instant::now();
            }
        }
    }

    fn mark_finished(&mut self, idx: usize) {
        if let Some(last_refresh) = self.last_refresh.get_mut(idx) {
            *last_refresh = Instant::now();
        }
    }
}

fn initial_last_refresh(
    kinds: &[ProviderKind],
    disk_cache: &cache::CacheFile,
    now: chrono::DateTime<chrono::Utc>,
    startup_fetch_indices: &[usize],
    refresh_on_start: bool,
) -> Vec<Instant> {
    let instant_now = Instant::now();
    kinds
        .iter()
        .enumerate()
        .map(|(idx, kind)| {
            let startup_fetching = startup_fetch_indices.contains(&idx);
            if startup_fetching {
                instant_now
            } else if should_backdate_refresh_timer(refresh_on_start, startup_fetching) {
                if let Some(entry) = disk_cache.entries.get(kind.slug()) {
                    let age_secs = (now - entry.cached_at).num_seconds().max(0) as u64;
                    let elapsed = Duration::from_secs(age_secs);
                    instant_now.checked_sub(elapsed).unwrap_or(instant_now)
                } else {
                    instant_now
                }
            } else {
                instant_now
            }
        })
        .collect()
}

fn fetchable_indices(provider_count: usize, auth_ready: &[bool]) -> Vec<usize> {
    (0..provider_count)
        .filter(|&idx| auth_ready.get(idx).copied().unwrap_or(false))
        .collect()
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

fn apply_dashboard_config(dashboard: &mut Dashboard, config: &Config) {
    dashboard.show_all_windows = config.ui.show_all_windows;
    dashboard.auto_refresh_enabled = config.tui.auto_refresh;
    for provider in &config.favorites.providers {
        dashboard.set_provider_favorite(provider, true);
    }
    for (provider, preferences) in &config.quota_preferences {
        dashboard.set_quota_preferences(provider, preferences.clone());
    }
    if dashboard.all_loaded() {
        dashboard.refresh_visual_order();
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
                ProviderEntry::Done(cached_provider_result(entry))
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
    apply_dashboard_config(&mut dashboard, &config);

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum TuiLoopAction {
    Continue,
    Quit,
}

struct TuiEventContext<'a> {
    rt: &'a tokio::runtime::Runtime,
    kinds: &'a [ProviderKind],
    auth_ready: &'a [bool],
    config: &'a mut Config,
    dashboard: &'a mut Dashboard,
    fetches: &'a mut TuiFetchController,
}

fn handle_tui_event(event: Event, ctx: TuiEventContext<'_>) -> TuiLoopAction {
    match event {
        Event::Mouse(mouse) => handle_mouse_event(mouse, ctx),
        Event::Key(key) => handle_key_event(key.code, ctx),
        _ => TuiLoopAction::Continue,
    }
}

fn handle_mouse_event(mouse: MouseEvent, ctx: TuiEventContext<'_>) -> TuiLoopAction {
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            if ctx.dashboard.show_detail {
                ctx.dashboard.scroll_detail(3);
            } else {
                ctx.dashboard.navigate(Direction::Down);
            }
        }
        MouseEventKind::ScrollUp => {
            if ctx.dashboard.show_detail {
                ctx.dashboard.scroll_detail(-3);
            } else {
                ctx.dashboard.navigate(Direction::Up);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            match ctx.dashboard.hit_test(mouse.column, mouse.row) {
                Some(HitResult::Refresh) => {
                    ctx.fetches.refresh_all_fetchable(
                        ctx.rt,
                        ctx.kinds,
                        ctx.config,
                        ctx.auth_ready,
                        ctx.dashboard,
                    );
                }
                Some(HitResult::AutoRefreshToggle) => {
                    ctx.dashboard.auto_refresh_enabled = !ctx.dashboard.auto_refresh_enabled;
                }
                Some(HitResult::Quit) => return TuiLoopAction::Quit,
                Some(HitResult::OpenUpdate) => {
                    if let Some(url) = ctx.dashboard.update_release_url() {
                        quotas::update::open_url(&url);
                    }
                }
                Some(HitResult::Card(vpos)) => {
                    if ctx.dashboard.selected_index == vpos && !ctx.dashboard.show_detail {
                        // Second click on already-selected card opens detail.
                        ctx.dashboard.show_detail = true;
                        ctx.dashboard.detail_mode = DetailMode::Auto;
                        ctx.dashboard.reset_detail_position();
                    } else {
                        ctx.dashboard.selected_index = vpos;
                        ctx.dashboard.show_detail = false;
                    }
                }
                None => {}
            }
        }
        MouseEventKind::Moved => {
            ctx.dashboard.set_mouse_pos(mouse.column, mouse.row);
        }
        _ => {}
    }

    TuiLoopAction::Continue
}

fn handle_key_event(code: KeyCode, ctx: TuiEventContext<'_>) -> TuiLoopAction {
    match code {
        KeyCode::Char('q') | KeyCode::Char('Q') => return TuiLoopAction::Quit,
        KeyCode::Esc | KeyCode::Backspace if ctx.dashboard.show_detail => {
            ctx.dashboard.show_detail = false;
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            ctx.fetches.refresh_all_fetchable(
                ctx.rt,
                ctx.kinds,
                ctx.config,
                ctx.auth_ready,
                ctx.dashboard,
            );
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if let Some(selected) = ctx.dashboard.selected_provider() {
                if let Ok(json) = serde_json::to_string_pretty(selected) {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        let _ = clipboard.set_text(&json);
                    }
                }
            }
        }
        KeyCode::Char('f') | KeyCode::Char('F') if ctx.dashboard.show_detail => {
            if let Some(provider) = ctx.dashboard.selected_provider_slug() {
                if let Some(row) = ctx.dashboard.selected_detail_row() {
                    ctx.config.toggle_quota_favorite(&provider, &row.quota_key);
                    let prefs = ctx.config.quota_preferences_for(&provider);
                    ctx.dashboard.set_quota_preferences(&provider, prefs);
                } else {
                    ctx.config.toggle_provider_favorite(&provider);
                    ctx.dashboard.toggle_selected_provider_favorite();
                }
                let _ = ctx.config.save();
            }
        }
        KeyCode::Char('f') | KeyCode::Char('F') => {
            if let Some(provider) = ctx.dashboard.toggle_selected_provider_favorite() {
                ctx.config.toggle_provider_favorite(&provider);
                let _ = ctx.config.save();
            }
        }
        KeyCode::Char('x') | KeyCode::Char('X') if ctx.dashboard.show_detail => {
            if let Some(provider) = ctx.dashboard.selected_provider_slug() {
                if let Some(row) = ctx.dashboard.selected_detail_row() {
                    ctx.config.toggle_quota_hidden(&provider, &row.quota_key);
                    let prefs = ctx.config.quota_preferences_for(&provider);
                    ctx.dashboard.set_quota_preferences(&provider, prefs);
                    ctx.dashboard.move_detail_focus(0);
                    let _ = ctx.config.save();
                }
            }
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            ctx.dashboard.auto_refresh_enabled = !ctx.dashboard.auto_refresh_enabled;
        }
        KeyCode::Enter => {
            if ctx.dashboard.show_detail {
                ctx.dashboard.show_detail = false;
                ctx.dashboard.reset_detail_position();
            } else {
                ctx.dashboard.show_detail = true;
                ctx.dashboard.detail_mode = DetailMode::Auto;
                ctx.dashboard.reset_detail_position();
            }
        }
        KeyCode::Tab if ctx.dashboard.show_detail => {
            ctx.dashboard.cycle_detail_mode();
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if ctx.dashboard.show_detail {
                ctx.dashboard.detail_prev();
            } else {
                ctx.dashboard.navigate(Direction::Left);
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if ctx.dashboard.show_detail {
                ctx.dashboard.detail_next();
            } else {
                ctx.dashboard.navigate(Direction::Right);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if ctx.dashboard.show_detail {
                ctx.dashboard.move_detail_focus(-1);
            } else {
                ctx.dashboard.navigate(Direction::Up);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if ctx.dashboard.show_detail {
                ctx.dashboard.move_detail_focus(1);
            } else {
                ctx.dashboard.navigate(Direction::Down);
            }
        }
        KeyCode::PageUp => {
            if ctx.dashboard.show_detail {
                ctx.dashboard.scroll_detail(-20);
            } else {
                ctx.dashboard.page_up();
            }
        }
        KeyCode::PageDown => {
            if ctx.dashboard.show_detail {
                ctx.dashboard.scroll_detail(20);
            } else {
                ctx.dashboard.page_down();
            }
        }
        _ => {}
    }

    TuiLoopAction::Continue
}

fn run_tui(
    kinds: Vec<ProviderKind>,
    config: Config,
    cached: bool,
    update_check_enabled: bool,
) -> io::Result<()> {
    let mut config = config;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .map_err(io::Error::other)?;

    let disk_cache = cache::read_cache();
    let now = chrono::Utc::now();
    let TuiStartupPlan {
        entries: initial_entries,
        fetch_indices,
        auth_ready,
    } = build_tui_startup_plan(&kinds, &config, &disk_cache, now, cached);

    let mut dashboard = Dashboard::new_with_entries(kinds.clone(), initial_entries);
    dashboard.set_update_info(update::cached_update_info(env!("CARGO_PKG_VERSION")));
    let mut update_rx = None;
    if update_check_enabled && !cached && update::should_check_for_update(now) {
        let (tx, rx) = std::sync::mpsc::channel();
        update_rx = Some(rx);
        rt.spawn(async move {
            let info = update::refresh_update_cache(env!("CARGO_PKG_VERSION"))
                .await
                .ok()
                .filter(|info| info.is_update_available());
            let _ = tx.send(info);
        });
    }
    apply_dashboard_config(&mut dashboard, &config);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut fetches = TuiFetchController::new(
        &kinds,
        &disk_cache,
        now,
        &fetch_indices,
        config.tui.refresh_on_start,
    );
    fetches.spawn_startup(&rt, &kinds, &fetch_indices, &config);

    let tick = Duration::from_millis(80);
    let result: io::Result<()> = (|| loop {
        terminal.draw(|f| dashboard.render(f))?;

        fetches.drain_completed(&mut dashboard);
        if let Some(rx) = &update_rx {
            if let Ok(info) = rx.try_recv() {
                dashboard.set_update_info(info);
                update_rx = None;
            }
        }

        // Per-provider auto-refresh: each provider refreshes on its own schedule.
        // Claude uses a longer interval to avoid rate-limiting.
        // Skip providers without credentials.
        fetches.refresh_due(&rt, &kinds, &config, cached, &auth_ready, &mut dashboard);

        if crossterm::event::poll(tick)? {
            let action = handle_tui_event(
                crossterm::event::read()?,
                TuiEventContext {
                    rt: &rt,
                    kinds: &kinds,
                    auth_ready: &auth_ready,
                    config: &mut config,
                    dashboard: &mut dashboard,
                    fetches: &mut fetches,
                },
            );
            if action == TuiLoopAction::Quit {
                return Ok(());
            }
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
    let config = Config::load();
    let kinds = filter_kinds(&args.provider, &config, args.cached);

    // Hidden: fetch + write cache, exit silently (used by background refresh).
    if args.update_cache {
        let _ = fetch_all(kinds, &config);
        return;
    }

    // Hidden: refresh crates.io update cache, exit silently.
    if args.update_version_cache {
        let _ = refresh_update_cache_sync();
        return;
    }

    if args.check_update {
        match refresh_update_cache_sync() {
            Ok(info) if info.is_update_available() => println!("{}", info.summary()),
            Ok(info) => println!("quotas {} is current", info.current_version),
            Err(e) => {
                eprintln!("update check failed: {e}");
                std::process::exit(1);
            }
        }
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

    if let Err(e) = run_tui(kinds.clone(), config, args.cached, !args.no_update_check) {
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
        providers: filter_kinds(&args.provider, config, args.cached),
        format: args.format.clone(),
        update_info: update::cached_update_info(env!("CARGO_PKG_VERSION")),
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

    if !args.no_update_check && !args.cached && update::should_check_for_update(chrono::Utc::now())
    {
        let _ = spawn_bg_version_check();
    }
}

fn spawn_bg_refresh() -> io::Result<()> {
    spawn_detached(&["--update-cache"])
}

fn spawn_bg_version_check() -> io::Result<()> {
    spawn_detached(&["--update-version-cache"])
}

fn spawn_detached(args: &[&str]) -> io::Result<()> {
    let self_exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(self_exe);
    for arg in args {
        cmd.arg(arg);
    }
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
    fn startup_plan_uses_cached_entry_without_fetch_in_cached_mode() {
        let now = chrono::Utc::now();
        let cached = test_cache_entry(ProviderKind::Claude);

        let planned =
            plan_tui_provider_entry(ProviderKind::Claude, Some(&cached), true, true, true, now);

        assert!(!planned.fetch_on_start);
        match planned.entry {
            ProviderEntry::Done(result) => {
                assert_eq!(result.kind, ProviderKind::Claude);
                assert_eq!(result.cached_at, Some(cached.cached_at));
            }
            _ => panic!("expected cached result to be done"),
        }
    }

    #[test]
    fn startup_plan_refreshes_cached_entry_when_startup_fetch_enabled() {
        let now = chrono::Utc::now();
        let cached = test_cache_entry(ProviderKind::Codex);

        let planned =
            plan_tui_provider_entry(ProviderKind::Codex, Some(&cached), false, true, true, now);

        assert!(planned.fetch_on_start);
        match planned.entry {
            ProviderEntry::Refreshing(result) => {
                assert_eq!(result.kind, ProviderKind::Codex);
                assert_eq!(result.cached_at, Some(cached.cached_at));
            }
            _ => panic!("expected cached result to refresh"),
        }
    }

    #[test]
    fn startup_plan_reports_missing_cache_when_refresh_disabled() {
        let planned = plan_tui_provider_entry(
            ProviderKind::Gemini,
            None,
            false,
            false,
            true,
            chrono::Utc::now(),
        );

        assert!(!planned.fetch_on_start);
        match planned.entry {
            ProviderEntry::Done(ProviderResult {
                status: quotas::providers::ProviderStatus::Unavailable { info },
                ..
            }) => {
                assert_eq!(info.reason, "No cached data; startup refresh disabled");
            }
            _ => panic!("expected unavailable result"),
        }
    }

    #[test]
    fn startup_plan_requires_auth_without_credentials() {
        let planned = plan_tui_provider_entry(
            ProviderKind::DeepSeek,
            None,
            false,
            true,
            false,
            chrono::Utc::now(),
        );

        assert!(!planned.fetch_on_start);
        match planned.entry {
            ProviderEntry::Done(ProviderResult {
                kind,
                status: quotas::providers::ProviderStatus::AuthRequired,
                ..
            }) => {
                assert_eq!(kind, ProviderKind::DeepSeek);
            }
            _ => panic!("expected auth required result"),
        }
    }

    #[test]
    fn periodic_refresh_follows_tui_toggle() {
        assert!(should_periodic_refresh(false, true));
        assert!(!should_periodic_refresh(false, false));
        assert!(!should_periodic_refresh(true, true));
    }

    #[test]
    fn manual_refresh_targets_only_authenticated_providers() {
        assert_eq!(fetchable_indices(4, &[true, false, true]), vec![0, 2]);
    }

    #[test]
    fn periodic_refresh_targets_only_selected_provider_in_detail_view() {
        let kinds = vec![
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::Gemini,
        ];
        let auth_ready = vec![true, true, true];
        let entry_done = vec![true, true, true];
        let elapsed = vec![
            Duration::from_secs(601),
            Duration::from_secs(301),
            Duration::from_secs(301),
        ];

        let targets = periodic_refresh_candidates(PeriodicRefreshState {
            cached: false,
            auto_refresh_enabled: true,
            show_detail: true,
            selected_entry: Some(1),
            kinds: &kinds,
            auth_ready: &auth_ready,
            entry_done: &entry_done,
            elapsed: &elapsed,
        });

        assert_eq!(targets, vec![1]);
    }

    #[test]
    fn periodic_refresh_targets_all_due_providers_on_dashboard() {
        let kinds = vec![
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::Gemini,
        ];
        let auth_ready = vec![true, true, false];
        let entry_done = vec![true, true, true];
        let elapsed = vec![
            Duration::from_secs(601),
            Duration::from_secs(301),
            Duration::from_secs(301),
        ];

        let targets = periodic_refresh_candidates(PeriodicRefreshState {
            cached: false,
            auto_refresh_enabled: true,
            show_detail: false,
            selected_entry: Some(1),
            kinds: &kinds,
            auth_ready: &auth_ready,
            entry_done: &entry_done,
            elapsed: &elapsed,
        });

        assert_eq!(targets, vec![0, 1]);
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

    fn test_cache_entry(kind: ProviderKind) -> cache::CacheEntry {
        let cached_at = chrono::Utc::now() - chrono::Duration::minutes(10);
        cache::CacheEntry {
            result: ProviderResult {
                kind,
                status: quotas::providers::ProviderStatus::AuthRequired,
                fetched_at: cached_at,
                raw_response: None,
                auth_source: None,
                cached_at: None,
            },
            cached_at,
        }
    }
}
