#[derive(Clone, Copy)]
pub enum Staleness {
    Fresh,
    Warning,
    Stale,
}

pub struct FreshnessLabel {
    pub label: String,
    pub staleness: Staleness,
}

impl FreshnessLabel {
    pub fn new(seconds_ago: i64) -> Self {
        let label = if seconds_ago < 60 {
            format!("Updated {}s ago", seconds_ago)
        } else {
            format!("Updated {}m ago", seconds_ago / 60)
        };
        let staleness = if seconds_ago >= 300 {
            Staleness::Stale
        } else if seconds_ago >= 120 {
            Staleness::Warning
        } else {
            Staleness::Fresh
        };
        Self { label, staleness }
    }
}
