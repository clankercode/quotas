use crate::providers::{ProviderResult, ProviderStatus, QuotaWindow};
use crate::tui::freshness::{FreshnessLabel, Staleness};
use chrono::Utc;
use ratatui::prelude::*;

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

        // Header
        lines.push(Line::from(vec![Span::raw(" ")]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(self.result.kind.display_name())
                .bold()
                .fg(Color::White),
            Span::raw("   "),
            freshness_span(&self.result),
        ]));

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
                    for window in &quota.windows {
                        render_window(&mut lines, window, bar_width);
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

fn render_window(lines: &mut Vec<Line<'_>>, w: &QuotaWindow, bar_width: u16) {
    // Special-case the payg balance row: no bar, just a dollar figure.
    if w.window_type == "payg_balance" {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(format!("{:<14} ", w.window_type)),
            Span::raw(format!("${:.2}", w.remaining as f64 / 100.0)).bold(),
        ]));
        return;
    }

    let used = w.used.max(0);
    let limit = w.limit.max(1);
    let used_pct = (used as f64 / limit as f64).clamp(0.0, 1.0);
    let remaining_pct = 1.0 - used_pct;
    let color = if remaining_pct <= 0.10 {
        Color::Red
    } else if remaining_pct <= 0.25 {
        Color::Yellow
    } else {
        Color::Green
    };
    let time_frac = time_remaining_fraction(w);
    let bar_spans = marked_big_bar(remaining_pct, time_frac, bar_width, color);

    // Row 1: name + bar + % left
    let mut l1 = vec![
        Span::raw("  "),
        Span::raw(format!("{:<14} ", truncate(&w.window_type, 14))),
    ];
    l1.extend(bar_spans);
    l1.push(Span::raw(" "));
    l1.push(
        Span::raw(format!("{:>3.0}% left", remaining_pct * 100.0))
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

    // Row 4: pace commentary — compare quota-remaining to time-remaining.
    if let Some(time_frac) = time_frac {
        let diff = remaining_pct - time_frac;
        let (label, style) = if diff >= 0.05 {
            (
                format!(
                    "pacing ahead — {:.0}% quota vs {:.0}% time left",
                    remaining_pct * 100.0,
                    time_frac * 100.0
                ),
                Style::new().green(),
            )
        } else if diff <= -0.05 {
            (
                format!(
                    "burning fast — {:.0}% quota vs {:.0}% time left",
                    remaining_pct * 100.0,
                    time_frac * 100.0
                ),
                Style::new().yellow(),
            )
        } else {
            (
                format!(
                    "on pace — {:.0}% quota vs {:.0}% time left",
                    remaining_pct * 100.0,
                    time_frac * 100.0
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

fn time_remaining_fraction(w: &QuotaWindow) -> Option<f64> {
    let reset = w.reset_at?;
    let period = w.period_seconds?;
    if period <= 0 {
        return None;
    }
    let remaining = (reset - Utc::now()).num_seconds();
    if remaining <= 0 {
        return Some(0.0);
    }
    Some((remaining as f64 / period as f64).clamp(0.0, 1.0))
}

fn marked_big_bar<'a>(
    remaining_pct: f64,
    time_remaining_pct: Option<f64>,
    width: u16,
    color: Color,
) -> Vec<Span<'a>> {
    let w = width as usize;
    if w == 0 {
        return Vec::new();
    }
    let filled = ((remaining_pct.clamp(0.0, 1.0)) * w as f64).round() as usize;
    let marker_pos = time_remaining_pct
        .map(|t| (((t.clamp(0.0, 1.0)) * w as f64).round() as usize).min(w.saturating_sub(1)));

    let bar_style = Style::new().fg(color);
    let marker_style = Style::new().fg(Color::White).bold();

    let mut out: Vec<Span<'a>> = Vec::new();
    let mut buf = String::new();
    let mut in_marker = false;

    for i in 0..w {
        let is_marker = marker_pos == Some(i);
        if is_marker != in_marker && !buf.is_empty() {
            out.push(Span::styled(
                std::mem::take(&mut buf),
                if in_marker { marker_style } else { bar_style },
            ));
            in_marker = is_marker;
        }
        if is_marker {
            buf.push('┃');
        } else if i < filled {
            buf.push('█');
        } else {
            buf.push('░');
        }
    }
    if !buf.is_empty() {
        out.push(Span::styled(
            buf,
            if in_marker { marker_style } else { bar_style },
        ));
    }
    out
}

fn freshness_span<'a>(result: &'a ProviderResult) -> Span<'a> {
    let age = (Utc::now() - result.fetched_at).num_seconds();
    let label = FreshnessLabel::new(age);
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

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
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
