use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;

pub struct CodexProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl CodexProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    fn auth_header(token: &str, use_oauth: bool) -> String {
        if use_oauth {
            format!("Bearer {}", token)
        } else {
            token.to_string()
        }
    }

    async fn fetch(&self, token: &str, use_oauth: bool) -> Result<ProviderResult> {
        let url = "https://chatgpt.com/backend-api/wham/usage";
        let auth_header = Self::auth_header(token, use_oauth);

        let resp = self
            .http
            .get(url)
            .header("Authorization", &auth_header)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 200 {
            let mut quota = parse_usage(&body);
            // Enrich with per-credit title/expiry when any banked resets exist.
            // Best-effort: keep the usage summary count if detail fails or is sparse.
            // Always retain the detail body in raw_response when we got one so the
            // detail view's raw section can show it after the main usage payload.
            let mut detail_raw: Option<serde_json::Value> = None;
            if quota
                .banked_resets
                .as_ref()
                .is_some_and(|b| b.available_count > 0)
            {
                if let Ok(Some(detail_body)) =
                    self.fetch_reset_credits_detail(&auth_header).await
                {
                    if let Some(detail) = parse_banked_reset_credits_detail(&detail_body) {
                        merge_banked_reset_detail(&mut quota, detail);
                    }
                    detail_raw = Some(detail_body);
                }
            }
            let raw_response = Some(match detail_raw {
                Some(detail) => serde_json::json!({
                    "usage": body,
                    "rate_limit_reset_credits": detail,
                }),
                None => body,
            });
            return Ok(ProviderResult {
                kind: ProviderKind::Codex,
                status: ProviderStatus::Available { quota },
                fetched_at: Utc::now(),
                raw_response,
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
            kind: ProviderKind::Codex,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: format!("{} (status {})", msg, status.as_u16()),
                    console_url: Some("https://chatgpt.com/codex/console".into()),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
            cached_at: None,
        })
    }

    /// Fetch the banked-reset detail payload. Returns the raw JSON body on
    /// success so callers can both parse structured credits and retain the
    /// body for the detail view's multi-part raw section.
    async fn fetch_reset_credits_detail(
        &self,
        auth_header: &str,
    ) -> Result<Option<serde_json::Value>> {
        let url = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
        let resp = self
            .http
            .get(url)
            .header("Authorization", auth_header)
            .header("Accept", "application/json")
            .header("originator", "Codex Desktop")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let body: serde_json::Value = resp.json().await?;
        Ok(Some(body))
    }
}

/// Extract the usage payload from a Codex `raw_response`.
///
/// New fetches that also retrieved banked-reset detail store a multi-part
/// envelope `{ "usage": …, "rate_limit_reset_credits": … }`. Older cache
/// entries (and fetches without detail) keep the flat usage body.
pub(crate) fn usage_body_from_raw(raw: &serde_json::Value) -> &serde_json::Value {
    raw.get("usage")
        .filter(|u| u.is_object())
        .unwrap_or(raw)
}

/// Format a Codex rate-limit window duration for display keys.
///
/// Prefer whole-day labels when the period is an exact multiple of a day
/// (e.g. 604800s → `7d` rather than `168h`). Fall back to hours for the
/// classic 5h primary window, then raw seconds.
fn format_window_duration(seconds: i64) -> String {
    if seconds >= 86400 && seconds % 86400 == 0 {
        format!("{}d", seconds / 86400)
    } else if seconds >= 3600 && seconds % 3600 == 0 {
        format!("{}h", seconds / 3600)
    } else if seconds > 0 {
        format!("{}s", seconds)
    } else {
        "0h".to_string()
    }
}

