use crate::auth::{AuthCredential, AuthResolver};
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;

pub struct SiliconFlowProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl SiliconFlowProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_info(&self, key: &str) -> Result<ProviderResult> {
        let resp = self
            .http
            .get("https://api.siliconflow.cn/v1/user/info")
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if !status.is_success() {
            let msg = body
                .pointer("/message")
                .or_else(|| body.pointer("/error/message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Ok(ProviderResult {
                kind: ProviderKind::SiliconFlow,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: msg.to_string(),
                        console_url: Some("https://cloud.siliconflow.cn/account/ak".into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
            });
        }

        let quota = parse_info(&body)?;
        Ok(ProviderResult {
            kind: ProviderKind::SiliconFlow,
            status: ProviderStatus::Available { quota },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
        })
    }
}

#[derive(Deserialize)]
struct UserInfo {
    #[serde(rename = "chargeBalance", default)]
    charge_balance: String,
    #[serde(rename = "totalBalance", default)]
    total_balance: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    name: String,
}

pub(crate) fn parse_info(body: &serde_json::Value) -> Result<ProviderQuota> {
    // Response is wrapped in {"code":20000,"status":"OK","message":"...","data":{...}}
    // or the user info fields may be at top level.
    let data = body.get("data").unwrap_or(body);
    let info: UserInfo = serde_json::from_value(data.clone())
        .map_err(|e| crate::Error::Provider(format!("parse error: {}", e)))?;

    // Balances are decimal strings of CNY. Scale to integer ×10000 to keep precision.
    let total: f64 = info.total_balance.parse().unwrap_or(0.0);
    let charge: f64 = info.charge_balance.parse().unwrap_or(0.0);
    // granted = total - charge (free credits)
    let granted = (total - charge).max(0.0);

    let total_units = (total * 10_000.0).round() as i64;
    let charge_units = (charge * 10_000.0).round() as i64;
    let granted_units = (granted * 10_000.0).round() as i64;

    let mut windows: Vec<QuotaWindow> = Vec::new();
    if total_units > 0 {
        windows.push(QuotaWindow {
            window_type: "balance_cny".into(),
            used: 0,
            limit: total_units,
            remaining: total_units,
            reset_at: None,
            period_seconds: None,
        });
    }
    if charge_units > 0 {
        windows.push(QuotaWindow {
            window_type: "paid_cny".into(),
            used: 0,
            limit: charge_units,
            remaining: charge_units,
            reset_at: None,
            period_seconds: None,
        });
    }
    if granted_units > 0 {
        windows.push(QuotaWindow {
            window_type: "free_cny".into(),
            used: 0,
            limit: granted_units,
            remaining: granted_units,
            reset_at: None,
            period_seconds: None,
        });
    }

    let plan_name = if info.name.is_empty() {
        "SiliconFlow".to_string()
    } else {
        format!("SiliconFlow · {}", info.name)
    };

    Ok(ProviderQuota {
        plan_name,
        windows,
        unlimited: info.status == "unlimited",
    })
}

#[async_trait]
impl crate::providers::Provider for SiliconFlowProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::SiliconFlow
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = match &auth.credential {
            AuthCredential::Bearer(k) => k.clone(),
            AuthCredential::Token(t) => t.clone(),
        };

        match self.fetch_info(&key).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::SiliconFlow,
                status: ProviderStatus::NetworkError {
                    message: e.to_string(),
                },
                fetched_at: Utc::now(),
                raw_response: None,
                auth_source: None,
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
    fn parses_siliconflow_user_info() {
        let body = serde_json::json!({
            "code": 20000,
            "status": "OK",
            "data": {
                "id": "user_abc",
                "name": "Test User",
                "email": "test@example.com",
                "balance": "18.0000",
                "chargeBalance": "8.0000",
                "totalBalance": "18.0000",
                "status": "normal"
            }
        });
        let quota = parse_info(&body).unwrap();
        assert!(quota.plan_name.contains("Test User"));
        // total + paid + free(=10)
        assert_eq!(quota.windows.len(), 3);
        assert_eq!(quota.windows[0].window_type, "balance_cny");
        assert_eq!(quota.windows[0].remaining, 180_000);
        assert_eq!(quota.windows[1].window_type, "paid_cny");
        assert_eq!(quota.windows[1].remaining, 80_000);
        assert_eq!(quota.windows[2].window_type, "free_cny");
        assert_eq!(quota.windows[2].remaining, 100_000);
    }
}
