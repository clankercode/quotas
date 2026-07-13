use crate::config::QuotaPreferences;
use crate::providers::{BankedResets, ProviderResult, ProviderStatus, QuotaWindow};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailRowKey {
    pub quota_key: String,
    pub hidden: bool,
}

pub struct DetailRenderOptions<'a> {
    pub width: u16,
    pub height: u16,
    pub mode: DetailMode,
    pub auto_refresh: bool,
    pub provider_favorite: bool,
    pub preferences: &'a QuotaPreferences,
    pub focused_row: Option<usize>,
}

struct WindowRenderOptions {
    bar_width: u16,
    label_w: usize,
    show_headers: bool,
    subrow_indent: String,
    mode: ResolvedDetailMode,
    favorite: bool,
    focused: bool,
}

impl DetailView {
    pub fn new(result: ProviderResult) -> Self {
        Self { result }
    }

    pub fn render(&self, options: DetailRenderOptions<'_>) -> Text<'_> {
        let mut lines: Vec<Line> = Vec::new();
        let pct_w: u16 = 10; // room for " 100% used"
        let indent: u16 = 2;
        let gap: u16 = 1; // space between label and bar
        let resolved_mode = resolve_mode(options.mode, options.width, options.height, &self.result);

        // These are refined once we know the actual labels in the provider.
        let label_w: usize;
        let bar_width: u16;

        // Header — freshness only when we have valid auth data.
        let show_freshness = !matches!(self.result.status, ProviderStatus::AuthRequired);
        lines.push(Line::from(vec![Span::raw(" ")]));
        lines.push(render_header_line(
            &self.result,
            options.width,
            show_freshness,
            options.auto_refresh,
            options.provider_favorite,
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
                    let (visible_windows, hidden_windows) =
                        partition_windows(&quota.windows, options.preferences, false);
                    let buckets_seen: BTreeSet<u8> = visible_windows
                        .iter()
                        .map(|w| bar::window_sort_key(w).0)
                        .collect();
                    let show_headers = visible_windows.len() >= 3 && buckets_seen.len() >= 2;

                    // Compute label width from actual labels so we don't waste space.
                    label_w = visible_windows
                        .iter()
                        .map(|w| {
                            bar::display_label(&w.window_type, show_headers)
                                .chars()
                                .count()
                        })
                        .max()
                        .unwrap_or(12)
                        .clamp(8, 20);
                    bar_width = options
                        .width
                        .saturating_sub(indent + label_w as u16 + gap + pct_w)
                        .clamp(10, options.width.saturating_sub(indent + gap + pct_w));

                    let subrow_indent = " ".repeat((indent + gap + label_w as u16) as usize);

                    let mut last_bucket: Option<u8> = None;
                    for (visible_idx, window) in visible_windows.iter().enumerate() {
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
                            WindowRenderOptions {
                                bar_width,
                                label_w,
                                show_headers,
                                subrow_indent: subrow_indent.clone(),
                                mode: resolved_mode,
                                favorite: options.preferences.favorites.iter().any(|favorite| {
                                    favorite.eq_ignore_ascii_case(&window.window_type)
                                }),
                                focused: options.focused_row == Some(visible_idx),
                            },
                        );
                        if resolved_mode == ResolvedDetailMode::Normal {
                            lines.push(Line::from(""));
                        }
                    }
                    for (hidden_idx, window) in hidden_windows.iter().enumerate() {
                        let row_idx = visible_windows.len() + hidden_idx;
                        lines.push(render_hidden_row(
                            window,
                            options.focused_row == Some(row_idx),
                        ));
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

        // Banked resets sit below window rows (and still render when windows
        // are empty / unlimited), but only while any remain available.
        if let ProviderStatus::Available { quota } = &self.result.status {
            if let Some(banked) = &quota.banked_resets {
                if banked.available_count > 0 {
                    render_banked_resets_section(&mut lines, banked);
                }
            }
        }

        // Raw JSON section at bottom. Multi-endpoint providers (Codex with
        // banked-reset detail, Cursor plan+usage, Grok, OpenRouter, …) store
        // an envelope of objects; render main first, then each extra part.
        if let Some(raw) = &self.result.raw_response {
            render_raw_response_section(&mut lines, raw);
        }

        Text::from(lines)
    }
}

/// True when `raw` looks like a multi-endpoint envelope: two or more top-level
/// keys whose values are only objects, arrays, or null (not mixed scalars like
/// a real single API body with `plan_type: "pro"`).
fn is_multipart_raw_envelope(raw: &serde_json::Value) -> bool {
    let Some(obj) = raw.as_object() else {
        return false;
    };
    if obj.len() < 2 {
        return false;
    }
    obj.values()
        .all(|v| v.is_object() || v.is_array() || v.is_null())
}

/// Preferred order for primary parts of multi-endpoint raw envelopes.
/// serde_json `Map` may iterate alphabetically; we always put known main
/// payloads first so extras (e.g. Codex `rate_limit_reset_credits`) follow.
const RAW_PRIMARY_KEYS: &[&str] = &[
    "usage",      // Codex (main wham/usage)
    "plan",       // Cursor plan info (before usage)
    "default",    // Grok CLI /v1/billing
    "key",        // OpenRouter key endpoint
    "validation", // Grok management key validation
    "balance",    // Grok management prepaid balance
];

/// Parts in display order: known primary keys first (in `RAW_PRIMARY_KEYS`
/// order), then any remaining keys sorted for stable output.
fn multipart_parts_display_order(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Vec<(&str, &serde_json::Value)> {
    let mut out = Vec::with_capacity(obj.len());
    let mut seen = std::collections::HashSet::new();
    for key in RAW_PRIMARY_KEYS {
        if let Some(v) = obj.get(*key) {
            out.push((*key, v));
            seen.insert(*key);
        }
    }
    let mut rest: Vec<_> = obj
        .iter()
        .filter(|(k, _)| !seen.contains(k.as_str()))
        .map(|(k, v)| (k.as_str(), v))
        .collect();
    rest.sort_by(|a, b| a.0.cmp(b.0));
    out.extend(rest);
    out
}

fn push_pretty_json_lines(lines: &mut Vec<Line<'_>>, value: &serde_json::Value) {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
    for raw_line in pretty.lines() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(raw_line.to_string()).dim(),
        ]));
    }
}