fn push_rate_limit_windows(
    windows: &mut Vec<QuotaWindow>,
    rate_limit: &serde_json::Value,
    label_prefix: &str,
) {
    let primary = rate_limit
        .get("primary_window")
        .cloned()
        .unwrap_or_default();
    let secondary = rate_limit
        .get("secondary_window")
        .cloned()
        .unwrap_or_default();

    let primary_pct = primary
        .get("used_percent")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let primary_reset = primary
        .get("reset_at")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let primary_window_sec = primary
        .get("limit_window_seconds")
        .and_then(|v| v.as_i64())
        .unwrap_or(18000);

    if primary_reset > 0 || primary_pct > 0 {
        let base = format_window_duration(primary_window_sec);
        let label = if label_prefix.is_empty() {
            base
        } else {
            format!("{}/{}", label_prefix, base)
        };
        windows.push(QuotaWindow {
            window_type: label,
            used: primary_pct,
            limit: 100,
            remaining: 100 - primary_pct,
            reset_at: Utc.timestamp_opt(primary_reset, 0).single(),
            period_seconds: Some(primary_window_sec),
        });
    }

    let secondary_pct = secondary
        .get("used_percent")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let secondary_reset = secondary
        .get("reset_at")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let secondary_window_sec = secondary
        .get("limit_window_seconds")
        .and_then(|v| v.as_i64())
        .unwrap_or(604800);

    if secondary_reset > 0 || secondary_pct > 0 {
        let base = format_window_duration(secondary_window_sec);
        let label = if label_prefix.is_empty() {
            base
        } else {
            format!("{}/{}", label_prefix, base)
        };
        windows.push(QuotaWindow {
            window_type: label,
            used: secondary_pct,
            limit: 100,
            remaining: 100 - secondary_pct,
            reset_at: Utc.timestamp_opt(secondary_reset, 0).single(),
            period_seconds: Some(secondary_window_sec),
        });
    }
}

fn short_codex_label(name: &str) -> String {
    let trimmed = name
        .strip_prefix("GPT-5.3-")
        .or_else(|| name.strip_prefix("gpt-5.3-"))
        .unwrap_or(name);
    let lower = trimmed.to_ascii_lowercase();
    let label: String = lower
        .rsplit('-')
        .next()
        .unwrap_or(&lower)
        .chars()
        .take(10)
        .collect();
    if label == "spark" {
        "spk".to_string()
    } else {
        label
    }
}

