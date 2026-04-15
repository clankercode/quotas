use crate::providers::QuotaWindow;
use chrono::Utc;
use ratatui::prelude::*;

/// Returns `(symbol, scale)` if the window type represents a currency balance
/// (e.g. `balance_usd`, `paid_cny`). `scale` is the integer divisor to get
/// the decimal amount (10_000.0 for ×10000 storage, 100.0 for ×100).
pub fn currency_window(window_type: &str) -> Option<(&'static str, f64)> {
    let lower = window_type.to_ascii_lowercase();
    // payg_balance uses ×100 scaling (legacy Kimi format).
    if lower == "payg_balance" {
        return Some(("$", 100.0));
    }
    if lower.ends_with("_usd") {
        return Some(("$", 10_000.0));
    }
    if lower.ends_with("_cny") {
        return Some(("¥", 10_000.0));
    }
    None
}

/// Fraction of the window's period that has already *elapsed*. Returns
/// None when `reset_at` or `period_seconds` is missing. Mirrors the
/// semantics of `used_pct` so both share the same left-to-right axis.
pub fn time_elapsed_fraction(w: &QuotaWindow) -> Option<f64> {
    let reset = w.reset_at?;
    let period = w.period_seconds?;
    if period <= 0 {
        return None;
    }
    let remaining = (reset - Utc::now()).num_seconds();
    if remaining <= 0 {
        return Some(1.0);
    }
    if remaining >= period {
        return Some(0.0);
    }
    Some(((period - remaining) as f64 / period as f64).clamp(0.0, 1.0))
}

/// Pick the "base" fill color based on how much of the window is used.
pub fn bar_color(used_pct: f64) -> Color {
    if used_pct >= 0.90 {
        Color::Red
    } else if used_pct >= 0.75 {
        Color::Yellow
    } else {
        Color::Green
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    OnPace,
    Overspend,
    Slack,
    Future,
}

const OVERSPEND_RGB: Color = Color::Rgb(255, 140, 0);

fn cells_for(used_pct: f64, time_elapsed_pct: Option<f64>, width: usize) -> Vec<Cell> {
    let used_cells = ((used_pct.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let time_cells =
        time_elapsed_pct.map(|t| ((t.clamp(0.0, 1.0)) * width as f64).round() as usize);
    (0..width)
        .map(|i| {
            let is_used = i < used_cells;
            let past_time = match time_cells {
                Some(t) => i < t,
                None => is_used,
            };
            match (is_used, past_time) {
                (true, true) => Cell::OnPace,
                (true, false) => Cell::Overspend,
                (false, true) => Cell::Slack,
                (false, false) => Cell::Future,
            }
        })
        .collect()
}

/// Build the dual-info bar: fill grows left-to-right with `used_pct`;
/// any portion used *beyond* the elapsed-time fraction is drawn in
/// orange to flag overspending; the gap between used and elapsed when
/// used < elapsed is shown as a dim "slack" region so it's visible that
/// you're pacing ahead.
pub fn build<'a>(
    used_pct: f64,
    time_elapsed_pct: Option<f64>,
    width: u16,
    base_color: Color,
) -> Vec<Span<'a>> {
    build_labeled(used_pct, time_elapsed_pct, width, base_color, "")
}

/// Like `build`, but with an overlay string centered in the bar. Cells
/// under the overlay replace their block char with the overlay char and
/// take a background color matching the underlying fill so the text
/// reads as a label sitting on the bar.
pub fn build_labeled<'a>(
    used_pct: f64,
    time_elapsed_pct: Option<f64>,
    width: u16,
    base_color: Color,
    overlay: &str,
) -> Vec<Span<'a>> {
    let w = width as usize;
    if w == 0 {
        return Vec::new();
    }
    let cells = cells_for(used_pct, time_elapsed_pct, w);

    let overlay_chars: Vec<char> = overlay.chars().collect();
    let overlay_len = overlay_chars.len().min(w);
    // Anchor the overlay at the fill/empty boundary so the text hugs the
    // interesting visual edge. Fill ~0% → left-aligned; fill ~100% →
    // right-aligned; fill ~50% → centered. Avoids the floating-text
    // problem when a wide bar has only a few cells filled.
    let used_cells = ((used_pct.clamp(0.0, 1.0)) * w as f64).round() as usize;
    let overlay_start = if overlay_len == 0 {
        w
    } else {
        let half = overlay_len / 2;
        let raw = used_cells.saturating_sub(half);
        raw.min(w - overlay_len)
    };
    let overlay_end = overlay_start + overlay_len;

    let base_char_for = |c: Cell| match c {
        Cell::OnPace | Cell::Overspend => '█',
        Cell::Slack => '▒',
        Cell::Future => '░',
    };
    let base_style_for = |c: Cell| match c {
        Cell::OnPace => Style::new().fg(base_color),
        Cell::Overspend => Style::new().fg(OVERSPEND_RGB),
        Cell::Slack => Style::new().fg(Color::DarkGray),
        Cell::Future => Style::new().fg(Color::DarkGray),
    };
    let overlay_style_for = |c: Cell| match c {
        Cell::OnPace => Style::new().bg(base_color).fg(Color::Black).bold(),
        Cell::Overspend => Style::new().bg(OVERSPEND_RGB).fg(Color::Black).bold(),
        Cell::Slack => Style::new().bg(Color::DarkGray).fg(Color::White).bold(),
        Cell::Future => Style::new().fg(Color::White).bold(),
    };

    let mut out: Vec<Span<'a>> = Vec::new();
    let mut buf = String::new();
    let mut cur_style: Option<Style> = None;
    let flush = |buf: &mut String, out: &mut Vec<Span<'a>>, style: Option<Style>| {
        if !buf.is_empty() {
            out.push(Span::styled(std::mem::take(buf), style.unwrap_or_default()));
        }
    };
    for i in 0..w {
        let cell = cells[i];
        let in_overlay = i >= overlay_start && i < overlay_end;
        let (ch, style) = if in_overlay {
            (overlay_chars[i - overlay_start], overlay_style_for(cell))
        } else {
            (base_char_for(cell), base_style_for(cell))
        };
        if cur_style == Some(style) {
            buf.push(ch);
        } else {
            flush(&mut buf, &mut out, cur_style);
            cur_style = Some(style);
            buf.push(ch);
        }
    }
    flush(&mut buf, &mut out, cur_style);
    out
}