fn render_raw_response_section(lines: &mut Vec<Line<'_>>, raw: &serde_json::Value) {
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::raw("── raw response ──").dim(),
    ]));
    if is_multipart_raw_envelope(raw) {
        let obj = raw.as_object().expect("checked above");
        for (key, part) in multipart_parts_display_order(obj) {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::raw(format!("── {key} ──")).dim(),
            ]));
            push_pretty_json_lines(lines, part);
        }
    } else {
        push_pretty_json_lines(lines, raw);
    }
}

fn render_window(lines: &mut Vec<Line<'_>>, w: &QuotaWindow, options: WindowRenderOptions) {
    let label_src = bar::display_label(&w.window_type, options.show_headers);
    let row_prefix = if options.focused { "› " } else { "  " };
    let marker = if options.favorite { "★ " } else { "" };
    // Special-case currency balance rows: no bar, just the formatted amount.
    if let Some((sym, scale)) = bar::currency_window(&w.window_type) {
        lines.push(Line::from(vec![
            Span::raw(row_prefix),
            Span::raw(marker),
            Span::raw(format!(
                "{:<width$} ",
                bar::truncate_suffix(&label_src, options.label_w),
                width = options.label_w
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
    let bar_spans = bar::build(used_pct, time_elapsed, options.bar_width, color);
    // Currency allowances (Grok monthly $) keep the pacing bar but render
    // their used/limit/remaining numbers as dollar amounts.
    let currency = bar::currency_bar_scale(&w.window_type);

    if options.mode == ResolvedDetailMode::Compact {
        let compact_label_w = options.label_w.min(14);
        let mut compact = vec![
            Span::raw(row_prefix),
            Span::raw(marker),
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
        let remaining_txt = match currency {
            Some((sym, scale)) => format!("{}{:.2}L", sym, w.remaining as f64 / scale),
            None => format!("{}L", fmt_exact(w.remaining)),
        };
        compact.push(Span::raw(remaining_txt).dim());
        lines.push(Line::from(compact));

        if let Some(reset) = w.reset_at {
            let rel = humanize_duration(reset - Utc::now());
            lines.push(Line::from(vec![
                Span::raw(options.subrow_indent),
                Span::raw(format!("resets in {}", rel)).dim(),
            ]));
        }
        return;
    }

    // Row 1: name + bar + % used
    let mut l1 = vec![
        Span::raw(row_prefix),
        Span::raw(marker),
        Span::raw(format!(
            "{:<width$} ",
            bar::truncate_suffix(&label_src, options.label_w),
            width = options.label_w
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
    let numbers = match currency {
        Some((sym, scale)) => format!(
            "{sym}{:.2} used · {sym}{:.2} left · {sym}{:.2} cap",
            used as f64 / scale,
            w.remaining as f64 / scale,
            w.limit as f64 / scale,
        ),
        None => format!(
            "{} used · {} left · {} cap",
            fmt_exact(used),
            fmt_exact(w.remaining),
            fmt_exact(w.limit)
        ),
    };
    lines.push(Line::from(vec![
        Span::raw(options.subrow_indent.clone()),
        Span::raw(numbers).dim(),
    ]));

    // Row 3: reset info (if known)
    if let Some(reset) = w.reset_at {
        let rel = humanize_duration(reset - Utc::now());
        let abs = reset.format("%Y-%m-%d %H:%M UTC").to_string();
        lines.push(Line::from(vec![
            Span::raw(options.subrow_indent.clone()),
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
            Span::raw(options.subrow_indent),
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
    provider_favorite: bool,
) -> Line<'static> {
    let provider = if provider_favorite {
        format!("★ {}", result.kind.display_name())
    } else {
        result.kind.display_name().to_string()
    };
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
    let available =
        total_width.saturating_sub(freshness_width + usize::from(!freshness.is_empty()));
    let provider_width = available
        .min(18)
        .max(available.min(provider.chars().count().max(1)));
    let plan_width = available.saturating_sub(provider_width + usize::from(!plan.is_empty()));
    let mut row = format!(
        "  {:<provider_width$}",
        truncate_text(&provider, provider_width.max(1)),
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

fn render_hidden_row(window: &QuotaWindow, focused: bool) -> Line<'static> {
    let prefix = if focused { "› " } else { "  " };
    let label = bar::display_label(&window.window_type, false);
    Line::from(vec![
        Span::raw(prefix),
        Span::raw(format!("[x] hidden quota '{}' (press x to show)", label)).dim(),
    ])
}

/// Banked / earned rate-limit reset credits (Codex): count + per-credit detail.
fn render_banked_resets_section(lines: &mut Vec<Line<'_>>, banked: &BankedResets) {
    lines.push(Line::from(""));
    let n = banked.available_count;
    let header = if n == 1 {
        "── banked resets (1 available) ──".to_string()
    } else {
        format!("── banked resets ({n} available) ──")
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::raw(header).cyan().dim(),
    ]));

    if banked.credits.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw("Redeemable full usage resets (no per-credit detail).").dim(),
        ]));
        return;
    }

    let now = Utc::now();
    for credit in &banked.credits {
        let title = credit
            .title
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("Reset");
        let mut main = title.to_string();
        if let Some(exp) = credit.expires_at {
            let secs = (exp - now).num_seconds();
            let exp_label = if secs <= 0 {
                "expired".to_string()
            } else if secs < 3600 {
                format!("expires in {}m", (secs + 59) / 60)
            } else if secs < 86400 {
                format!("expires in {}h", (secs + 3599) / 3600)
            } else {
                format!("expires in {}d", (secs + 86399) / 86400)
            };
            main.push_str(" · ");
            main.push_str(&exp_label);
        }
        if let Some(src) = credit.source.as_deref().filter(|s| !s.is_empty()) {
            main.push_str(" · ");
            main.push_str(src);
        }
        let status = credit.status.to_ascii_lowercase();
        let title_span = if status == "available" {
            Span::raw(main).cyan()
        } else {
            Span::raw(format!("{main} [{status}]")).dim()
        };
        lines.push(Line::from(vec![Span::raw("  "), title_span]));
        if let Some(desc) = credit.description.as_deref().filter(|s| !s.is_empty()) {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::raw(desc.to_string()).dim(),
            ]));
        }
    }
}

fn partition_windows<'a>(
    windows: &'a [QuotaWindow],
    preferences: &QuotaPreferences,
    show_all_windows: bool,
) -> (Vec<&'a QuotaWindow>, Vec<&'a QuotaWindow>) {
    let mut visible = Vec::new();
    let mut hidden = Vec::new();
    for window in windows {
        let hidden_by_pref = preferences
            .hidden
            .iter()
            .any(|hidden_key| hidden_key.eq_ignore_ascii_case(&window.window_type));
        let hidden_by_ui = !show_all_windows
            && bar::autohide_window(&window.window_type)
            && window.limit <= 0
            && bar::currency_window(&window.window_type).is_none();
        if hidden_by_pref || hidden_by_ui {
            hidden.push(window);
        } else {
            visible.push(window);
        }
    }
    visible.sort_by_key(|window| {
        (
            !preferences
                .favorites
                .iter()
                .any(|favorite| favorite.eq_ignore_ascii_case(&window.window_type)),
            bar::window_sort_key(window),
        )
    });
    hidden.sort_by_key(|window| bar::window_sort_key(window));
    (visible, hidden)
}

pub fn detail_row_keys(
    result: &ProviderResult,
    show_all_windows: bool,
    preferences: &QuotaPreferences,
) -> Vec<DetailRowKey> {
    match &result.status {
        ProviderStatus::Available { quota } => {
            let (visible, hidden) =
                partition_windows(&quota.windows, preferences, show_all_windows);
            visible
                .into_iter()
                .map(|window| DetailRowKey {
                    quota_key: window.window_type.clone(),
                    hidden: false,
                })
                .chain(hidden.into_iter().map(|window| DetailRowKey {
                    quota_key: window.window_type.clone(),
                    hidden: true,
                }))
                .collect()
        }
        _ => Vec::new(),
    }
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
    use crate::providers::{
        BankedResetCredit, BankedResets, ProviderKind, ProviderQuota, QuotaWindow,
    };

    fn render_detail_text(result: ProviderResult, width: u16, height: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Paragraph;
        use ratatui::Terminal;

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = DetailView::new(result);
        terminal
            .draw(|f| {
                let text = view.render(DetailRenderOptions {
                    width,
                    height,
                    mode: DetailMode::Auto,
                    auto_refresh: true,
                    provider_favorite: false,
                    preferences: &QuotaPreferences::default(),
                    focused_row: None,
                });
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

    fn antigravity_summary_quota_result() -> ProviderResult {
        ProviderResult {
            kind: ProviderKind::Antigravity,
            status: ProviderStatus::Available {
                quota: ProviderQuota {
                    plan_name: "Antigravity".into(),
                    windows: vec![
                        QuotaWindow {
                            window_type: "7d/gemini".into(),
                            used: 18,
                            limit: 100,
                            remaining: 82,
                            reset_at: Some(
                                chrono::DateTime::parse_from_rfc3339("2026-07-19T17:07:49Z")
                                    .unwrap()
                                    .with_timezone(&chrono::Utc),
                            ),
                            period_seconds: Some(7 * 86400),
                        },
                        QuotaWindow {
                            window_type: "5h/gemini".into(),
                            used: 2,
                            limit: 100,
                            remaining: 98,
                            reset_at: Some(
                                chrono::DateTime::parse_from_rfc3339("2026-07-13T18:32:16Z")
                                    .unwrap()
                                    .with_timezone(&chrono::Utc),
                            ),
                            period_seconds: Some(5 * 3600),
                        },
                    ],
                    unlimited: false,
                    banked_resets: None,
                },
            },
            fetched_at: chrono::Utc::now(),
            raw_response: None,
            auth_source: Some(
                "oauth:/home/xertrov/.gemini/antigravity-cli/antigravity-oauth-token".into(),
            ),
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
    fn renders_grok_monthly_numbers_as_dollars() {
        // Grok monthly $ allowance stores USD in ×10000 units. The pacing
        // bar stays, but row-2 numbers must read as dollars, not raw counts.
        let result = ProviderResult {
            kind: ProviderKind::Grok,
            status: ProviderStatus::Available {
                quota: ProviderQuota {
                    plan_name: "Grok Build".into(),
                    windows: vec![QuotaWindow {
                        window_type: "monthly_allowance".into(),
                        used: 293_100,     // $29.31
                        limit: 1_500_000,  // $150.00
                        remaining: 1_206_900, // $120.69
                        reset_at: Some(
                            chrono::DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
                                .unwrap()
                                .with_timezone(&chrono::Utc),
                        ),
                        period_seconds: Some(31 * 24 * 3600),
                    }],
                    unlimited: false,
                    banked_resets: None,
                },
            },
            fetched_at: chrono::Utc::now(),
            raw_response: None,
            auth_source: None,
            cached_at: None,
        };
        let out = render_detail_text(result, 100, 18);
        assert!(out.contains("$29.31 used"), "out:\n{out}");
        assert!(out.contains("$120.69 left"), "out:\n{out}");
        assert!(out.contains("$150.00 cap"), "out:\n{out}");
        // Must NOT show raw ×10000 counts.
        assert!(!out.contains("293,100"), "out:\n{out}");
    }

    #[test]
    fn renders_bare_monthly_as_plain_counts_not_dollars() {
        // Regression: a GitHub Copilot–style `monthly` window carries COUNTS
        // (premium requests), not USD. It must render as plain counts, never
        // dollars — only the distinct `monthly_allowance` window is currency.
        let result = ProviderResult {
            kind: ProviderKind::GitHubCopilot,
            status: ProviderStatus::Available {
                quota: ProviderQuota {
                    plan_name: "Copilot".into(),
                    windows: vec![QuotaWindow {
                        window_type: "monthly".into(),
                        used: 50,
                        limit: 300,
                        remaining: 250,
                        reset_at: None,
                        period_seconds: None,
                    }],
                    unlimited: false,
                    banked_resets: None,
                },
            },
            fetched_at: chrono::Utc::now(),
            raw_response: None,
            auth_source: None,
            cached_at: None,
        };
        let out = render_detail_text(result, 100, 18);
        assert!(out.contains("50 used"), "out:\n{out}");
        assert!(out.contains("250 left"), "out:\n{out}");
        assert!(out.contains("300 cap"), "out:\n{out}");
        assert!(!out.contains('$'), "out:\n{out}");
    }

    fn codex_with_banked_resets() -> ProviderResult {
        ProviderResult {
            kind: ProviderKind::Codex,
            status: ProviderStatus::Available {
                quota: ProviderQuota {
                    plan_name: "Codex / ChatGPT pro".into(),
                    windows: vec![QuotaWindow {
                        window_type: "7d".into(),
                        used: 100,
                        limit: 100,
                        remaining: 0,
                        reset_at: Some(
                            chrono::DateTime::parse_from_rfc3339("2026-07-20T00:00:00Z")
                                .unwrap()
                                .with_timezone(&chrono::Utc),
                        ),
                        period_seconds: Some(604800),
                    }],
                    unlimited: false,
                    banked_resets: Some(BankedResets {
                        available_count: 2,
                        credits: vec![
                            BankedResetCredit {
                                id: "c1".into(),
                                status: "available".into(),
                                title: Some("Full reset".into()),
                                description: Some(
                                    "You've been granted one free rate limit reset.".into(),
                                ),
                                granted_at: None,
                                expires_at: Some(
                                    chrono::DateTime::parse_from_rfc3339("2026-07-26T23:51:27Z")
                                        .unwrap()
                                        .with_timezone(&chrono::Utc),
                                ),
                                source: Some("Codex Team".into()),
                            },
                            BankedResetCredit {
                                id: "c2".into(),
                                status: "available".into(),
                                title: Some("Full reset".into()),
                                description: None,
                                granted_at: None,
                                expires_at: Some(
                                    chrono::DateTime::parse_from_rfc3339("2026-07-31T19:09:54Z")
                                        .unwrap()
                                        .with_timezone(&chrono::Utc),
                                ),
                                source: None,
                            },
                        ],
                    }),
                },
            },
            fetched_at: chrono::Utc::now(),
            raw_response: None,
            auth_source: Some("oauth:~/.codex/auth.json".into()),
            cached_at: None,
        }
    }

    #[test]
    fn renders_banked_resets_section_with_expiry() {
        let out = render_detail_text(codex_with_banked_resets(), 100, 28);
        assert!(
            out.contains("banked resets") && out.contains("2 available"),
            "section header: {out}"
        );
        assert!(out.contains("Full reset"), "credit title: {out}");
        assert!(
            out.contains("expires in") || out.contains("expired"),
            "expiry label: {out}"
        );
        assert!(out.contains("Codex Team"), "source: {out}");
        assert!(
            out.contains("free rate limit reset"),
            "description: {out}"
        );
    }

    #[test]
    fn renders_multipart_raw_with_extra_after_main() {
        let mut result = codex_with_banked_resets();
        result.raw_response = Some(serde_json::json!({
            "usage": {
                "plan_type": "pro",
                "rate_limit_reset_credits": { "available_count": 2 }
            },
            "rate_limit_reset_credits": {
                "available_count": 2,
                "credits": [{
                    "id": "c1",
                    "status": "available",
                    "title": "Full reset"
                }]
            }
        }));
        // Tall enough that the raw section is visible in the buffer.
        let out = render_detail_text(result, 100, 80);
        assert!(out.contains("raw response"), "header: {out}");
        assert!(out.contains("── usage ──"), "main part header: {out}");
        assert!(
            out.contains("── rate_limit_reset_credits ──"),
            "extra part header: {out}"
        );
        let usage_pos = out.find("── usage ──").expect("usage");
        let extra_pos = out
            .find("── rate_limit_reset_credits ──")
            .expect("extra");
        assert!(
            usage_pos < extra_pos,
            "extra must render after main usage; out:\n{out}"
        );
        assert!(
            out.contains("\"title\": \"Full reset\"") || out.contains("Full reset"),
            "extra body content: {out}"
        );
    }

    #[test]
    fn renders_flat_raw_without_multipart_headers() {
        let mut result = codex_with_banked_resets();
        result.raw_response = Some(serde_json::json!({
            "plan_type": "pro",
            "rate_limit": { "primary_window": { "used_percent": 10 } }
        }));
        let out = render_detail_text(result, 100, 80);
        assert!(out.contains("raw response"), "header: {out}");
        assert!(out.contains("\"plan_type\": \"pro\""), "flat body: {out}");
        // Single-endpoint body is not an envelope — no per-key subheaders.
        assert!(
            !out.contains("── plan_type ──"),
            "must not multipart-split scalar mixed body: {out}"
        );
    }

    #[test]
    fn renders_antigravity_group_quota_with_exact_counts() {
        // height ≥ 20 so Auto mode stays Normal with 2 windows (else Compact).
        let out = render_detail_text(antigravity_summary_quota_result(), 100, 24);

        assert!(out.contains("Antigravity"), "out:\n{out}");
        // 7d/gemini: 18 used / 82 left / 100 cap
        assert!(out.contains("18% used"), "out:\n{out}");
        assert!(out.contains("18 used"), "out:\n{out}");
        assert!(out.contains("82 left"), "out:\n{out}");
        assert!(out.contains("100 cap"), "out:\n{out}");
        assert!(out.contains("resets in"), "out:\n{out}");
        assert!(
            out.contains("gemini") || out.contains("7d/gemini") || out.contains("5h/gemini"),
            "out:\n{out}"
        );
    }

    #[test]
    fn moves_plan_into_header_above_the_fold() {
        let out = render_detail_text(antigravity_summary_quota_result(), 80, 18);
        let lines: Vec<&str> = out.lines().collect();

        assert!(lines.get(1).is_some_and(|line| line.contains("Antigravity")));
        assert!(lines.get(1).is_some_and(|line| line.contains("Updated")));
        assert!(lines
            .get(2)
            .is_some_and(|line| line.contains("auth: oauth")));
        assert!(
            lines.iter().take(4).any(|line| line.contains("Antigravity")),
            "expected plan name near the header"
        );
    }

    #[test]
    fn compact_layout_kicks_in_for_short_detail_view() {
        let out = render_detail_text(antigravity_summary_quota_result(), 80, 10);

        assert!(out.contains("Antigravity"));
        assert!(out.contains("Updated"));
        // Compact mode drops the long "X used · Y left · Z cap" trail.
        assert!(!out.contains("18 used · 82 left · 100 cap"));
    }

    #[test]
    fn detail_row_keys_put_favorites_first_and_hidden_last() {
        let result = ProviderResult {
            kind: ProviderKind::Claude,
            status: ProviderStatus::Available {
                quota: ProviderQuota {
                    plan_name: "Claude".into(),
                    windows: vec![
                        QuotaWindow {
                            window_type: "7d".into(),
                            used: 4,
                            limit: 100,
                            remaining: 96,
                            reset_at: None,
                            period_seconds: None,
                        },
                        QuotaWindow {
                            window_type: "5h".into(),
                            used: 1,
                            limit: 100,
                            remaining: 99,
                            reset_at: None,
                            period_seconds: None,
                        },
                        QuotaWindow {
                            window_type: "spark/7d".into(),
                            used: 2,
                            limit: 100,
                            remaining: 98,
                            reset_at: None,
                            period_seconds: None,
                        },
                    ],
                    unlimited: false,
                    banked_resets: None,
                },
            },
            fetched_at: chrono::Utc::now(),
            raw_response: None,
            auth_source: None,
            cached_at: None,
        };
        let prefs = QuotaPreferences {
            favorites: vec!["spark/7d".into()],
            hidden: vec!["5h".into()],
        };

        let rows = detail_row_keys(&result, false, &prefs);

        assert_eq!(rows[0].quota_key, "spark/7d");
        assert!(!rows[0].hidden);
        assert_eq!(rows[2].quota_key, "5h");
        assert!(rows[2].hidden);
    }
}
