use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;

pub struct KimiProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl KimiProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_payg(&self, key: &str) -> Result<ProviderResult> {
        let url = "https://api.moonshot.ai/v1/users/me/balance";
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 200 {
            let data = body.get("data").cloned().unwrap_or_default();
            let available = data
                .get("available_balance")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            return Ok(ProviderResult {
                kind: ProviderKind::Kimi,
                status: ProviderStatus::Available {
                    quota: ProviderQuota {
                        plan_name: "Kimi PAYG".to_string(),
                        windows: vec![QuotaWindow {
                            window_type: "payg_balance".to_string(),
                            used: 0,
                            limit: 0,
                            remaining: (available * 100.0) as i64,
                            reset_at: None,
                            period_seconds: None,
                        }],
                        unlimited: false,
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            });
        }

        let msg = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");

        Ok(ProviderResult {
            kind: ProviderKind::Kimi,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: msg.to_string(),
                    console_url: Some("https://platform.kimi.ai/".into()),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
            cached_at: None,
        })
    }

    async fn fetch_coding(&self, key: &str) -> Result<ProviderResult> {
        let url = "https://api.kimi.com/coding/v1/usages";
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 200 {
            let quota = parse_coding_response(&body);
            return Ok(ProviderResult {
                kind: ProviderKind::Kimi,
                status: ProviderStatus::Available { quota },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            });
        }

        self.fetch_payg(key).await
    }
}

/// Some fields come back as strings ("100") or as integers (100). Accept either.
fn num_field(v: &serde_json::Value, key: &str) -> i64 {
    match v.get(key) {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn parse_reset(v: &serde_json::Value) -> Option<DateTime<Utc>> {
    let s = v
        .get("reset_at")
        .or_else(|| v.get("resetTime"))
        .or_else(|| v.get("resetAt"))
        .and_then(|x| x.as_str())?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub(crate) fn parse_coding_response(body: &serde_json::Value) -> ProviderQuota {
    let usage = body.get("usage").cloned().unwrap_or_default();
    let limits = body
        .get("limits")
        .and_then(|l| l.as_array())
        .cloned()
        .unwrap_or_default();

    let mut windows = Vec::new();

    let weekly_limit = num_field(&usage, "limit");
    let weekly_used = num_field(&usage, "used");
    let weekly_remaining = if usage.get("remaining").is_some() {
        num_field(&usage, "remaining")
    } else {
        (weekly_limit - weekly_used).max(0)
    };
    let reset_at = parse_reset(&usage);

    if weekly_limit > 0 {
        windows.push(QuotaWindow {
            window_type: "weekly".to_string(),
            used: weekly_used,
            limit: weekly_limit,
            remaining: weekly_remaining,
            reset_at,
            period_seconds: Some(7 * 86400),
        });
    }

    for limit_entry in limits {
        let detail = limit_entry.get("detail").cloned().unwrap_or_default();
        let window = limit_entry.get("window").cloned().unwrap_or_default();
        let limit = num_field(&detail, "limit");
        let used = if detail.get("used").is_some() {
            num_field(&detail, "used")
        } else {
            let remaining = num_field(&detail, "remaining");
            (limit - remaining).max(0)
        };
        let remaining = if detail.get("remaining").is_some() {
            num_field(&detail, "remaining")
        } else {
            (limit - used).max(0)
        };
        let duration = window
            .get("duration")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                window
                    .get("duration")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
            })
            .unwrap_or(0);
        let unit = window
            .get("timeUnit")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let unit_norm = unit.trim_start_matches("TIME_UNIT_");

        let window_type = match (duration, unit_norm) {
            (300, "MINUTE") => "5h".to_string(),
            (0, _) => detail
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("window")
                .to_string(),
            _ => format!("{}{}", duration, unit_norm.chars().next().unwrap_or(' ')),
        };

        let unit_secs: i64 = match unit_norm {
            "SECOND" => 1,
            "MINUTE" => 60,
            "HOUR" => 3600,
            "DAY" => 86400,
            "WEEK" => 7 * 86400,
            _ => 0,
        };
        let period_seconds = if duration > 0 && unit_secs > 0 {
            Some(duration * unit_secs)
        } else {
            None
        };

        let entry_reset = parse_reset(&detail);
        windows.push(QuotaWindow {
            window_type,
            used,
            limit,
            remaining,
            reset_at: entry_reset,
            period_seconds,
        });
    }

    // totalQuota is the monthly Kimi-membership cap. Per the Kimi docs
    // (https://www.kimi.com/code/docs/en/kimi-code/membership.html), when
    // this is reached the entire Kimi Code quota is "frozen" regardless of
    // the 7d and 5h windows, which is why surfacing it matters.
    //
    // The server returns `used` on a 0/100+ scale, but the only signal
    // that actually matters is "any usage at all" — the raw `limit` is
    // meaningless, and whether used=1 or used=50, the result is the same:
    // the monthly cap is exhausted and Kimi Code is frozen. Collapse the
    // window to a binary state so the bar renders fully red (exhausted)
    // or fully green (available) instead of an unhelpful 1% / 50% bar.
    // The server also returns no reset timestamp, so the bar renders
    // without pacing or a "resets in X" hint.
    if let Some(total) = body.get("totalQuota").and_then(|v| v.as_object()) {
        let total_value = serde_json::Value::Object(total.clone());
        let raw_limit = num_field(&total_value, "limit");
        let raw_used = if total.contains_key("used") {
            num_field(&total_value, "used")
        } else {
            // No `used` field — derive from `remaining` against the raw
            // limit. The limit value only matters for this fallback math;
            // it is never surfaced in the emitted window.
            let remaining = num_field(&total_value, "remaining");
            (raw_limit - remaining).max(0)
        };
        let exhausted = raw_used > 0;
        if raw_limit > 0 {
            windows.push(QuotaWindow {
                window_type: "total_quota".to_string(),
                used: i64::from(exhausted),
                limit: 1,
                remaining: i64::from(!exhausted),
                reset_at: None,
                period_seconds: None,
            });
        }
    }

    ProviderQuota {
        plan_name: "Kimi Coding Plan".to_string(),
        windows,
        unlimited: false,
    }
}

#[async_trait]
impl crate::providers::Provider for KimiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Kimi
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = auth.credential.unwrap_token()?.to_string();

        match self.fetch_coding(&key).await {
            Ok(r) => Ok(r),
            Err(_) => self.fetch_payg(&key).await,
        }
    }

