use crate::providers::{ProviderResult, ProviderStatus, QuotaWindow};
use crate::tui::bar;
use crate::tui::freshness::{FreshnessLabel, Staleness};
use chrono::Utc;
use ratatui::prelude::*;
use std::collections::BTreeSet;

pub struct DetailView {
    pub result: ProviderResult,
}

impl DetailView {
    pub fn new(result: ProviderResult) -> Self {
        Self { result }
    }

    pub fn render(&self, width: u16) -> Text<'_> {
        let mut lines: Vec<Line> = Vec::new();
        let bar_width: u16 = width.saturating_sub(16).clamp(10, 60);

        // Header — freshness only when we have valid auth data.
        let show_freshness = !matches!(self.result.status, ProviderStatus::AuthRequired);
        lines.push(Line::from(vec![Span::raw(" ")]));
        let mut header_spans = vec![
            Span::raw("  "),
            Span::raw(self.result.kind.display_name())
                .bold()
                .fg(Color::White),
        ];
        if show_freshness {
            header_spans.push(Span::raw("   "));
            header_spans.push(freshness_span(&self.result));
        }
        lines.push(Line::from(header_spans));

        // Auth source line (env var name, file path, oauth path, etc.)
        if let Some(source) = &self.result.auth_source {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::raw(pretty_auth_source(source)).dim(),
            ]));
        }

        match &self.result.status {
            ProviderStatus::Available { quota } => {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::raw(quota.plan_name.clone()).italic().dim(),
                ]));

                if quota.unlimited {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::raw("Unlimited plan — no quota cap").green(),
                    ]));
                } else if quota.windows.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![Span::raw(
                        "  No usage windows reported by this provider.",
                    )
                    .dim()]));
                } else {
                    lines.push(Line::from(""));
                    let mut sorted: Vec<&QuotaWindow> = quota.windows.iter().collect();
                    sorted.sort_by_key(|w| bar::window_sort_key(w));
                    let buckets_seen: BTreeSet<u8> =
                        sorted.iter().map(|w| bar::window_sort_key(w).0).collect();
                    let show_headers = sorted.len() >= 3 && buckets_seen.len() >= 2;
                    let mut last_bucket: Option<u8> = None;
                    for window in sorted {
                        let bucket = bar::window_sort_key(window).0;
                        if show_headers && Some(bucket) != last_bucket {
                            if let Some(label) = bar::bucket_label(bucket) {
                                lines.push(Line::from(vec![
                                    Span::raw("  "),
                                    Span::raw(label).dim(),
                                ]));
                            }
                            last_bucket = Some(bucket);
                        }
                        render_window(&mut lines, window, bar_width, show_headers);
                        lines.push(Line::from(""));
                    }
                }
            }
            ProviderStatus::Unavailable { info } => {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::raw("Unavailable").yellow().bold(),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::raw(info.reason.clone()),
                ]));
                if let Some(url) = &info.console_url {
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::raw(url.clone()).underlined().dim(),
                    ]));
                }
            }
            ProviderStatus::AuthRequired => {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::raw("Authentication required").red().bold(),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::raw("Set an API key via env var, config, or log in to the native CLI.")
                        .dim(),
                ]));
            }
            ProviderStatus::NetworkError { message } => {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::raw("Network error").red().bold(),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::raw(message.clone()),
                ]));
            }
        }

        // Raw JSON section at bottom
        if let Some(raw) = &self.result.raw_response {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::raw("── raw response ──").dim(),
            ]));
            let pretty = serde_json::to_string_pretty(raw).unwrap_or_default();
            for raw_line in pretty.lines() {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::raw(raw_line.to_string()).dim(),
                ]));
            }
        }

        Text::from(lines)
    }
}