pub(crate) fn parse_usage(body: &serde_json::Value) -> ProviderQuota {
    let plan_type = body
        .get("plan_type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let credits_obj = body.get("credits").cloned().unwrap_or_default();
    let unlimited = credits_obj
        .get("unlimited")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let balance_str = credits_obj
        .get("balance")
        .and_then(|v| v.as_str())
        .unwrap_or("0");

    let mut windows = Vec::new();

    let rate_limit_obj = body.get("rate_limit").cloned().unwrap_or_default();
    push_rate_limit_windows(&mut windows, &rate_limit_obj, "");

    // Per-model sub-limits (e.g., GPT-5.3-Codex-Spark).
    if let Some(extras) = body
        .get("additional_rate_limits")
        .and_then(|v| v.as_array())
    {
        for entry in extras {
            let raw_label = entry
                .get("limit_name")
                .and_then(|v| v.as_str())
                .or_else(|| entry.get("metered_feature").and_then(|v| v.as_str()))
                .unwrap_or("extra");
            let prefix = short_codex_label(raw_label);
            if let Some(rl) = entry.get("rate_limit") {
                push_rate_limit_windows(&mut windows, rl, &prefix);
            }
        }
    }

    let balance_value = balance_str.parse::<f64>().unwrap_or(0.0);
    if balance_value > 0.0 {
        let balance_cents = (balance_value * 100.0) as i64;
        windows.push(QuotaWindow {
            window_type: "credits".to_string(),
            used: 0,
            limit: 0,
            remaining: balance_cents,
            reset_at: None,
            period_seconds: None,
        });
    }

    ProviderQuota {
        plan_name: format!("Codex / ChatGPT {}", plan_type),
        windows,
        unlimited,
        banked_resets: parse_banked_resets_summary(body),
    }
}

/// Summary from `wham/usage` (`rate_limit_reset_credits.available_count` only).
fn parse_banked_resets_summary(body: &serde_json::Value) -> Option<crate::providers::BankedResets> {
    let summary = body.get("rate_limit_reset_credits")?;
    let available_count = summary.get("available_count")?.as_i64()?;
    // Always attach when the field is present so a 0-count can clear UI state
    // after the last credit is spent (optional: hide 0 in the TUI).
    Some(crate::providers::BankedResets {
        available_count,
        credits: Vec::new(),
    })
}

/// Full list from `GET …/wham/rate-limit-reset-credits`.
///
/// Returns `None` for empty/unusable payloads so callers keep the usage-summary
/// count instead of clobbering it with zeros.
pub(crate) fn parse_banked_reset_credits_detail(
    body: &serde_json::Value,
) -> Option<crate::providers::BankedResets> {
    let count_field = body.get("available_count").and_then(|v| v.as_i64());
    let mut credits = Vec::new();
    if let Some(arr) = body.get("credits").and_then(|v| v.as_array()) {
        for c in arr {
            let id = c
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let status = c
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let parse_ts = |key: &str| {
                c.get(key)
                    .and_then(|v| v.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc))
            };
            credits.push(crate::providers::BankedResetCredit {
                id,
                status,
                title: c
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                description: c
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                granted_at: parse_ts("granted_at"),
                expires_at: parse_ts("expires_at"),
                source: c
                    .get("profile_user_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            });
        }
    }
    // Require a real signal — bare `{}` must not wipe the usage summary.
    if count_field.is_none() && credits.is_empty() {
        return None;
    }
    Some(crate::providers::BankedResets {
        available_count: count_field.unwrap_or(credits.len() as i64),
        credits,
    })
}

/// Merge detail endpoint into usage-derived banked resets.
/// Usage `available_count` stays authoritative when detail omits/zeros it
/// without providing credit rows.
pub(crate) fn merge_banked_reset_detail(
    quota: &mut ProviderQuota,
    detail: crate::providers::BankedResets,
) {
    let detail_count = detail.available_count;
    let detail_credits = detail.credits;

    let Some(br) = quota.banked_resets.as_mut() else {
        if detail_count > 0 || !detail_credits.is_empty() {
            quota.banked_resets = Some(crate::providers::BankedResets {
                available_count: if detail_count > 0 {
                    detail_count
                } else {
                    detail_credits.len() as i64
                },
                credits: detail_credits,
            });
        }
        return;
    };

    if !detail_credits.is_empty() {
        br.credits = detail_credits;
    }
    // Non-zero detail count is authoritative; never zero a good usage summary
    // just because the detail payload was sparse.
    if detail_count > 0 {
        br.available_count = detail_count;
    }
}

#[async_trait]
impl crate::providers::Provider for CodexProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let token = auth.credential.unwrap_token()?.to_string();

        let use_oauth =
            matches!(&auth.source[..], s if s.contains("oauth") || s.contains(".codex"));

        match self.fetch(&token, use_oauth).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Codex,
                status: ProviderStatus::NetworkError {
                    message: e.to_string(),
                },
                fetched_at: Utc::now(),
                raw_response: None,
                auth_source: None,
                cached_at: None,
            }),
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
            .join("tests/fixtures/codex")
            .join(name);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {}", path.display(), e));
        serde_json::from_str(&raw).expect("parse fixture json")
    }

    #[test]
    fn format_window_duration_prefers_days_over_hours() {
        assert_eq!(format_window_duration(18000), "5h");
        assert_eq!(format_window_duration(604800), "7d");
        assert_eq!(format_window_duration(86400), "1d");
        assert_eq!(format_window_duration(90), "90s");
    }

    #[test]
    fn parses_codex_wham_usage() {
        let body = serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 23,
                    "limit_window_seconds": 18000,
                    "reset_after_seconds": 12345,
                    "reset_at": 1744502400i64
                },
                "secondary_window": {
                    "used_percent": 45,
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 302400,
                    "reset_at": 1744934400i64
                }
            },
            "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": "42.50"
            }
        });
        let quota = parse_usage(&body);
        assert!(quota.plan_name.contains("plus"));
        assert_eq!(quota.windows.len(), 3);
        assert_eq!(quota.windows[0].window_type, "5h");
        assert_eq!(quota.windows[0].used, 23);
        assert_eq!(quota.windows[1].window_type, "7d");
        assert_eq!(quota.windows[2].window_type, "credits");
        assert_eq!(quota.windows[2].remaining, 4250);
    }

    #[test]
    fn parses_live_wham_usage_fixture_primary_week_only() {
        // Captured 2026-07-13 from chatgpt.com/backend-api/wham/usage.
        // Pro plans currently expose a single primary window of 604800s
        // (7 days) with secondary_window=null — previously this rendered
        // as the ugly "168h" label (604800/3600).
        let body = fixture("wham_usage_live.json");
        let quota = parse_usage(&body);

        assert!(quota.plan_name.contains("pro"));
        assert!(
            !quota.windows.iter().any(|w| w.window_type.contains("168h")),
            "must not emit 168h labels: {:?}",
            quota.windows.iter().map(|w| &w.window_type).collect::<Vec<_>>()
        );

        let week = quota
            .windows
            .iter()
            .find(|w| w.window_type == "7d")
            .expect("7d primary window");
        assert_eq!(week.used, 76);
        assert_eq!(week.limit, 100);
        assert_eq!(week.remaining, 24);
        assert_eq!(week.period_seconds, Some(604800));
        assert!(week.reset_at.is_some());

        let spark = quota
            .windows
            .iter()
            .find(|w| w.window_type == "spk/7d")
            .expect("spk/7d additional limit");
        assert_eq!(spark.used, 0);
        assert_eq!(spark.limit, 100);
        assert_eq!(spark.period_seconds, Some(604800));

        // Summary-only banked resets from usage payload.
        let banked = quota.banked_resets.expect("rate_limit_reset_credits on live");
        assert_eq!(banked.available_count, 2);
        assert!(
            banked.credits.is_empty(),
            "usage payload has count only; detail is a separate endpoint"
        );
    }

    #[test]
    fn parses_live_rate_limit_reset_credits_detail() {
        let body = fixture("rate_limit_reset_credits_live.json");
        let banked = parse_banked_reset_credits_detail(&body).expect("detail");
        assert_eq!(banked.available_count, 2);
        assert_eq!(banked.credits.len(), 2);
        assert_eq!(banked.credits[0].status, "available");
        assert_eq!(banked.credits[0].title.as_deref(), Some("Full reset"));
        assert!(banked.credits[0].expires_at.is_some());
        assert_eq!(
            banked.credits[0].source.as_deref(),
            Some("Codex Team")
        );
        assert!(banked.credits[0]
            .description
            .as_deref()
            .unwrap_or("")
            .contains("rate limit reset"));
    }

    #[test]
    fn parse_usage_omits_banked_when_field_missing() {
        let body = serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 1,
                    "limit_window_seconds": 18000,
                    "reset_at": 1744502400i64
                }
            },
            "credits": { "balance": "0", "unlimited": false }
        });
        let quota = parse_usage(&body);
        assert!(quota.banked_resets.is_none());
    }

    #[test]
    fn sparse_detail_payload_does_not_parse() {
        assert!(parse_banked_reset_credits_detail(&serde_json::json!({})).is_none());
    }

    #[test]
    fn usage_body_from_raw_unwraps_multipart_envelope() {
        let usage = serde_json::json!({
            "plan_type": "pro",
            "rate_limit_reset_credits": { "available_count": 2 }
        });
        let envelope = serde_json::json!({
            "usage": usage,
            "rate_limit_reset_credits": { "available_count": 2, "credits": [] }
        });
        assert_eq!(usage_body_from_raw(&envelope)["plan_type"], "pro");
        // Flat legacy body is returned as-is.
        assert_eq!(usage_body_from_raw(&usage)["plan_type"], "pro");
    }

    #[test]
    fn merge_detail_keeps_usage_count_when_detail_count_zero() {
        let mut quota = ProviderQuota {
            plan_name: "x".into(),
            windows: vec![],
            unlimited: false,
            banked_resets: Some(crate::providers::BankedResets {
                available_count: 2,
                credits: vec![],
            }),
        };
        merge_banked_reset_detail(
            &mut quota,
            crate::providers::BankedResets {
                available_count: 0,
                credits: vec![crate::providers::BankedResetCredit {
                    id: "c1".into(),
                    status: "available".into(),
                    title: Some("Full reset".into()),
                    description: None,
                    granted_at: None,
                    expires_at: None,
                    source: None,
                }],
            },
        );
        let br = quota.banked_resets.unwrap();
        assert_eq!(br.available_count, 2, "usage count must stay");
        assert_eq!(br.credits.len(), 1);
        assert_eq!(br.credits[0].title.as_deref(), Some("Full reset"));
    }

    #[test]
    fn primary_week_window_labels_as_7d_not_168h() {
        let body = serde_json::json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 10,
                    "limit_window_seconds": 604800,
                    "reset_at": 1744934400i64
                },
                "secondary_window": null
            },
            "credits": { "balance": "0", "unlimited": false }
        });
        let quota = parse_usage(&body);
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].window_type, "7d");
    }
}
