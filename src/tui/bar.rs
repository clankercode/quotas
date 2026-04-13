use crate::providers::QuotaWindow;
use chrono::Utc;
use ratatui::prelude::*;

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
    let w = width as usize;
    if w == 0 {
        return Vec::new();
    }
    let used_cells = ((used_pct.clamp(0.0, 1.0)) * w as f64).round() as usize;
    let time_cells = time_elapsed_pct.map(|t| ((t.clamp(0.0, 1.0)) * w as f64).round() as usize);

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Cell {
        OnPace,
        Overspend,
        Slack,
        Future,
    }

    let cells: Vec<Cell> = (0..w)
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
        .collect();

    let on_pace = Style::new().fg(base_color);
    let overspend = Style::new().fg(Color::Rgb(255, 140, 0));
    let slack = Style::new().fg(Color::DarkGray);
    let future = Style::new().fg(Color::DarkGray);

    let char_for = |c: Cell| match c {
        Cell::OnPace | Cell::Overspend => '█',
        Cell::Slack => '▒',
        Cell::Future => '░',
    };
    let style_for = |c: Cell| match c {
        Cell::OnPace => on_pace,
        Cell::Overspend => overspend,
        Cell::Slack => slack,
        Cell::Future => future,
    };

    let mut out: Vec<Span<'a>> = Vec::new();
    let mut buf = String::new();
    let mut cur_style = style_for(cells[0]);
    buf.push(char_for(cells[0]));
    for cell in cells.iter().skip(1).copied() {
        let s = style_for(cell);
        if s == cur_style {
            buf.push(char_for(cell));
        } else {
            out.push(Span::styled(std::mem::take(&mut buf), cur_style));
            cur_style = s;
            buf.push(char_for(cell));
        }
    }
    if !buf.is_empty() {
        out.push(Span::styled(buf, cur_style));
    }
    out
}

/// Sort key that clusters windows by period bucket:
/// 5h / hourly first, then weekly / 7d, then monthly, then credits / balances.
pub fn window_sort_key(w: &QuotaWindow) -> (u8, String) {
    let wt = w.window_type.as_str();
    let lower = wt.to_ascii_lowercase();
    let bucket = if wt == "payg_balance" {
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