    fn auth_resolver(&self) -> &dyn AuthResolver {
        &*self.auth
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> serde_json::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/kimi")
            .join(name);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {}", path.display(), e));
        serde_json::from_str(&raw).expect("parse fixture json")
    }

    #[test]
    fn parses_live_coding_usages_fixture() {
        // Captured live from https://api.kimi.com/coding/v1/usages on 2026-07-11.
        // Response uses string-typed numbers ("100") and `resetTime` (not
        // `reset_at`), plus the TIME_UNIT_-prefixed time unit. The parser
        // accepts both via num_field() / trim_start_matches("TIME_UNIT_").
        let body = fixture("coding_usages_default.json");
        let quota = parse_coding_response(&body);

        assert_eq!(quota.plan_name, "Kimi Coding Plan");

        let weekly = quota
            .windows
            .iter()
            .find(|w| w.window_type == "weekly")
            .expect("weekly window");
        assert_eq!(weekly.limit, 100);
        assert_eq!(weekly.remaining, 100);
        assert!(weekly.reset_at.is_some(), "weekly reset parsed");
        assert_eq!(weekly.period_seconds, Some(7 * 86400));

        let five_h = quota
            .windows
            .iter()
            .find(|w| w.window_type == "5h")
            .expect("5h window");
        assert_eq!(five_h.limit, 100);
        assert_eq!(five_h.remaining, 100);
        assert_eq!(five_h.period_seconds, Some(5 * 3600));
        assert!(five_h.reset_at.is_some(), "5h reset parsed");

        // totalQuota is the monthly Kimi-membership cap (per
        // https://www.kimi.com/code/docs/en/kimi-code/membership.html).
        // Server returns it without a reset timestamp and the
        // 7d/5h windows both report 100/100 even when the billing
        // cycle is exhausted — which is why surfacing it matters.
        // The raw value is "any usage at all" — `used=1` of the
        // meaningless 100-limit collapses to a binary exhausted
        // signal so the bar renders fully red instead of 1% green.
        let total = quota
            .windows
            .iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window");
        assert_eq!(total.limit, 1, "raw 100-limit collapsed to binary 1");
        assert_eq!(total.used, 1, "non-zero used => exhausted");
        assert_eq!(total.remaining, 0);
        assert!(
            total.reset_at.is_none(),
            "totalQuota carries no server-provided reset timestamp"
        );
        assert!(
            total.period_seconds.is_none(),
            "no period_seconds without a reset hint; bar renders without pacing"
        );
    }

