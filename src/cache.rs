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
        match entry.result.kind {
            ProviderKind::Antigravity => rehydrate_antigravity_result(&mut entry.result),
            // Re-parse so label/window changes (e.g. 168h → 7d) apply to
            // cached entries without waiting for the next live refresh.
            ProviderKind::Codex => rehydrate_codex_result(&mut entry.result),
            _ => {}
        }
    }
}

fn rehydrate_antigravity_result(result: &mut ProviderResult) {
    let Some(raw) = &result.raw_response else {
        return;
    };
    let Ok(quota) = crate::providers::antigravity::parse_quota(raw) else {
        return;
    };
    result.status = ProviderStatus::Available { quota };
}

fn rehydrate_codex_result(result: &mut ProviderResult) {
    let Some(raw) = &result.raw_response else {
        return;
    };
    // Multi-part envelopes put usage under "usage"; flat bodies are legacy.
    let usage = crate::providers::codex::usage_body_from_raw(raw);
    let mut quota = crate::providers::codex::parse_usage(usage);
    // Prefer per-credit rows from the stored detail endpoint body when present.
    if let Some(detail_body) = raw.get("rate_limit_reset_credits") {
        if let Some(detail) =
            crate::providers::codex::parse_banked_reset_credits_detail(detail_body)
        {
            crate::providers::codex::merge_banked_reset_detail(&mut quota, detail);
        }
    }
    // Flat usage-only cache: usage raw only has banked count; preserve
    // per-credit detail from the previous status when re-parse would wipe
    // it — but only while the count still matches (spent credits must not
    // reappear).
    if let ProviderStatus::Available { quota: old } = &result.status {
        if let (Some(new_br), Some(old_br)) = (&mut quota.banked_resets, &old.banked_resets) {
            if new_br.available_count <= 0 {
                new_br.credits.clear();
            } else if new_br.credits.is_empty()
                && !old_br.credits.is_empty()
                && new_br.available_count == old_br.available_count
            {
                new_br.credits = old_br.credits.clone();
            }
        }
    }
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
                banked_resets: None,
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
    fn read_cache_rehydrates_antigravity_summary_raw_response() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_xdg = std::env::var_os("XDG_CACHE_HOME");
        let cache_root =
            std::env::temp_dir().join(format!("quotas-cache-test-{}", std::process::id()));
        let cache_dir = cache_root.join("quotas");
        std::fs::create_dir_all(&cache_dir).unwrap();

        let cached_at = Utc::now();
        let result = ProviderResult {
            kind: ProviderKind::Antigravity,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: "stale".into(),
                    console_url: None,
                },
            },
            fetched_at: cached_at,
            raw_response: Some(serde_json::json!({
                "groups": [{
                    "displayName": "Gemini Models",
                    "buckets": [{
                        "bucketId": "gemini-weekly",
                        "displayName": "Weekly Limit",
                        "window": "weekly",
                        "resetTime": "2026-07-19T17:07:49Z",
                        "remainingFraction": 0.82
                    }, {
                        "bucketId": "gemini-5h",
                        "displayName": "Five Hour Limit",
                        "window": "5h",
                        "resetTime": "2026-07-13T18:32:16Z",
                        "remainingFraction": 0.98
                    }]
                }]
            })),
            auth_source: None,
            cached_at: None,
        };
        let mut file = CacheFile::default();
        file.entries
            .insert("antigravity".into(), CacheEntry { result, cached_at });
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

        let entry = cache.entries.get("antigravity").unwrap();
        let ProviderStatus::Available { quota } = &entry.result.status else {
            panic!("expected available Antigravity quota");
        };
        assert_eq!(quota.plan_name, "Antigravity");
        assert_eq!(quota.windows.len(), 2);
        let weekly = quota.windows.iter().find(|w| w.window_type == "7d/gemini").unwrap();
        assert_eq!(weekly.limit, 100);
        assert_eq!(weekly.remaining, 82);
        assert_eq!(weekly.used, 18);
    }

    #[test]
    fn read_cache_rehydrates_codex_week_window_labels() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_xdg = std::env::var_os("XDG_CACHE_HOME");
        let cache_root = std::env::temp_dir().join(format!(
            "quotas-cache-codex-test-{}",
            std::process::id()
        ));
        let cache_dir = cache_root.join("quotas");
        std::fs::create_dir_all(&cache_dir).unwrap();

        let cached_at = Utc::now();
        // Stale status used the old 168h label; raw_response has a 7-day
        // primary window that the current parser labels `7d`.
        let result = ProviderResult {
            kind: ProviderKind::Codex,
            status: ProviderStatus::Available {
                quota: crate::providers::ProviderQuota {
                    plan_name: "stale".into(),
                    windows: vec![crate::providers::QuotaWindow {
                        window_type: "168h".into(),
                        used: 76,
                        limit: 100,
                        remaining: 24,
                        reset_at: None,
                        period_seconds: Some(604800),
                    }],
                    unlimited: false,
                    banked_resets: None,
                },
            },
            fetched_at: cached_at,
            raw_response: Some(serde_json::json!({
                "plan_type": "pro",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 76,
                        "limit_window_seconds": 604800,
                        "reset_at": 1744934400i64
                    },
                    "secondary_window": null
                },
                "credits": { "balance": "0", "unlimited": false }
            })),
            auth_source: None,
            cached_at: None,
        };
        let mut file = CacheFile::default();
        file.entries
            .insert("codex".into(), CacheEntry { result, cached_at });
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

        let codex = cache.entries.get("codex").unwrap();
        let ProviderStatus::Available { quota } = &codex.result.status else {
            panic!("expected available Codex quota");
        };
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].window_type, "7d");
        assert_eq!(quota.windows[0].used, 76);
    }
    #[test]
    fn rehydrate_preserves_codex_banked_credit_details_when_count_matches() {
        use crate::providers::{BankedResetCredit, BankedResets, ProviderKind, ProviderStatus};

        let usage = serde_json::json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 10,
                    "limit_window_seconds": 604800,
                    "reset_at": 1784487518i64
                },
                "secondary_window": null
            },
            "credits": { "balance": "0", "unlimited": false },
            "rate_limit_reset_credits": { "available_count": 2 }
        });
        let mut result = ProviderResult {
            kind: ProviderKind::Codex,
            status: ProviderStatus::Available {
                quota: crate::providers::ProviderQuota {
                    plan_name: "stale".into(),
                    windows: vec![],
                    unlimited: false,
                    banked_resets: Some(BankedResets {
                        available_count: 2,
                        credits: vec![BankedResetCredit {
                            id: "c1".into(),
                            status: "available".into(),
                            title: Some("Full reset".into()),
                            description: None,
                            granted_at: None,
                            expires_at: None,
                            source: None,
                        }],
                    }),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(usage),
            auth_source: None,
            cached_at: None,
        };
        rehydrate_codex_result(&mut result);
        let ProviderStatus::Available { quota } = &result.status else {
            panic!("expected available");
        };
        let br = quota.banked_resets.as_ref().expect("banked");
        assert_eq!(br.available_count, 2);
        assert_eq!(br.credits.len(), 1);
        assert_eq!(br.credits[0].title.as_deref(), Some("Full reset"));
    }

    #[test]
    fn rehydrate_drops_banked_credits_when_count_is_zero() {
        use crate::providers::{BankedResetCredit, BankedResets, ProviderKind, ProviderStatus};

        let usage = serde_json::json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 0,
                    "limit_window_seconds": 604800,
                    "reset_at": 1784487518i64
                }
            },
            "credits": { "balance": "0", "unlimited": false },
            "rate_limit_reset_credits": { "available_count": 0 }
        });
        let mut result = ProviderResult {
            kind: ProviderKind::Codex,
            status: ProviderStatus::Available {
                quota: crate::providers::ProviderQuota {
                    plan_name: "stale".into(),
                    windows: vec![],
                    unlimited: false,
                    banked_resets: Some(BankedResets {
                        available_count: 2,
                        credits: vec![BankedResetCredit {
                            id: "c1".into(),
                            status: "available".into(),
                            title: Some("Full reset".into()),
                            description: None,
                            granted_at: None,
                            expires_at: None,
                            source: None,
                        }],
                    }),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(usage),
            auth_source: None,
            cached_at: None,
        };
        rehydrate_codex_result(&mut result);
        let ProviderStatus::Available { quota } = &result.status else {
            panic!("expected available");
        };
        let br = quota.banked_resets.as_ref().expect("banked field present with 0");
        assert_eq!(br.available_count, 0);
        assert!(br.credits.is_empty(), "spent credits must not reappear");
    }

    #[test]
    fn rehydrate_reads_banked_detail_from_multipart_raw_envelope() {
        use crate::providers::{ProviderKind, ProviderStatus};

        let envelope = serde_json::json!({
            "usage": {
                "plan_type": "pro",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 10,
                        "limit_window_seconds": 604800,
                        "reset_at": 1784487518i64
                    },
                    "secondary_window": null
                },
                "credits": { "balance": "0", "unlimited": false },
                "rate_limit_reset_credits": { "available_count": 1 }
            },
            "rate_limit_reset_credits": {
                "available_count": 1,
                "credits": [{
                    "id": "from-raw",
                    "status": "available",
                    "title": "Full reset",
                    "profile_user_id": "Codex Team"
                }]
            }
        });
        let mut result = ProviderResult {
            kind: ProviderKind::Codex,
            status: ProviderStatus::Available {
                quota: crate::providers::ProviderQuota {
                    plan_name: "stale".into(),
                    windows: vec![],
                    unlimited: false,
                    banked_resets: None,
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(envelope),
            auth_source: None,
            cached_at: None,
        };
        rehydrate_codex_result(&mut result);
        let ProviderStatus::Available { quota } = &result.status else {
            panic!("expected available");
        };
        assert_eq!(quota.windows[0].window_type, "7d");
        let br = quota.banked_resets.as_ref().expect("banked");
        assert_eq!(br.available_count, 1);
        assert_eq!(br.credits.len(), 1);
        assert_eq!(br.credits[0].id, "from-raw");
        assert_eq!(br.credits[0].title.as_deref(), Some("Full reset"));
        assert_eq!(br.credits[0].source.as_deref(), Some("Codex Team"));
    }

}
