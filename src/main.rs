use clap::Parser;
use crossterm::event::{Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use quotas::auth::env::EnvResolver;
use quotas::auth::file::FileResolver;
use quotas::auth::oauth::OAuthFileResolver;
use quotas::auth::opencode::{KimiCliResolver, OpencodeAuthResolver, OpencodeSlot};
use quotas::auth::refresh;
use quotas::auth::{AuthResolver, MultiResolver};
use quotas::config::Config;
use quotas::output::json::JsonOutput;
use quotas::providers::{Provider, ProviderKind, ProviderResult};
use quotas::tui::Dashboard;
use quotas::tui::Direction;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "quotas")]
#[command(about = "Check AI provider usage quotas from your configured credentials", long_about = None)]
struct Args {
    #[arg(long)]
    json: bool,

    #[arg(long)]
    pretty: bool,

    #[arg(long, value_delimiter = ',')]
    provider: Vec<String>,
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
            "minimax" => Some(ProviderKind::Minimax),
            "zai" | "zhipu" | "z.ai" | "glm" => Some(ProviderKind::Zai),
            "kimi" | "moonshot" => Some(ProviderKind::Kimi),
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
        ProviderKind::Minimax => Box::new(
            quotas::providers::minimax::MinimaxProvider::with_multi_resolver(MultiResolver::new(
                vec![auth],
            )),
        ),
        ProviderKind::Zai => Box::new(quotas::providers::zai::ZaiProvider::new(auth)),
        ProviderKind::Kimi => Box::new(quotas::providers::kimi::KimiProvider::new(auth)),
    };
    match provider.fetch().await {
        Ok(r) => r,
        Err(quotas::Error::Auth(msg)) => auth_required_result(kind, msg),
        Err(e) => network_error_result(kind, e.to_string()),
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
    }
}

fn network_error_result(kind: ProviderKind, message: String) -> ProviderResult {
    ProviderResult {
        kind,
        status: quotas::providers::ProviderStatus::NetworkError { message },
        fetched_at: chrono::Utc::now(),
        raw_response: None,
    }
}

fn fetch_all(kinds: Vec<ProviderKind>, config: &Config) -> Vec<ProviderResult> {
    kinds
        .into_iter()
        .map(|k| fetch_provider_sync(k, config))
        .collect()
}

fn spawn_fetches(
    rt: &tokio::runtime::Runtime,
    kinds: &[ProviderKind],
    config: Config,
    tx: tokio::sync::mpsc::UnboundedSender<(usize, ProviderResult)>,
) {
    for (idx, kind) in kinds.iter().cloned().enumerate() {
        let tx = tx.clone();
        let config = config.clone();
        rt.spawn(async move {
            let result = fetch_one(kind, &config).await;
            let _ = tx.send((idx, result));
        });
    }
}

fn run_tui(kinds: Vec<ProviderKind>, config: Config) -> io::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let mut dashboard = Dashboard::new_loading(kinds.clone());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(usize, ProviderResult)>();
    spawn_fetches(&rt, &kinds, config.clone(), tx);

    let tick = Duration::from_millis(80);
    let result: io::Result<()> = (|| loop {
        terminal.draw(|f| dashboard.render(f))?;

        while let Ok((idx, result)) = rx.try_recv() {
            dashboard.update(idx, result);
        }

        if crossterm::event::poll(tick)? {
            if let Event::Key(KeyEvent { code, .. }) = crossterm::event::read()? {
                match code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(()),
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        dashboard.reset_loading();
                        let (new_tx, new_rx) = tokio::sync::mpsc::unbounded_channel();
                        rx = new_rx;
                        spawn_fetches(&rt, &kinds, config.clone(), new_tx);
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
                    KeyCode::Enter => {
                        dashboard.show_detail = !dashboard.show_detail;
                    }
                    KeyCode::Left => dashboard.navigate(Direction::Left),
                    KeyCode::Right => dashboard.navigate(Direction::Right),
                    KeyCode::Up => dashboard.navigate(Direction::Up),
                    KeyCode::Down => dashboard.navigate(Direction::Down),
                    KeyCode::PageUp => dashboard.page_up(),
                    KeyCode::PageDown => dashboard.page_down(),
                    _ => {}
                }
            }
        } else if !dashboard.all_loaded() {
            dashboard.tick_spinner();
        }
    })();

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn main() {
    let args = Args::parse();
    let kinds = filter_kinds(&args.provider);
    let config = Config::load();

    if args.json {
        let results = fetch_all(kinds, &config);
        let output = JsonOutput::from_results(results);
        println!("{}", output.to_json(args.pretty));
        return;
    }

    if let Err(e) = run_tui(kinds.clone(), config) {
        eprintln!("Error: {:?}", e);
    }
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
}
