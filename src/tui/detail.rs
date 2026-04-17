use crate::providers::{ProviderResult, ProviderStatus, QuotaWindow};
use crate::tui::bar;
use crate::tui::freshness::FreshnessLabel;
use chrono::Utc;
use ratatui::prelude::*;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailMode {
    Auto,
    Normal,
    Compact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolvedDetailMode {
    Normal,
    Compact,
}

pub struct DetailView {
    pub result: ProviderResult,
}

impl DetailView {
    pub fn new(result: ProviderResult) -> Self {
        Self { result }
    }

    pub fn render(
        &self,
        width: u16,
        height: u16,
        mode: DetailMode,
        auto_refresh: bool,
    ) -> Text<'_> {
        let mut lines: Vec<Line> = Vec::new();
        let pct_w: u16 = 10; // room for " 100% used"
        let indent: u16 = 2;
        let gap: u16 = 1; // space between label and bar
        let resolved_mode = resolve_mode(mode, width, height, &self.result);

        // These are refined once we know the actual labels in the provider.
        let label_w: usize;
        let bar_width: u16;

        // Header — freshness only when we have valid auth data.
        let show_freshness = !matches!(self.result.status, ProviderStatus::AuthRequired);
        lines.push(Line::from(vec![Span::raw(" ")]));
        lines.push(render_header_line(
            &self.result,
            width,
            show_freshness,
            auto_refresh,
        ));

        // Auth source line (env var name, file path, oauth path, etc.)
        if let Some(source) = &self.result.auth_source {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::raw(pretty_auth_source(source)).dim(),
            ]));
        }

        match &self.result.status {
            ProviderStatus::Available { quota } => {
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

                    // Compute label width from actual labels so we don't waste space.
                    label_w = sorted
                        .iter()
                        .map(|w| {
                            bar::display_label(&w.window_type, show_headers)
                                .chars()
                                .count()
                        })
                        .max()
                        .unwrap_or(12)
                        .clamp(8, 20);
                    bar_width = width
                        .saturating_sub(indent + label_w as u16 + gap + pct_w)
                        .clamp(10, width.saturating_sub(indent + gap + pct_w));

                    let subrow_indent = " ".repeat((indent + gap + label_w as u16) as usize);

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
                        render_window(
                            &mut lines,
                            window,
                            bar_width,
                            label_w,
                            show_headers,
                            subrow_indent.clone(),
                            resolved_mode,
                        );
                        if resolved_mode == ResolvedDetailMode::Normal {
                            lines.push(Line::from(""));
                        }
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

fn render_window(
    lines: &mut Vec<Line<'_>>,
    w: &QuotaWindow,
    bar_width: u16,
    label_w: usize,
    show_headers: bool,
    subrow_indent: String,
    mode: ResolvedDetailMode,
) {
    let label_src = bar::display_label(&w.window_type, show_headers);
    // Special-case currency balance rows: no bar, just the formatted amount.
    if let Some((sym, scale)) = bar::currency_window(&w.window_type) {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(format!(
                "{:<width$} ",
                bar::truncate_suffix(&label_src, label_w),
                width = label_w
            )),
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

    if mode == ResolvedDetailMode::Compact {
        let compact_label_w = label_w.min(14);
        let mut compact = vec![
            Span::raw("  "),
            Span::raw(format!(
                "{:<width$} ",
                bar::truncate_suffix(&label_src, compact_label_w),
                width = compact_label_w
            )),
        ];
        compact.extend(bar_spans);
        compact.push(Span::raw(" "));
        compact.push(
            Span::raw(format!("{:>3.0}% ", used_pct * 100.0))
                .bold()
                .fg(color),
        );
        compact.push(Span::raw(format!("{}L", fmt_exact(w.remaining))).dim());
        lines.push(Line::from(compact));

        if let Some(reset) = w.reset_at {
            let rel = humanize_duration(reset - Utc::now());
            lines.push(Line::from(vec![
                Span::raw(subrow_indent),
                Span::raw(format!("resets in {}", rel)).dim(),
            ]));
        }
        return;
    }

    // Row 1: name + bar + % used
    let mut l1 = vec![
        Span::raw("  "),
        Span::raw(format!(
            "{:<width$} ",
            bar::truncate_suffix(&label_src, label_w),
            width = label_w
        )),
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
        Span::raw(subrow_indent.clone()),
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
            Span::raw(subrow_indent.clone()),
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
            Span::raw(subrow_indent),
            Span::styled(label, style),
        ]));
    }
}

fn freshness_label(result: &ProviderResult) -> FreshnessLabel {
    if let Some(cached_at) = result.cached_at {
        let age = (Utc::now() - cached_at).num_seconds().max(0);
        FreshnessLabel::cached(age)
    } else {
        let age = (Utc::now() - result.fetched_at).num_seconds().max(0);
        FreshnessLabel::with_interval(age, result.kind.auto_refresh_secs())
    }
}

