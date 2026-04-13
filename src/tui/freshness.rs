#[derive(Clone, Copy)]
pub enum Staleness {
    Fresh,
    Warning,
    Stale,
}

pub struct FreshnessLabel {
    pub label: String,
    pub staleness: Staleness,
    /// How far through the refresh period [0.0, 1.0].
    /// Used to draw the bg-fill progress bar on the freshness text.
    pub fraction: f64,
}

impl FreshnessLabel {
    /// Construct with an explicit refresh interval (seconds).
    /// Staleness turns Warning 15s before the next refresh, Stale at/after.
    pub fn with_interval(seconds_ago: i64, interval_secs: u64) -> Self {
        let label = if seconds_ago < 60 {
            format!("Updated {}s ago", seconds_ago)
        } else {
            format!("Updated {}m ago", seconds_ago / 60)
        };
        let iv = interval_secs as i64;
        let fraction = (seconds_ago as f64 / iv as f64).clamp(0.0, 1.0);
        let staleness = if seconds_ago >= iv {
            Staleness::Stale
        } else if seconds_ago >= iv - 15 {
            Staleness::Warning
        } else {
            Staleness::Fresh
        };
        Self {
            label,
            staleness,
            fraction,
        }
    }
}
