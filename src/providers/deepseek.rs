use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;

pub struct DeepSeekProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl DeepSeekProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_balance(&self, key: &str) -> Result<ProviderResult> {
        let resp = self
            .http
            .get("https://api.deepseek.com/user/balance")
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if !status.is_success() {
            let msg = body
                .pointer("/error/message")
                .or_else(|| body.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Ok(ProviderResult {
                kind: ProviderKind::DeepSeek,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: msg.to_string(),
                        console_url: Some("https://platform.deepseek.com/usage".into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
            });
        }

        let quota = parse_balance(&body)?;
        Ok(ProviderResult {
            kind: ProviderKind::DeepSeek,
            status: ProviderStatus::Available { quota },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
        })
    }
}

#[derive(Deserialize)]
struct BalanceResponse {
    is_available: Option<bool>,
    balance_infos: Vec<BalanceInfo>,
}

#[derive(Deserialize)]
struct BalanceInfo {
    currency: String,
    total_balance: String,
    granted_balance: String,
    topped_up_balance: String,
}

pub(crate) fn parse_balance(body: &serde_json::Value) -> Result<ProviderQuota> {
    let resp: BalanceResponse = serde_json::from_value(body.clone())
        .map_err(|e| crate::Error::Provider(format!("parse error: {}", e)))?;

    // Convert string balances (e.g. "12.34") to integer cents for our window model.
    // We store as micro-units (×10000) to preserve 4 decimal places.
    let mut windows: Vec<QuotaWindow> = Vec::new();

    for info in &resp.balance_infos {
        let total: f64 = info.total_balance.parse().unwrap_or(0.0);
        let granted: f64 = info.granted_balance.parse().unwrap_or(0.0);
        let topped_up: f64 = info.topped_up_balance.parse().unwrap_or(0.0);
        let currency = &info.currency;

        // Show total balance as a payg_balance-style window.
        // Encode as integer by scaling to 4 decimal places.
        let total_units = (total * 10_000.0).round() as i64;
        let granted_units = (granted * 10_000.0).round() as i64;
        let topped_units = (topped_up * 10_000.0).round() as i64;

        if total_units > 0 || granted_units > 0 || topped_units > 0 {
            windows.push(QuotaWindow {
                window_type: format!("balance_{}", currency.to_lowercase()),
                used: 0,
                limit: total_units,
                remaining: total_units,
                reset_at: None,
                period_seconds: None,
            });
            if granted_units > 0 {
                windows.push(QuotaWindow {
                    window_type: format!("granted_{}", currency.to_lowercase()),
                    used: 0,
                    limit: granted_units,
                    remaining: granted_units,
                    reset_at: None,
                    period_seconds: None,
                });
            }
            if topped_units > 0 {
                windows.push(QuotaWindow {
                    window_type: format!("topped_up_{}", currency.to_lowercase()),
                    used: 0,
                    limit: topped_units,
                    remaining: topped_units,
                    reset_at: None,
                    period_seconds: None,
                });
            }
        }
    }

    let available = resp.is_available.unwrap_or(true);
    let plan = if available {
        "DeepSeek API"
    } else {
        "DeepSeek API (unavailable)"
    };

    Ok(ProviderQuota {
        plan_name: plan.to_string(),
        windows,
        unlimited: false,
    })
}

#[async_trait]
impl crate::providers::Provider for DeepSeekProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::DeepSeek
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = auth.credential.unwrap_token()?.to_string();

        match self.fetch_balance(&key).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::DeepSeek,
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
    fn parses_balance_payload() {
        let body = serde_json::json!({
            "is_available": true,
            "balance_infos": [
                {
                    "currency": "CNY",
                    "total_balance": "12.3456",
                    "granted_balance": "2.0000",
                    "topped_up_balance": "10.3456"
                }
            ]
        });
        let quota = parse_balance(&body).unwrap();
        assert_eq!(quota.plan_name, "DeepSeek API");
        // balance_cny + granted_cny + topped_up_cny
        assert_eq!(quota.windows.len(), 3);
        assert_eq!(quota.windows[0].window_type, "balance_cny");
        assert_eq!(quota.windows[0].remaining, 123_456);
        assert_eq!(quota.windows[1].window_type, "granted_cny");
        assert_eq!(quota.windows[2].window_type, "topped_up_cny");
    }

    #[test]
    fn parses_zero_granted_omits_window() {
        let body = serde_json::json!({
            "is_available": true,
            "balance_infos": [
                {
                    "currency": "USD",
                    "total_balance": "5.00",
                    "granted_balance": "0.00",
                    "topped_up_balance": "5.00"
                }
            ]
        });
        let quota = parse_balance(&body).unwrap();
        // granted is 0 → skipped; total + topped_up
        assert_eq!(quota.windows.len(), 2);
        assert_eq!(quota.windows[0].window_type, "balance_usd");
        assert_eq!(quota.windows[1].window_type, "topped_up_usd");
    }
}
