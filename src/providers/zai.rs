use crate::auth::{AuthCredential, AuthResolver};
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;

pub struct ZaiProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl ZaiProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_quota(&self, key: &str) -> Result<ProviderResult> {
        let url = "https://api.z.ai/api/monitor/usage/quota/limit";
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
            let code = body.get("code").and_then(|c| c.as_i64());
            if code == Some(200) {
                let quota = self.parse_response(&body)?;
                return Ok(ProviderResult {
                    kind: ProviderKind::Zai,
                    status: ProviderStatus::Available { quota },
                    fetched_at: Utc::now(),
                    raw_response: Some(body),
                });
            }
        }

        let msg = body
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");

        Ok(ProviderResult {
            kind: ProviderKind::Zai,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: msg.to_string(),
                    console_url: Some("https://open.bigmodel.cn/finance-center/finance/pay".into()),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(body),
        })
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<ProviderQuota> {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct LimitEntry {
            #[serde(rename = "type", default)]
            limit_type: String,
            #[serde(rename = "rawType", default)]
            raw_type: Option<String>,
            #[serde(default)]
            usage: i64,
            #[serde(default)]
            current_value: i64,
            #[serde(default)]
            remaining: i64,
            #[serde(default)]
            percentage: i64,
            #[serde(rename = "next_reset_time", default)]
            next_reset_time: Option<i64>,
        }

        #[derive(Deserialize)]
        struct Data {
            #[serde(default)]
            level: String,
            #[serde(default)]
            limits: Vec<LimitEntry>,
        }

        let data: Data = serde_json::from_value(body.get("data").cloned().unwrap_or_default())
            .map_err(|e| crate::Error::Provider(format!("parse error: {}", e)))?;

        let windows: Vec<QuotaWindow> = data
            .limits
            .iter()
            .map(|l| {
                let used = l.usage - l.remaining;
                let raw = l.raw_type.as_deref().unwrap_or("");
                let (window_type, _, _) = match (raw, l.limit_type.as_str()) {
                    ("TOKENS_LIMIT", _) if l.limit_type.contains("5h") || (l.limit_type.contains("Token") && l.usage <= 2_000_000) => ("5h".to_string(), 3, 5),
                    ("TOKENS_LIMIT", _) if l.limit_type.contains("Weekly") => ("weekly".to_string(), 6, 7),
                    ("TIME_LIMIT", _) => ("monthly_mcp".to_string(), 5, 1),
                    _ => (l.limit_type.clone(), 0, 0),
                };
                QuotaWindow {
                    window_type,
                    used,
                    limit: l.usage,
                    remaining: l.remaining,
                    reset_at: l.next_reset_time.and_then(|t| Utc.timestamp_millis_opt(t).single()),
                }
            })
            .collect();

        Ok(ProviderQuota {
            plan_name: format!("Z.ai {}", data.level),
            windows,
            unlimited: false,
        })
    }
}

#[async_trait]
impl crate::providers::Provider for ZaiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Zai
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = match &auth.credential {
            AuthCredential::Bearer(k) => k.clone(),
            AuthCredential::Token(t) => t.clone(),
        };

        match self.fetch_quota(&key).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Zai,
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
