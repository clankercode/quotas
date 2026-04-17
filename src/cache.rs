use crate::providers::{ProviderKind, ProviderResult, ProviderStatus};
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
    let mut cache: CacheFile = serde_json::from_str(&content).unwrap_or_default();
    rehydrate_cached_results(&mut cache);
    cache
}

fn rehydrate_cached_results(cache: &mut CacheFile) {
    for entry in cache.entries.values_mut() {
        if entry.result.kind == ProviderKind::Gemini {
            rehydrate_gemini_result(&mut entry.result);
        }
    }
}

fn rehydrate_gemini_result(result: &mut ProviderResult) {
    let Some(raw) = &result.raw_response else {
        return;
    };
    let Ok(quota) = crate::providers::gemini::parse_quota(raw) else {
        return;
    };
    result.status = ProviderStatus::Available { quota };
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
    use crate::providers::{ProviderKind, ProviderQuota, ProviderStatus};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    #[test]
    fn read_cache_rehydrates_gemini_fraction_only_raw_response() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_xdg = std::env::var_os("XDG_CACHE_HOME");
        let cache_root =
            std::env::temp_dir().join(format!("quotas-cache-test-{}", std::process::id()));
        let cache_dir = cache_root.join("quotas");
        std::fs::create_dir_all(&cache_dir).unwrap();

        let cached_at = Utc::now();
        let result = ProviderResult {
            kind: ProviderKind::Gemini,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: "stale".into(),
                    console_url: None,
                },
            },
            fetched_at: cached_at,
            raw_response: Some(serde_json::json!({
                "buckets": [
                    {
                        "remainingFraction": 0.96,
                        "resetTime": "2026-04-18T04:00:00Z",
                        "modelId": "gemini-2.5-flash",
                        "tokenType": "REQUESTS"
                    }
                ]
            })),
            auth_source: None,
            cached_at: None,
        };
        let mut file = CacheFile::default();
        file.entries
            .insert("gemini".into(), CacheEntry { result, cached_at });
        std::fs::write(
            cache_dir.join("cache.json"),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();

        std::env::set_var("XDG_CACHE_HOME", &cache_root);
        let cache = read_cache();
        if let Some(value) = previous_xdg {
            std::env::set_var("XDG_CACHE_HOME", value);
        } else {
            std::env::remove_var("XDG_CACHE_HOME");
        }
        let _ = std::fs::remove_dir_all(cache_root);

        let gemini = cache.entries.get("gemini").unwrap();
        let ProviderStatus::Available { quota } = &gemini.result.status else {
            panic!("expected available Gemini quota");
        };
        assert_eq!(quota.windows[0].limit, 100);
        assert_eq!(quota.windows[0].remaining, 96);
        assert_eq!(quota.windows[0].used, 4);
    }
}
