use crate::auth::{AuthCredential, AuthResolver};
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
                        }],
                        unlimited: false,
                    },
                },
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
            kind: ProviderKind::Kimi,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: msg.to_string(),
                    console_url: Some("https://platform.kimi.ai/".into()),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(body),
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
            let usage = body.get("usage").cloned().unwrap_or_default();
            let limits = body.get("limits").and_then(|l| l.as_array()).cloned().unwrap_or_default();

            let mut windows = Vec::new();

            let weekly_remaining = usage.get("remaining").and_then(|v| v.as_i64()).unwrap_or(0);
            let weekly_limit = usage.get("limit").and_then(|v| v.as_i64()).unwrap_or(0);
            let weekly_used = usage.get("used").and_then(|v| v.as_i64()).unwrap_or(0);
            let reset_at_str = usage.get("reset_at").and_then(|v| v.as_str());
            let reset_at = reset_at_str.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            windows.push(QuotaWindow {
                window_type: "weekly".to_string(),
                used: weekly_used,
                limit: weekly_limit,
                remaining: weekly_remaining,
                reset_at,
            });

            for limit_entry in limits {
                let detail = limit_entry.get("detail").cloned().unwrap_or_default();
                let window = limit_entry.get("window").cloned().unwrap_or_default();
                let remaining = detail.get("remaining").and_then(|v| v.as_i64()).unwrap_or(0);
                let limit = detail.get("limit").and_then(|v| v.as_i64()).unwrap_or(0);
                let used = limit - remaining;
                let duration = window.get("duration").and_then(|v| v.as_i64()).unwrap_or(300);
                let unit = window.get("timeUnit").and_then(|v| v.as_str()).unwrap_or("MINUTE");

                let window_type = if duration == 300 && unit == "MINUTE" {
                    "5h".to_string()
                } else {
                    format!("{} {}", duration, unit)
                };

                windows.push(QuotaWindow {
                    window_type,
                    used,
                    limit,
                    remaining,
                    reset_at: None,
                });
            }

            return Ok(ProviderResult {
                kind: ProviderKind::Kimi,
                status: ProviderStatus::Available {
                    quota: ProviderQuota {
                        plan_name: "Kimi Coding Plan".to_string(),
                        windows,
                        unlimited: false,
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
            });
        }

        self.fetch_payg(key).await
    }
}

#[async_trait]
impl crate::providers::Provider for KimiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Kimi
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = match &auth.credential {
            AuthCredential::Bearer(k) => k.clone(),
            AuthCredential::Token(t) => t.clone(),
        };

        match self.fetch_coding(&key).await {
            Ok(r) => Ok(r),
            Err(_) => {
                self.fetch_payg(&key).await
            }
        }
    }

    fn auth_resolver(&self) -> &dyn AuthResolver {
        &*self.auth
    }
}
