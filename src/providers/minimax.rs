use crate::auth::{AuthCredential, AuthResolver, MultiResolver};
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::{Error, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;

pub struct MinimaxProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl MinimaxProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    pub fn with_multi_resolver(auth: MultiResolver) -> Self {
        Self {
            http: Client::new(),
            auth: Box::new(auth),
        }
    }

    async fn fetch_intl(&self, key: &str) -> Result<ProviderResult> {
        let url = "https://api.minimax.io/v1/api/openplatform/coding_plan/remains";
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", key))
            .header("Content-Type", "application/json")
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 200 {
            let code = body.get("base_resp").and_then(|b| b.get("status_code"));
            if code == Some(&serde_json::json!(0)) || code.is_none() {
                let quota = self.parse_response(&body)?;
                return Ok(ProviderResult {
                    kind: ProviderKind::Minimax,
                    status: ProviderStatus::Available { quota },
                    fetched_at: Utc::now(),
                    raw_response: Some(body),
                });
            }
        }

        let msg = body
            .get("base_resp")
            .and_then(|b| b.get("status_msg"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");

        Ok(ProviderResult {
            kind: ProviderKind::Minimax,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: msg.to_string(),
                    console_url: Some("https://platform.minimax.io/user-center/payment/coding-plan".into()),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(body),
        })
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<ProviderQuota> {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct ModelRemain {
            model_name: String,
            start_time: i64,
            end_time: i64,
            remains_time: i64,
            #[serde(rename = "current_interval_total_count")]
            total_count: i64,
            #[serde(rename = "current_interval_usage_count")]
            usage_count: i64,
        }

        #[derive(Deserialize)]
        struct Response {
            model_remains: Vec<ModelRemain>,
        }

        let resp: Response = serde_json::from_value(body.clone())
            .map_err(|e| Error::Provider(format!("parse error: {}", e)))?;

        let windows: Vec<QuotaWindow> = resp
            .model_remains
            .iter()
            .map(|m| {
                let remaining = m.usage_count;
                let used = m.total_count - m.usage_count;
                QuotaWindow {
                    window_type: "5h".to_string(),
                    used,
                    limit: m.total_count,
                    remaining,
                    reset_at: Utc.timestamp_millis_opt(m.end_time).single(),
                }
            })
            .collect();

        let plan_name = resp
            .model_remains
            .first()
            .map(|m| m.model_name.clone())
            .unwrap_or_else(|| "Token Plan".to_string());

        Ok(ProviderQuota {
            plan_name,
            windows,
            unlimited: false,
        })
    }
}

#[async_trait]
impl crate::providers::Provider for MinimaxProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Minimax
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = match &auth.credential {
            AuthCredential::Bearer(k) => k.clone(),
            AuthCredential::Token(t) => t.clone(),
        };

        match self.fetch_intl(&key).await {
            Ok(mut r) => {
                r.raw_response = Some(r.raw_response.unwrap_or_default());
                Ok(r)
            }
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Minimax,
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
