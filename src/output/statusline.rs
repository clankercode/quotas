use crate::cache::CacheFile;
use crate::providers::{ProviderKind, ProviderResult, ProviderStatus, QuotaWindow};
use std::collections::BTreeSet;

/// Nerd Font icons for each provider status.
const ICON_AVAILABLE: &str = "\u{f05d}"; // nf-fa-check_circle (  )

const DEFAULT_FORMAT: &str = "%provider %remaining/%limit";

pub struct StatusLineConfig {
    pub icons: bool,
    pub providers: Vec<ProviderKind>,
    pub format: Option<String>,
}

impl Default for StatusLineConfig {
    fn default() -> Self {
        Self {
            icons: true,
            providers: vec![],
            format: None,
        }
    }
}

/// Render a statusline string from a cache file.
pub fn render(cache: &CacheFile, config: &StatusLineConfig) -> String {
    let format = config.format.as_deref().unwrap_or(DEFAULT_FORMAT);
    let filter: Option<BTreeSet<ProviderKind>> = if config.providers.is_empty() {
        None
    } else {
        Some(config.providers.iter().copied().collect())
    };

    let mut parts: Vec<String> = Vec::new();

    for entry in cache.entries.values() {
        if let Some(ref f) = filter {
            if !f.contains(&entry.result.kind) {
                continue;
            }
        }

        if let Some(segment) = format_result(&entry.result, format, config.icons) {
            parts.push(segment);
        }
    }

    parts.join(" | ")
}

/// Format a single provider result according to the template.
/// Returns None if the provider isn't in a showable state
/// (auth_required, unavailable, network_error).
fn format_result(result: &ProviderResult, format: &str, icons: bool) -> Option<String> {
    match &result.status {
        ProviderStatus::Available { quota } => {
            let primary = primary_window(&quota.windows);
            let mut out = if icons {
                format!("{ICON_AVAILABLE} ")
            } else {
                String::new()
            };

            let remaining_str = if quota.unlimited {
                "\u{221E}".to_string() // ∞
            } else {
                primary.map_or("?".into(), |w| w.remaining.to_string())
            };
            let limit_str = if quota.unlimited {
                "\u{221E}".to_string()
            } else {
                primary.map_or("?".into(), |w| w.limit.to_string())
            };
            let used_str = primary.map_or("?".into(), |w| w.used.to_string());
            let window_str = primary.map_or("?".into(), |w| w.window_type.clone());
            let reset_str = primary
                .and_then(|w| w.reset_at)
                .map(|t| format_timestamp(&t))
                .unwrap_or_else(|| "?".into());

            let mut s = format.to_string();
            s = s.replace("%provider", result.kind.display_name());
            s = s.replace("%remaining", &remaining_str);
            s = s.replace("%limit", &limit_str);
            s = s.replace("%used", &used_str);
            s = s.replace("%window", &window_str);
            s = s.replace("%reset", &reset_str);

            out.push_str(&s);
            Some(out)
        }
        ProviderStatus::Unavailable { .. }
        | ProviderStatus::AuthRequired
        | ProviderStatus::NetworkError { .. } => None,
    }
}

/// Pick the "primary" window to display. Prefers the first non-unlimited window,
/// falling back to the first window if all are unlimited or list is empty.
fn primary_window(windows: &[QuotaWindow]) -> Option<&QuotaWindow> {
    if windows.is_empty() {
        return None;
    }
    // If there's a window with a finite limit, prefer it.
    windows
        .iter()
        .find(|w| w.limit > 0)
        .or_else(|| windows.first())
}

fn format_timestamp(ts: &chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let diff = *ts - now;
    let mins = diff.num_minutes();
    if mins < 0 {
        return "now".into();
    }
    if mins < 60 {
        format!("{mins}m")
    } else {
        format!("{}h{}m", mins / 60, mins % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ProviderKind, ProviderQuota, ProviderStatus};
    use chrono::Utc;

    fn available_result(kind: ProviderKind, remaining: i64, limit: i64) -> ProviderResult {
        ProviderResult {
            kind,
            status: ProviderStatus::Available {
                quota: ProviderQuota {
                    plan_name: "test".into(),
                    windows: vec![QuotaWindow {
                        window_type: "5h".into(),
                        used: limit - remaining,
                        limit,
                        remaining,
                        reset_at: None,
                        period_seconds: Some(18000),
                    }],
                    unlimited: false,
                },
            },
            fetched_at: Utc::now(),
            raw_response: None,
            auth_source: None,
            cached_at: None,
        }
    }

    fn unlimited_result(kind: ProviderKind) -> ProviderResult {
        ProviderResult {
            kind,
            status: ProviderStatus::Available {
                quota: ProviderQuota {
                    plan_name: "unlimited".into(),
                    windows: vec![],
                    unlimited: true,
                },
            },
            fetched_at: Utc::now(),
            raw_response: None,
            auth_source: None,
            cached_at: None,
        }
    }

    fn auth_required_result(kind: ProviderKind) -> ProviderResult {
        ProviderResult {
            kind,
            status: ProviderStatus::AuthRequired,
            fetched_at: Utc::now(),
            raw_response: None,
            auth_source: None,
            cached_at: None,
        }
    }

    #[test]
    fn default_format_available() {
        let mut cache = CacheFile::default();
        cache.entries.insert(
            "claude".into(),
            crate::cache::CacheEntry {
                result: available_result(ProviderKind::Claude, 42, 50),
                cached_at: Utc::now(),
            },
        );
        let config = StatusLineConfig {
            icons: false,
            ..Default::default()
        };
        let out = render(&cache, &config);
        assert_eq!(out, "Claude 42/50");
    }

    #[test]
    fn default_format_unlimited() {
        let mut cache = CacheFile::default();
        cache.entries.insert(
            "codex".into(),
            crate::cache::CacheEntry {
                result: unlimited_result(ProviderKind::Codex),
                cached_at: Utc::now(),
            },
        );
        let config = StatusLineConfig {
            icons: false,
            ..Default::default()
        };
        let out = render(&cache, &config);
        assert!(out.contains("\u{221E}"), "expected ∞ in: {out}");
    }

    #[test]
    fn auth_required_omitted() {
        let mut cache = CacheFile::default();
        cache.entries.insert(
            "claude".into(),
            crate::cache::CacheEntry {
                result: auth_required_result(ProviderKind::Claude),
                cached_at: Utc::now(),
            },
        );
        let config = StatusLineConfig::default();
        let out = render(&cache, &config);
        assert!(out.is_empty());
    }

    #[test]
    fn provider_filter() {
        let mut cache = CacheFile::default();
        cache.entries.insert(
            "claude".into(),
            crate::cache::CacheEntry {
                result: available_result(ProviderKind::Claude, 42, 50),
                cached_at: Utc::now(),
            },
        );
        cache.entries.insert(
            "codex".into(),
            crate::cache::CacheEntry {
                result: available_result(ProviderKind::Codex, 10, 20),
                cached_at: Utc::now(),
            },
        );
        let config = StatusLineConfig {
            icons: false,
            providers: vec![ProviderKind::Claude],
            format: None,
        };
        let out = render(&cache, &config);
        assert!(out.contains("Claude"));
        assert!(!out.contains("Codex"));
    }
}
