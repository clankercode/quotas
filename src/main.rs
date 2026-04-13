use clap::Parser;
use crossterm::event::{Event, KeyCode, KeyEvent};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use quotas::auth::env::EnvResolver;
use quotas::auth::file::FileResolver;
use quotas::auth::oauth::OAuthFileResolver;
use quotas::auth::{AuthResolver, MultiResolver};
use quotas::output::json::JsonOutput;
use quotas::providers::{Provider, ProviderKind, ProviderResult};
use quotas::tui::Direction;
use quotas::tui::Dashboard;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;

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

fn build_auth_resolver(kind: &ProviderKind) -> Box<dyn AuthResolver> {
    match kind {
        ProviderKind::Minimax => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(EnvResolver::new(vec![("MINIMAX_API_KEY", "minimax")])),
                Box::new(FileResolver::new(
                    vec![dirs::home_dir().unwrap_or_default().join(".minimax")],
                    |content| {
                        content
                            .lines()
                            .find(|l| l.starts_with("api_key="))
                            .map(|l| l.trim_start_matches("api_key=").to_string())
                    },
                    "minimax",
                )),
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
                    |content| content.lines().next().map(|l| l.to_string()),
                    "zai",
                )),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::Kimi => Box::new(MultiResolver::new(vec![Box::new(EnvResolver::new(
            vec![("MOONSHOT_API_KEY", "moonshot"), ("KIMI_API_KEY", "kimi")],
        ))])),
        ProviderKind::Copilot => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(EnvResolver::new(vec![("GITHUB_TOKEN", "github")])),
                Box::new(OAuthFileResolver::copilot()),
            ];
            Box::new(MultiResolver::new(resolvers))
        }
        ProviderKind::Codex => {
            let resolvers: Vec<Box<dyn AuthResolver>> = vec![
                Box::new(EnvResolver::new(vec![("OPENAI_API_KEY", "openai")])),
                Box::new(OAuthFileResolver::codex()),
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
            "minimax" => Some(ProviderKind::Minimax),
            "zai" | "zhipu" | "z.ai" => Some(ProviderKind::Zai),
            "kimi" | "moonshot" => Some(ProviderKind::Kimi),
            "copilot" | "github" | "github_copilot" => Some(ProviderKind::Copilot),
            "codex" | "chatgpt" => Some(ProviderKind::Codex),
            _ => None,
        })
        .collect()
}

fn fetch_provider_sync(kind: ProviderKind) -> Option<ProviderResult> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        let auth = build_auth_resolver(&kind);
        let result: Box<dyn Provider> = match kind {
            ProviderKind::Minimax => {
                Box::new(quotas::providers::minimax::MinimaxProvider::with_multi_resolver(
                    MultiResolver::new(vec![auth]),
                ))
            }
            ProviderKind::Zai => {
                Box::new(quotas::providers::zai::ZaiProvider::new(auth))
            }
            ProviderKind::Kimi => {
                Box::new(quotas::providers::kimi::KimiProvider::new(auth))
            }
            ProviderKind::Copilot => {
                Box::new(quotas::providers::copilot::CopilotProvider::new(auth))
            }
            ProviderKind::Codex => {
                Box::new(quotas::providers::codex::CodexProvider::new(auth))
            }
        };
        result.fetch().await.ok()
    })
}

fn fetch_all(kinds: Vec<ProviderKind>) -> Vec<ProviderResult> {
    kinds.into_iter().filter_map(fetch_provider_sync).collect()
}

fn run_tui(kinds: Vec<ProviderKind>) -> io::Result<()> {
    let initial_results = fetch_all(kinds.clone());
    let mut dashboard = Dashboard::new(initial_results);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        terminal.draw(|f| dashboard.render(f))?;

        if let Event::Key(KeyEvent { code, .. }) = crossterm::event::read()? {
            match code {
                KeyCode::Char('q') | KeyCode::Char('Q') => break,
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    let fetched = fetch_all(kinds.clone());
                    dashboard = Dashboard::new(fetched);
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
                KeyCode::Left => {
                    dashboard.navigate(Direction::Left);
                }
                KeyCode::Right => {
                    dashboard.navigate(Direction::Right);
                }
                KeyCode::Up => {
                    dashboard.navigate(Direction::Up);
                }
                KeyCode::Down => {
                    dashboard.navigate(Direction::Down);
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn main() {
    let args = Args::parse();
    let kinds = filter_kinds(&args.provider);

    if args.json {
        let results = fetch_all(kinds);
        let output = JsonOutput::from_results(results);
        println!("{}", output.to_json(args.pretty));
        return;
    }

    if let Err(e) = run_tui(kinds.clone()) {
        eprintln!("Error: {:?}", e);
    }
}
