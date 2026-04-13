use crate::auth::{AuthCredential, AuthResolver};
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
        })
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

    let rate_limit_obj = body.get("rate_limit").cloned().unwrap_or_default();
    let primary = rate_limit_obj
        .get("primary_window")
        .cloned()
        .unwrap_or_default();
    let secondary = rate_limit_obj
        .get("secondary_window")
        .cloned()
        .unwrap_or_default();

    let mut windows = Vec::new();

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
        windows.push(QuotaWindow {
            window_type: format!("{}h", primary_window_sec / 3600),
            used: primary_pct,
            limit: 100,
            remaining: 100 - primary_pct,
            reset_at: Utc.timestamp_opt(primary_reset, 0).single(),
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
        windows.push(QuotaWindow {
            window_type: format!("{}d", secondary_window_sec / 86400),
            used: secondary_pct,
            limit: 100,
            remaining: 100 - secondary_pct,
            reset_at: Utc.timestamp_opt(secondary_reset, 0).single(),
        });
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
        let token = match &auth.credential {
            AuthCredential::Bearer(k) => k.clone(),
            AuthCredential::Token(t) => t.clone(),
        };

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
}