fn render_header_line(
    result: &ProviderResult,
    width: u16,
    show_freshness: bool,
    auto_refresh: bool,
) -> Line<'static> {
    let provider = result.kind.display_name();
    let plan = match &result.status {
        ProviderStatus::Available { quota } => quota.plan_name.clone(),
        _ => String::new(),
    };
    let freshness = if show_freshness {
        let label = freshness_label(result);
        if auto_refresh && !label.is_cached {
            format!("{} {}", label.label, refresh_meter(label.fraction, 8))
        } else {
            label.label
        }
    } else {
        String::new()
    };

    let total_width = width.saturating_sub(2) as usize;
    let freshness_width = freshness.chars().count();
    let available = total_width.saturating_sub(freshness_width + usize::from(!freshness.is_empty()));
    let provider_width = available.min(18).max(available.min(provider.chars().count().max(1)));
    let plan_width = available.saturating_sub(provider_width + usize::from(!plan.is_empty()));
    let mut row = format!(
        "  {:<provider_width$}",
        truncate_text(provider, provider_width.max(1)),
        provider_width = provider_width
    );
    if !plan.is_empty() && plan_width > 0 {
        row.push(' ');
        row.push_str(&format!(
            "{:>plan_width$}",
            truncate_text(&plan, plan_width),
            plan_width = plan_width
        ));
    }
    if !freshness.is_empty() {
        let current_width = row.chars().count();
        let freshness_start = total_width.saturating_sub(freshness_width);
        if current_width < freshness_start {
            row.push_str(&" ".repeat(freshness_start - current_width));
        } else {
            row.push(' ');
        }
        row.push_str(&freshness);
    }
    Line::from(vec![Span::raw(row)])
}

fn refresh_meter(fraction: f64, width: usize) -> String {
    let filled = ((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let mut out = String::with_capacity(width + 2);
    out.push('[');
    for idx in 0..width {
        out.push(if idx < filled { '=' } else { '-' });
    }
    out.push(']');
    out
}

fn truncate_text(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut out: String = chars.into_iter().take(width - 1).collect();
    out.push('…');
    out
}

fn resolve_mode(
    mode: DetailMode,
    width: u16,
    height: u16,
    result: &ProviderResult,
) -> ResolvedDetailMode {
    match mode {
        DetailMode::Normal => ResolvedDetailMode::Normal,
        DetailMode::Compact => ResolvedDetailMode::Compact,
        DetailMode::Auto => {
            let window_count = match &result.status {
                ProviderStatus::Available { quota } => quota.windows.len(),
                _ => 0,
            };
            if width < 96 || height < 16 || (height < 20 && window_count >= 2) {
                ResolvedDetailMode::Compact
            } else {
                ResolvedDetailMode::Normal
            }
        }
    }
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
    use crate::providers::{ProviderKind, ProviderQuota, QuotaWindow};

    fn render_detail_text(result: ProviderResult, width: u16, height: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Paragraph;
        use ratatui::Terminal;

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = DetailView::new(result);
        terminal
            .draw(|f| {
                let text = view.render(width, height, DetailMode::Auto, true);
                f.render_widget(Paragraph::new(text), f.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let mut lines = Vec::new();
        for y in 0..height {
            let mut line = String::new();
            for x in 0..width {
                line.push_str(buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    fn gemini_fraction_quota_result() -> ProviderResult {
        ProviderResult {
            kind: ProviderKind::Gemini,
            status: ProviderStatus::Available {
                quota: ProviderQuota {
                    plan_name: "Gemini API".into(),
                    windows: vec![QuotaWindow {
                        window_type: "REQUESTS_gemini-2.5-flash".into(),
                        used: 4,
                        limit: 100,
                        remaining: 96,
                        reset_at: Some(
                            chrono::DateTime::parse_from_rfc3339("2026-04-18T04:00:00Z")
                                .unwrap()
                                .with_timezone(&chrono::Utc),
                        ),
                        period_seconds: None,
                    }],
                    unlimited: false,
                },
            },
            fetched_at: chrono::Utc::now(),
            raw_response: None,
            auth_source: Some("oauth:/home/xertrov/.gemini/oauth_creds.json".into()),
            cached_at: None,
        }
    }

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

    #[test]
    fn renders_gemini_fraction_quota_with_exact_counts() {
        let out = render_detail_text(gemini_fraction_quota_result(), 100, 18);

        assert!(out.contains("Gemini API"));
        assert!(out.contains("4% used"));
        assert!(out.contains("4 used"));
        assert!(out.contains("96 left"));
        assert!(out.contains("100 cap"));
        assert!(out.contains("resets in"));
    }

    #[test]
    fn moves_plan_into_header_above_the_fold() {
        let out = render_detail_text(gemini_fraction_quota_result(), 80, 18);
        let lines: Vec<&str> = out.lines().collect();

        assert!(lines.get(1).is_some_and(|line| line.contains("Gemini API")));
        assert!(lines.get(1).is_some_and(|line| line.contains("Updated")));
        assert!(lines.get(1).is_some_and(|line| line.contains("Gemini API")));
        assert!(lines.get(2).is_some_and(|line| line.contains("auth: oauth")));
        assert!(
            lines
                .iter()
                .take(4)
                .any(|line| line.contains("Gemini API")),
            "expected plan name near the header"
        );
    }

    #[test]
    fn compact_layout_kicks_in_for_short_detail_view() {
        let out = render_detail_text(gemini_fraction_quota_result(), 80, 10);

        assert!(out.contains("Gemini API"));
        assert!(out.contains("Updated"));
        assert!(!out.contains("4 used · 96 left · 100 cap"));
    }
}
