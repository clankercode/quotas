use crate::auth::{AuthCredential, AuthResolver};
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use reqwest::Client;

pub struct CopilotProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl CopilotProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch(&self, token: &str) -> Result<ProviderResult> {
        let url = "https://api.github.com/copilot_internal/user";
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("token {}", token))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("editor-version", "vscode/1.99.1")
            .header("editor-plugin-version", "copilot-chat/0.26.7")
            .header("user-agent", "GitHubCopilotChat/0.26.7")
            .header("x-github-api-version", "2025-04-01")
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 200 {
            let plan = body
                .get("copilot_plan")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            let reset_date_str = body
                .get("quota_reset_date")
                .and_then(|v| v.as_str());

            let reset_at = reset_date_str.and_then(|s| {
                NaiveDate::parse_from_str(s, "%Y-%m-%d")
                    .ok()
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .map(|dt| Utc.from_utc_datetime(&dt))
            });

            let snapshots = body.get("quota_snapshots").cloned().unwrap_or_default();
            let premium = snapshots.get("premium_interactions").cloned().unwrap_or_default();

            let entitlement = premium.get("entitlement").and_then(|v| v.as_i64()).unwrap_or(0);
            let _remaining = premium.get("remaining").and_then(|v| v.as_i64()).unwrap_or(0);
            let remaining_precise = premium.get("quota_remaining").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let _percent = premium.get("percent_remaining").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let unlimited = premium.get("unlimited").and_then(|v| v.as_bool()).unwrap_or(false);

            let windows = if unlimited {
                vec![QuotaWindow {
                    window_type: "monthly".to_string(),
                    used: 0,
                    limit: 0,
                    remaining: 0,
                    reset_at,
                }]
            } else {
                vec![QuotaWindow {
                    window_type: "monthly".to_string(),
                    used: entitlement - remaining_precise as i64,
                    limit: entitlement,
                    remaining: remaining_precise as i64,
                    reset_at,
                }]
            };

            return Ok(ProviderResult {
                kind: ProviderKind::Copilot,
                status: ProviderStatus::Available {
                    quota: ProviderQuota {
                        plan_name: format!("GitHub Copilot {}", plan.replace("_", " ")),
                        windows,
                        unlimited,
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
            });
        }

        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");

        Ok(ProviderResult {
            kind: ProviderKind::Copilot,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: msg.to_string(),
                    console_url: Some("https://github.com/settings/copilot".into()),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(body),
        })
    }
}

#[async_trait]
impl crate::providers::Provider for CopilotProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Copilot
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let token = match &auth.credential {
            AuthCredential::Bearer(k) => k.clone(),
            AuthCredential::Token(t) => t.clone(),
        };

        match self.fetch(&token).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Copilot,
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
