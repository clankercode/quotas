use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
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

    async fn fetch(&self, token: &str, use_oauth: bool) -> Result<ProviderResult> {
        let url = "https://chatgpt.com/backend-api/wham/usage";
        let auth_header = if use_oauth {
            format!("Bearer {}", token)
        } else {
            token.to_string()
        };

        let resp = self
            .http
            .get(url)
            .header("Authorization", auth_header)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 200 {
            let quota = parse_usage(&body);
            return Ok(ProviderResult {
                kind: ProviderKind::Codex,
                status: ProviderStatus::Available { quota },
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
