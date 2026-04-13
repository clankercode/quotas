use crate::providers::{ProviderResult, ProviderStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub result: ProviderResult,
    pub cached_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CacheFile {
    pub entries: BTreeMap<String, CacheEntry>,
}

/// Returns `$XDG_CACHE_HOME/quotas/cache.json`, falling back to `~/.cache/quotas/cache.json`.
pub fn cache_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("quotas/cache.json"));
        }
    }
    let home = dirs::home_dir()?;
    Some(home.join(".cache/quotas/cache.json"))
}

pub fn read_cache() -> CacheFile {
    let Some(path) = cache_path() else {
        return CacheFile::default();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return CacheFile::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// Merge a batch of results into the cache. Each provider entry is updated
/// only if the new status is "better" than what's cached:
///
///   available > unavailable > auth_required > network_error
///
/// This prevents transient network errors from clobbering good cached data.
pub fn write_cache(results: &[ProviderResult]) {
    let Some(path) = cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut cache = read_cache();
    for result in results {
        let key = result.kind.slug().to_string();
        let should_overwrite = match cache.entries.get(&key) {
            None => true,
            Some(existing) => {
                status_priority(&result.status) >= status_priority(&existing.result.status)
            }
        };
        if should_overwrite {
            cache.entries.insert(
                key,
                CacheEntry {
                    result: result.clone(),
                    cached_at: Utc::now(),
                },
            );
        }
    }

    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = std::fs::write(&path, json);
    }
}

/// Age of the oldest cache entry, or None if cache is empty.
pub fn cache_age(cache: &CacheFile) -> Option<Duration> {
    let now = Utc::now();
    cache
        .entries
        .values()
        .map(|e| {
            let secs = (now - e.cached_at).num_seconds().max(0) as u64;
            Duration::from_secs(secs)
        })
        .max()
}

/// Higher = better. Used for merge priority.
fn status_priority(status: &ProviderStatus) -> u8 {
    match status {
        ProviderStatus::Available { .. } => 3,
        ProviderStatus::Unavailable { .. } => 2,
        ProviderStatus::AuthRequired => 1,
        ProviderStatus::NetworkError { .. } => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ProviderQuota, ProviderStatus};

    #[test]
    fn priority_ordering() {
        let avail = status_priority(&ProviderStatus::Available {
            quota: ProviderQuota {
                plan_name: "test".into(),
                windows: vec![],
                unlimited: false,
            },
        });
        let unavail = status_priority(&ProviderStatus::Unavailable {
            info: crate::providers::UnavailableInfo {
                reason: "test".into(),
                console_url: None,
            },
        });
        let auth = status_priority(&ProviderStatus::AuthRequired);
        let net = status_priority(&ProviderStatus::NetworkError {
            message: "fail".into(),
        });
        assert!(avail > unavail);
        assert!(unavail > auth);
        assert!(auth > net);
    }
}
