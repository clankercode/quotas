use crate::providers::{ProviderResult, ProviderStatus};
use crate::tui::freshness::FreshnessLabel;
use crate::tui::usage_bar::UsageBar;
use ratatui::prelude::*;

pub struct DetailView {
    pub result: ProviderResult,
}

impl DetailView {
    pub fn new(result: ProviderResult) -> Self {
        Self { result }
    }

    pub fn render(&self) -> Text<'_> {
        let mut lines: Vec<Line> = Vec::new();

        lines.push(Line::from(vec![Span::raw("═".repeat(40))]));
        lines.push(Line::from(vec![Span::raw(format!(
            " {} ",
            self.result.kind.display_name()
        ))
        .bold()]));
        lines.push(Line::from(vec![Span::raw("═".repeat(40))]));

        let freshness =
            FreshnessLabel::new((chrono::Utc::now() - self.result.fetched_at).num_seconds());
        let freshness_color = match freshness.staleness {
            crate::tui::freshness::Staleness::Fresh => Style::new().cyan(),
            crate::tui::freshness::Staleness::Warning => Style::new().yellow(),
            crate::tui::freshness::Staleness::Stale => Style::new().red(),
        };
        lines.push(Line::from(vec![Span::styled(
            freshness.label.clone(),
            freshness_color,
        )]));
        lines.push(Line::from(vec![Span::raw("")]));

        match &self.result.status {
            ProviderStatus::Available { quota } => {
                lines.push(Line::from(vec![Span::raw(format!(
                    "Plan: {}",
                    quota.plan_name
                ))]));
                if quota.unlimited {
                    lines.push(Line::from(vec![Span::raw("Status: Unlimited").green()]));
                } else {
                    lines.push(Line::from(vec![Span::raw("")]));
                    for window in &quota.windows {
                        lines.push(Line::from(vec![Span::raw(format!(
                            "── {} ──",
                            window.window_type
                        ))]));
                        if window.limit > 0 {
                            let pct = (window.used as f64 / window.limit as f64 * 100.0) as i64;
                            lines.push(Line::from(vec![Span::raw(format!(
                                "  Used: {} / {} ({}%)",
                                window.used, window.limit, pct
                            ))]));
                            lines.push(Line::from(vec![Span::raw(format!(
                                "  Remaining: {}",
                                window.remaining
                            ))]));
                            lines.push(Line::from(vec![Span::raw(UsageBar::render(
                                window.used,
                                window.limit,
                                &window.window_type,
                            ))]));
                        } else if window.window_type == "payg_balance" {
                            lines.push(Line::from(vec![Span::raw(format!(
                                "  Balance: ${:.2}",
                                window.remaining as f64 / 100.0
                            ))]));
                        } else {
                            lines.push(Line::from(vec![Span::raw(format!(
                                "  Remaining: {}",
                                window.remaining
                            ))]));
                        }
                        if let Some(reset) = window.reset_at {
                            lines.push(Line::from(vec![Span::raw(format!(
                                "  Resets: {}",
                                reset.format("%Y-%m-%d %H:%M UTC")
                            ))]));
                        }
                        lines.push(Line::from(vec![Span::raw("")]));
                    }
                }
            }
            ProviderStatus::Unavailable { info } => {
                lines.push(Line::from(vec![Span::raw("Status: Unavailable").red()]));
                lines.push(Line::from(vec![Span::raw(format!(
                    "Reason: {}",
                    info.reason
                ))]));
                if let Some(url) = &info.console_url {
                    lines.push(Line::from(vec![Span::raw(format!("Console: {}", url))]));
                }
            }
            ProviderStatus::AuthRequired => {
                lines.push(Line::from(vec![Span::raw(
                    "Status: Authentication Required",
                )
                .yellow()]));
                lines.push(Line::from(vec![Span::raw(
                    "Set your API key via environment variable or config file.",
                )]));
            }
            ProviderStatus::NetworkError { message } => {
                lines.push(Line::from(vec![Span::raw("Status: Network Error").red()]));
                lines.push(Line::from(vec![Span::raw(format!("Error: {}", message))]));
            }
        }

        Text::from(lines)
    }
}