/// Sort key that clusters windows by period bucket:
/// 5h / hourly first, then weekly / 7d, then monthly, then credits / balances.
pub fn window_sort_key(w: &QuotaWindow) -> (u8, String) {
    let wt = w.window_type.as_str();
    let lower = wt.to_ascii_lowercase();
    let bucket = if currency_window(wt).is_some() {
        9
    } else if lower.contains("credit") {
        8
    } else if lower.starts_with("monthly") || lower.contains("month") {
        3
    } else if lower == "5h" || lower.starts_with("5h/") || lower.starts_with("5h ") {
        1
    } else if lower.starts_with("7d") || lower.starts_with("wk") || lower.starts_with("weekly") {
        2
    } else {
        5
    };
    (bucket, lower)
}

/// Label to show as a section header for a bucket of windows.
pub fn bucket_label(bucket: u8) -> Option<&'static str> {
    match bucket {
        1 => Some("── 5h ──"),
        2 => Some("── 7d ──"),
        3 => Some("── monthly ──"),
        8 => Some("── credits ──"),
        _ => None,
    }
}

/// When a section header is active for a bucket, the window's own
/// bucket prefix (`5h/`, `wk/`, `7d/`, `monthly_`, etc.) is redundant.
/// Strip it so more of the model name fits. Also applies a small
/// provider-agnostic rename table so ugly names read better in the UI.
/// If stripping would leave an empty string, keep the original.
pub fn display_label(window_type: &str, show_headers: bool) -> String {
    // First, the rename table — applies regardless of show_headers
    // because these are purely cosmetic improvements to the raw key.
    let renamed: &str = match window_type {
        "weekly" => "7d",
        "weekly_sonnet" => "7d Sonnet",
        "weekly_opus" => "7d Opus",
        "weekly_haiku" => "7d Haiku",
        "monthly_mcp" => "month MCP",
        "monthly" => "month",
        "extra_credits" => "credits",
        "payg_balance" => "PAYG",
        "balance_usd" | "balance_cny" => "balance",
        "paid_cny" => "paid",
        "free_cny" => "free",
        "granted_cny" | "granted_usd" => "granted",
        "topped_up_cny" | "topped_up_usd" => "topped-up",
        "credits_usd" => "credits",
        "key_limit_usd" => "key limit",
        // Cursor-specific short labels
        "api_usage_pct" => "API%",
        "auto_usage_pct" => "Auto%",
        "billing_cycle" => "BC",
        "spend_limit" => "$ Lim",
        "bonus" => "Bonus",
        other => other,
    };
    if !show_headers {
        return renamed.to_string();
    }
    for prefix in ["5h/", "wk/", "7d/", "weekly/", "monthly/", "monthly_"] {
        if let Some(rest) = renamed.strip_prefix(prefix) {
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    renamed.to_string()
}

/// Whether a window type should be hidden by default to keep cards compact.
/// Users can reveal them via `ui.show_all_windows = true` in config.
pub fn autohide_window(window_type: &str) -> bool {
    matches!(window_type, "billing_cycle")
}

/// Suffix-preserving truncation: keeps the *end* of the string so that
/// `coding-plan-vlm` and `coding-plan-search` stay distinguishable when
/// forced to share a narrow label column. Unicode-safe.
pub fn truncate_suffix(s: &str, n: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= n {
        return s.to_string();
    }
    let keep = n.saturating_sub(1);
    let skip = char_count - keep;
    let mut out = String::from("…");
    out.extend(s.chars().skip(skip));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_string(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn fill_grows_from_left() {
        let s = to_string(&build(0.0, None, 4, Color::Green));
        assert_eq!(s, "░░░░");
        let s = to_string(&build(1.0, None, 4, Color::Green));
        assert_eq!(s, "████");
        let s = to_string(&build(0.5, None, 4, Color::Green));
        assert_eq!(s, "██░░");
    }

    #[test]
    fn overspend_tail_shows_when_used_exceeds_time() {
        // used 100%, time 25% — cells 1..4 (i.e. 3 of 4) are overspend.
        let s = to_string(&build(1.0, Some(0.25), 4, Color::Green));
        assert_eq!(s, "████");
    }

    #[test]
    fn slack_shows_between_used_and_time_when_ahead() {
        // used 25%, time 75% → cells 1..3 (2 of 4) are slack ▒.
        let s = to_string(&build(0.25, Some(0.75), 4, Color::Green));
        assert_eq!(s, "█▒▒░");
    }

    #[test]
    fn sort_key_orders_5h_before_weekly() {
        let a = QuotaWindow {
            window_type: "wk/M*".into(),
            used: 0,
            limit: 0,
            remaining: 0,
            reset_at: None,
            period_seconds: None,
        };
        let b = QuotaWindow {
            window_type: "5h/M*".into(),
            used: 0,
            limit: 0,
            remaining: 0,
            reset_at: None,
            period_seconds: None,
        };
        assert!(window_sort_key(&b) < window_sort_key(&a));
    }
}
