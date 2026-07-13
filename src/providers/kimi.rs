use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Datelike, TimeZone, Utc};
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
                        banked_resets: None,
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
            let quota = parse_coding_response(&body, Utc::now());
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

/// Next calendar-month boundary at 00:00 UTC (1st of next month). Used
/// as a best-effort reset anchor for the monthly `totalQuota` window
/// since the `/usages` endpoint doesn't return a server-side reset
/// timestamp for it.
///
/// Caveat: a live before/after pair on 2026-07-11 (frozen) → 2026-07-13/14
/// (healthy post-reset) shows the monthly cap reset mid-month, so
/// anniversary billing is more likely than a calendar-month boundary.
/// Without a server timestamp the hint can be off by weeks; it is only
/// a rough "somewhere this month-ish" cue.
fn next_month_reset(now: DateTime<Utc>) -> DateTime<Utc> {
    let (year, month) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .expect("1st of any month at 00:00 UTC is always a valid DateTime")
}

pub(crate) fn parse_coding_response(body: &serde_json::Value, now: DateTime<Utc>) -> ProviderQuota {
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
    // Empirically confirmed by a before/after pair on the same account:
    //
    //   frozen  (2026-07-11, 403 access_terminated): totalQuota {limit:100, used:1, remaining:99}
    //   healthy (2026-07-13/14, post monthly reset):  totalQuota {limit:100, remaining:99}
    //                                                 ^^^ no `used` field
    //
    // So (a) freeze is signalled by `used` present and >0, (b) `remaining`
    // is sticky at 99 across both states and must be ignored entirely —
    // never derive exhaustion from `limit - remaining`, and (c) raw `limit`
    // is likewise meaningless. Collapse to a binary bar (fully red /
    // fully green). Healthy payloads often omit `used` rather than
    // sending `used:0`; that still means available.
    //
    // The server returns no reset timestamp. We synthesize the next
    // calendar-month boundary (1st of next month, 00:00 UTC) as a
    // best-effort "resets in Xd Yh" hint. Caveat: this account's monthly
    // reset landed mid-month (~2026-07-14), so anniversary billing is
    // more likely than calendar month — the hint can be off by weeks.
    // `period_seconds` stays None so the binary bar doesn't get an
    // overspend/slack overlay based on elapsed calendar time.
    if let Some(total) = body.get("totalQuota").and_then(|v| v.as_object()) {
        let total_value = serde_json::Value::Object(total.clone());
        // Emit when we have any totalQuota usage signal. `used` is the
        // only freeze indicator; `remaining` alone still emits a green
        // (available) bar so the card surfaces the monthly window, but
        // never implies exhaustion. With neither field, skip entirely.
        let has_used = total.contains_key("used");
        let has_remaining = total.contains_key("remaining");
        if has_used || has_remaining {
            let exhausted = has_used && num_field(&total_value, "used") > 0;
            windows.push(QuotaWindow {
                window_type: "total_quota".to_string(),
                used: i64::from(exhausted),
                limit: 1,
                remaining: i64::from(!exhausted),
                reset_at: Some(next_month_reset(now)),
                period_seconds: None,
            });
        }
    }

    ProviderQuota {
        plan_name: "Kimi Coding Plan".to_string(),
        windows,
        unlimited: false,
        banked_resets: None,
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

    /// Fixed "now" used across the test suite so the synthesized
    /// calendar-month reset is deterministic. Mid-July 2026 keeps it
    /// near the live fixture's capture date so the assertions stay
    /// readable. `parse_from_rfc3339` isn't const-stable, so we wrap
    /// in a function — parse cost is negligible for tests.
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-12T10:00:00+00:00")
            .expect("invalid NOW constant")
            .with_timezone(&Utc)
    }

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
        // Captured live from https://api.kimi.com/coding/v1/usages on 2026-07-11
        // while the account was FROZEN (403 access_terminated_error on coding
        // requests). Response uses string-typed numbers ("100") and `resetTime`
        // (not `reset_at`), plus the TIME_UNIT_-prefixed time unit. The parser
        // accepts both via num_field() / trim_start_matches("TIME_UNIT_").
        //
        // totalQuota: {limit:100, used:1, remaining:99} — `used:1` is the
        // freeze flag; remaining is sticky/misleading (see post_reset fixture).
        let body = fixture("coding_usages_default.json");
        let quota = parse_coding_response(&body, now());

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
        // 7d/5h both report 100/100 even when frozen — which is why
        // surfacing totalQuota matters. `used=1` collapses to binary
        // exhausted so the bar renders fully red instead of 1% green.
        // The reset timestamp is synthesized to the next calendar
        // month boundary (1st of next month, 00:00 UTC) so the bar
        // shows a "resets in Xd Yh" hint (approximate — see next_month_reset).
        let total = quota
            .windows
            .iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window");
        assert_eq!(total.limit, 1, "raw 100-limit collapsed to binary 1");
        assert_eq!(total.used, 1, "used present and >0 => exhausted");
        assert_eq!(total.remaining, 0);
        // NOW is 2026-07-12 → next month is August 2026.
        assert_eq!(
            total.reset_at,
            Some(DateTime::parse_from_rfc3339("2026-08-01T00:00:00+00:00").unwrap().with_timezone(&Utc)),
            "synthesized calendar-month reset at next month 1st, 00:00 UTC"
        );
        assert!(
            total.period_seconds.is_none(),
            "binary window: no period_seconds so the bar doesn't draw overspend/slack against elapsed calendar time"
        );
    }

    #[test]
    fn parses_live_coding_usages_post_reset_fixture() {
        // Captured live on 2026-07-13/14 right after the monthly membership
        // quota reset (same account as coding_usages_default.json). Healthy:
        // totalQuota is {limit:100, remaining:99} with `used` OMITTED entirely.
        // remaining is still 99 — identical to the frozen capture — so it is
        // not a usage counter. Only the presence/absence of `used` distinguishes
        // frozen from available. Weekly resetTime advanced by 7 days.
        let body = fixture("coding_usages_post_reset.json");
        let quota = parse_coding_response(&body, now());

        assert_eq!(quota.plan_name, "Kimi Coding Plan");

        let weekly = quota
            .windows
            .iter()
            .find(|w| w.window_type == "weekly")
            .expect("weekly window");
        assert_eq!(weekly.limit, 100);
        assert_eq!(weekly.remaining, 100);
        assert_eq!(
            weekly.reset_at,
            Some(
                DateTime::parse_from_rfc3339("2026-07-20T09:59:07.479363Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );

        let five_h = quota
            .windows
            .iter()
            .find(|w| w.window_type == "5h")
            .expect("5h window");
        assert_eq!(five_h.remaining, 100);

        let total = quota
            .windows
            .iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window");
        assert_eq!(total.limit, 1, "collapsed to binary");
        assert_eq!(
            total.used, 0,
            "remaining-only payload (no used field) must be available, not derived as limit-remaining=1"
        );
        assert_eq!(total.remaining, 1);
        assert_eq!(
            total.reset_at,
            Some(DateTime::parse_from_rfc3339("2026-08-01T00:00:00+00:00").unwrap().with_timezone(&Utc))
        );
        assert!(total.period_seconds.is_none());
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
        let quota = parse_coding_response(&body, now());
        let total = quota
            .windows
            .iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window");
        assert_eq!(total.limit, 1);
        assert_eq!(total.used, 0);
        assert_eq!(total.remaining, 1);
        assert_eq!(
            total.reset_at,
            Some(DateTime::parse_from_rfc3339("2026-08-01T00:00:00+00:00").unwrap().with_timezone(&Utc))
        );
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
        let total_low = parse_coding_response(&body_low, now())
            .windows
            .into_iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window (low)");
        let total_high = parse_coding_response(&body_high, now())
            .windows
            .into_iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window (high)");
        assert_eq!((total_low.limit, total_low.used, total_low.remaining), (1, 1, 0));
        assert_eq!((total_high.limit, total_high.used, total_high.remaining), (1, 1, 0));
        assert_eq!(total_low.reset_at, total_high.reset_at);
    }

    #[test]
    fn parses_total_quota_from_remaining_only_when_used_missing() {
        // Live post-reset payloads omit `used` and only supply
        // `remaining` (stuck at 99). That shape is HEALTHY — do NOT
        // derive exhaustion from limit - remaining. Only an explicit
        // used>0 freezes the binary bar.
        let body = serde_json::json!({
            "usage": {"limit": "100", "remaining": "100"},
            "limits": [],
            "totalQuota": {"limit": "100", "remaining": "99"}
        });
        let total = parse_coding_response(&body, now())
            .windows
            .into_iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window");
        assert_eq!(total.limit, 1);
        assert_eq!(total.used, 0, "remaining-only => available (not limit-remaining)");
        assert_eq!(total.remaining, 1);
        assert_eq!(
            total.reset_at,
            Some(DateTime::parse_from_rfc3339("2026-08-01T00:00:00+00:00").unwrap().with_timezone(&Utc))
        );
    }

    #[test]
    fn total_quota_without_used_or_remaining_emits_no_window() {
        // A totalQuota object carrying no usage data (only `limit`, or
        // nothing) must NOT emit a window — asserting "frozen" here would
        // spuriously paint the card red on a payload with no usage signal.
        let body = serde_json::json!({
            "usage": {"limit": "100", "remaining": "100"},
            "limits": [],
            "totalQuota": {"limit": "100"}
        });
        let quota = parse_coding_response(&body, now());
        assert!(
            !quota.windows.iter().any(|w| w.window_type == "total_quota"),
            "no usage data => no total_quota window"
        );
    }

    #[test]
    fn total_quota_without_limit_still_emits_from_used() {
        // `limit` is meaningless; presence of `used` alone must still emit.
        let body = serde_json::json!({
            "usage": {"limit": "100", "remaining": "100"},
            "limits": [],
            "totalQuota": {"used": "1", "remaining": "99"}
        });
        let total = parse_coding_response(&body, now())
            .windows
            .into_iter()
            .find(|w| w.window_type == "total_quota")
            .expect("total_quota window from `used` without `limit`");
        assert_eq!(total.limit, 1);
        assert_eq!(total.used, 1);
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
        let quota = parse_coding_response(&body, now());
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
        let quota = parse_coding_response(&body, now());
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

    #[test]
    fn next_month_reset_returns_first_of_next_month_mid_month() {
        let now = DateTime::parse_from_rfc3339("2026-07-12T10:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let reset = next_month_reset(now);
        assert_eq!(
            reset,
            DateTime::parse_from_rfc3339("2026-08-01T00:00:00+00:00")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn next_month_reset_rolls_year_on_december() {
        let now = DateTime::parse_from_rfc3339("2026-12-31T23:59:59+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let reset = next_month_reset(now);
        assert_eq!(
            reset,
            DateTime::parse_from_rfc3339("2027-01-01T00:00:00+00:00")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn next_month_reset_on_first_day_returns_next_month() {
        // Exactly at the boundary — the current cycle just reset, so
        // the next reset is still next month, not now.
        let now = DateTime::parse_from_rfc3339("2026-07-01T00:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let reset = next_month_reset(now);
        assert_eq!(
            reset,
            DateTime::parse_from_rfc3339("2026-08-01T00:00:00+00:00")
                .unwrap()
                .with_timezone(&Utc)
        );
    }
}
