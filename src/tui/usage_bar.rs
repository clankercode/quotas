use ratatui::widgets::Gauge;

pub struct UsageBar;

impl UsageBar {
    pub fn render(used: i64, limit: i64, window_type: &str) -> String {
        if limit == 0 {
            return format!("{} [unlimited]", window_type);
        }
        let pct = ((used as f64) / (limit as f64) * 100.0).min(100.0);
        format!(
            "{} {:.0}% left ({}/{})",
            window_type,
            100.0 - pct,
            limit - used,
            limit
        )
    }

    pub fn gauge(used: i64, limit: i64, label: &str) -> Gauge<'_> {
        if limit == 0 {
            return Gauge::default()
                .label(format!("{} [unlimited]", label))
                .ratio(1.0);
        }
        let ratio = (used as f64) / (limit as f64);
        Gauge::default()
            .label(format!("{:.0}%", (1.0 - ratio) * 100.0))
            .ratio(ratio.min(1.0))
    }
}
