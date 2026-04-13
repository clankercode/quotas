use crate::providers::{ProviderKind, ProviderResult, ProviderStatus};
use crate::tui::freshness::FreshnessLabel;
use crate::tui::usage_bar::UsageBar;
use chrono::Utc;

pub struct ProviderCard {
    pub result: ProviderResult,
    pub selected: bool,
}

impl ProviderCard {
    pub fn new(result: ProviderResult, selected: bool) -> Self {
        Self { result, selected }
    }

    pub fn kind(&self) -> ProviderKind {
        self.result.kind
    }

    pub fn display_name(&self) -> &'static str {
        self.result.kind.display_name()
    }

    pub fn freshness_label(&self) -> FreshnessLabel {
        let secs = (Utc::now() - self.result.fetched_at).num_seconds();
        FreshnessLabel::new(secs)
    }

    pub fn primary_label(&self) -> String {
        match &self.result.status {
            ProviderStatus::Available { quota } => {
                if quota.unlimited {
                    return format!("{} [unlimited]", quota.plan_name);
                }
                let w = quota.windows.first();
                if let Some(w) = w {
                    if w.limit == 0 {
                        format!("{} [balance only]", quota.plan_name)
                    } else {
                        UsageBar::render(w.used, w.limit, &w.window_type)
                    }
                } else {
                    quota.plan_name.clone()
                }
            }
            ProviderStatus::Unavailable { info } => {
                format!("Unavailable: {}", info.reason)
            }
            ProviderStatus::AuthRequired => "Auth required".to_string(),
            ProviderStatus::NetworkError { message } => {
                format!("Error: {}", message)
            }
        }
    }

    pub fn secondary_lines(&self) -> Vec<String> {
        match &self.result.status {
            ProviderStatus::Available { quota } => {
                let mut lines = Vec::new();
                if let Some(w) = quota.windows.first() {
                    if w.limit > 0 {
                        lines.push(format!("{}/{} remaining", w.remaining, w.limit));
                    } else if let Some((sym, scale)) =
                        crate::tui::bar::currency_window(&w.window_type)
                    {
                        lines.push(format!("{}{:.2} balance", sym, w.remaining as f64 / scale));
                    }
                    if let Some(reset) = w.reset_at {
                        lines.push(format!("resets {}", reset.format("%b %d %H:%M")));
                    }
                }
                if quota.windows.len() > 1 {
                    lines.push(format!("+ {} more windows", quota.windows.len() - 1));
                }
                lines
            }
            ProviderStatus::Unavailable { info } => {
                if let Some(url) = &info.console_url {
                    vec![format!("See: {}", url)]
                } else {
                    vec![]
                }
            }
            ProviderStatus::AuthRequired => {
                vec!["Set API key in env or config".to_string()]
            }
            ProviderStatus::NetworkError { .. } => {
                vec!["Check your network connection".to_string()]
            }
        }
    }

    pub fn available(&self) -> bool {
        matches!(&self.result.status, ProviderStatus::Available { .. })
    }
}