    #[test]
    fn parses_total_quota_as_available_when_used_zero() {
        // When the monthly membership cap is untouched, totalQuota
        // reports used=0 / remaining=100. Collapsed to binary:
        // limit=1, used=0, remaining=1 — bar renders fully green.
        let body = serde_json::json!({
            "usage": {"limit": "100", "remaining": "100", "resetTime": "2026-07-13T09:59:07Z"},
            "limits": [],
            "totalQuota": {"limit": "100", "used": "0", "remaining": "100"}
        });
        let quota = parse_coding_response(&body);
        let total = quota
            .windows
            .iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window");
        assert_eq!(total.limit, 1);
        assert_eq!(total.used, 0);
        assert_eq!(total.remaining, 1);
    }

    #[test]
    fn parses_total_quota_ignores_raw_limit_value() {
        // The raw `limit` value is irrelevant — only "any usage at all"
        // matters. Whether the server reports used=1/100 or used=50/100,
        // the binary output must be identical.
        let body_low = serde_json::json!({
            "usage": {"limit": "100", "remaining": "100"},
            "limits": [],
            "totalQuota": {"limit": "100", "used": "1", "remaining": "99"}
        });
        let body_high = serde_json::json!({
            "usage": {"limit": "100", "remaining": "100"},
            "limits": [],
            "totalQuota": {"limit": "100", "used": "50", "remaining": "50"}
        });
        let total_low = parse_coding_response(&body_low)
            .windows
            .into_iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window (low)");
        let total_high = parse_coding_response(&body_high)
            .windows
            .into_iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window (high)");
        assert_eq!((total_low.limit, total_low.used, total_low.remaining), (1, 1, 0));
        assert_eq!((total_high.limit, total_high.used, total_high.remaining), (1, 1, 0));
    }

    #[test]
    fn parses_total_quota_from_remaining_only_when_used_missing() {
        // Some /usages responses may omit `used` and only supply
        // `remaining`. Fallback math still derives exhaustion: any
        // tokens consumed (remaining < limit) is a non-zero used
        // signal, so the binary result is exhausted.
        let body = serde_json::json!({
            "usage": {"limit": "100", "remaining": "100"},
            "limits": [],
            "totalQuota": {"limit": "100", "remaining": "99"}
        });
        let total = parse_coding_response(&body)
            .windows
            .into_iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window");
        assert_eq!(total.limit, 1);
        assert_eq!(total.used, 1, "remaining<limit => exhausted");
        assert_eq!(total.remaining, 0);
    }

    #[test]
    fn parses_kimi_coding_usages_without_total_quota() {
        // Older / free-tier responses may omit totalQuota entirely.
        // Parser must not crash or emit a phantom window.
        let body = serde_json::json!({
            "usage": {"limit": 50, "remaining": 50, "reset_at": "2026-07-13T00:00:00Z"},
            "limits": [{
                "detail": {"limit": 20, "remaining": 20, "resetTime": "2026-07-11T14:00:00Z"},
                "window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"}
            }]
        });
        let quota = parse_coding_response(&body);
        assert_eq!(quota.windows.len(), 2);
        assert!(!quota.windows.iter().any(|w| w.window_type == "total_quota"));
    }

    #[test]
    fn parses_kimi_coding_usages() {
        let body = serde_json::json!({
            "usage": {
                "limit": 1_000_000i64,
                "used": 400_000i64,
                "remaining": 600_000i64,
                "name": "Weekly limit",
                "reset_at": "2026-04-14T00:00:00Z"
            },
            "limits": [{
                "detail": {
                    "limit": 200_000i64,
                    "used": 50_000i64,
                    "remaining": 150_000i64,
                    "name": "5h limit"
                },
                "window": {"duration": 300, "timeUnit": "MINUTE"}
            }]
        });
        let quota = parse_coding_response(&body);
        assert_eq!(quota.windows.len(), 2);
        let weekly = &quota.windows[0];
        assert_eq!(weekly.window_type, "weekly");
        assert_eq!(weekly.limit, 1_000_000);
        assert_eq!(weekly.used, 400_000);
        assert_eq!(weekly.remaining, 600_000);
        assert!(weekly.reset_at.is_some());
        let five = &quota.windows[1];
        assert_eq!(five.window_type, "5h");
        assert_eq!(five.used, 50_000);
        assert_eq!(five.remaining, 150_000);
    }
}