fn render_window(lines: &mut Vec<Line<'_>>, w: &QuotaWindow, bar_width: u16, show_headers: bool) {
    let label_src = bar::display_label(&w.window_type, show_headers);
    // Special-case currency balance rows: no bar, just the formatted amount.
    if let Some((sym, scale)) = bar::currency_window(&w.window_type) {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(format!("{:<14} ", bar::truncate_suffix(&label_src, 14))),
            Span::raw(format!("{}{:.2}", sym, w.remaining as f64 / scale)).bold(),
        ]));
        return;
    }

    let used = w.used.max(0);
    let limit = w.limit.max(1);
    let used_pct = (used as f64 / limit as f64).clamp(0.0, 1.0);
    let color = bar::bar_color(used_pct);
    let time_elapsed = bar::time_elapsed_fraction(w);
    let bar_spans = bar::build(used_pct, time_elapsed, bar_width, color);

    // Row 1: name + bar + % used
    let mut l1 = vec![
        Span::raw("  "),
        Span::raw(format!("{:<14} ", bar::truncate_suffix(&label_src, 14))),
    ];
    l1.extend(bar_spans);
    l1.push(Span::raw(" "));
    l1.push(
        Span::raw(format!("{:>3.0}% used", used_pct * 100.0))
            .bold()
            .fg(color),
    );
    lines.push(Line::from(l1));

    // Row 2: exact numbers
    lines.push(Line::from(vec![
        Span::raw("                 "),
        Span::raw(format!(
            "{} used · {} left · {} cap",
            fmt_exact(used),
            fmt_exact(w.remaining),
            fmt_exact(w.limit)
        ))
        .dim(),
    ]));

    // Row 3: reset info (if known)
    if let Some(reset) = w.reset_at {
        let rel = humanize_duration(reset - Utc::now());
        let abs = reset.format("%Y-%m-%d %H:%M UTC").to_string();
        lines.push(Line::from(vec![
            Span::raw("                 "),
            Span::raw(format!("resets in {} · {}", rel, abs)).dim(),
        ]));
    }

    // Row 4: pace commentary — compare used vs time-elapsed.
    if let Some(time_elapsed) = time_elapsed {
        let diff = used_pct - time_elapsed;
        let (label, style) = if diff <= -0.05 {
            (
                format!(
                    "pacing ahead — {:.0}% used vs {:.0}% time elapsed",
                    used_pct * 100.0,
                    time_elapsed * 100.0
                ),
                Style::new().green(),
            )
        } else if diff >= 0.05 {
            (
                format!(
                    "burning fast — {:.0}% used vs {:.0}% time elapsed",
                    used_pct * 100.0,
                    time_elapsed * 100.0
                ),
                Style::new().fg(Color::Rgb(255, 140, 0)),
            )
        } else {
            (
                format!(
                    "on pace — {:.0}% used vs {:.0}% time elapsed",
                    used_pct * 100.0,
                    time_elapsed * 100.0
                ),
                Style::new().dim(),
            )
        };
        lines.push(Line::from(vec![
            Span::raw("                 "),
            Span::styled(label, style),
        ]));
    }
}

fn freshness_span(result: &ProviderResult) -> Span<'static> {
    let age = (Utc::now() - result.fetched_at).num_seconds().max(0);
    let label = FreshnessLabel::with_interval(age, result.kind.auto_refresh_secs());
    let style = match label.staleness {
        Staleness::Fresh => Style::new().cyan(),
        Staleness::Warning => Style::new().yellow(),
        Staleness::Stale => Style::new().red(),
    };
    Span::styled(label.label, style)
}

fn fmt_exact(n: i64) -> String {
    // Group thousands for readability, e.g. 148177 → 148,177.
    let neg = n < 0;
    let mut digits: Vec<char> = n.abs().to_string().chars().collect();
    let mut out = String::new();
    while digits.len() > 3 {
        let tail: String = digits.drain(digits.len() - 3..).collect();
        out = format!(",{}{}", tail, out);
    }
    let head: String = digits.into_iter().collect();
    let joined = format!("{}{}", head, out);
    if neg {
        format!("-{}", joined)
    } else {
        joined
    }
}

/// Format an auth source string for human display.
/// Inputs follow the resolver conventions: "env:VAR_NAME", "file:/path",
/// "oauth:/path", "opencode:slot-key".
fn pretty_auth_source(source: &str) -> String {
    if let Some(var) = source.strip_prefix("env:") {
        format!("auth: env ${}", var)
    } else if let Some(path) = source.strip_prefix("oauth:") {
        format!("auth: oauth  {}", path)
    } else if let Some(path) = source.strip_prefix("file:") {
        format!("auth: file   {}", path)
    } else {
        format!("auth: {}", source)
    }
}

fn humanize_duration(d: chrono::Duration) -> String {
    let secs = d.num_seconds();
    if secs <= 0 {
        return "now".to_string();
    }
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_exact_groups_thousands() {
        assert_eq!(fmt_exact(0), "0");
        assert_eq!(fmt_exact(999), "999");
        assert_eq!(fmt_exact(1_000), "1,000");
        assert_eq!(fmt_exact(148_177), "148,177");
        assert_eq!(fmt_exact(-1_234_567), "-1,234,567");
    }

    #[test]
    fn humanize_duration_ranges() {
        assert_eq!(humanize_duration(chrono::Duration::zero()), "now");
        assert_eq!(humanize_duration(chrono::Duration::minutes(5)), "5m");
        assert_eq!(
            humanize_duration(chrono::Duration::seconds(3600 * 2 + 300)),
            "2h 5m"
        );
        assert_eq!(
            humanize_duration(chrono::Duration::seconds(86400 * 3 + 3600)),
            "3d 1h"
        );
    }
}
