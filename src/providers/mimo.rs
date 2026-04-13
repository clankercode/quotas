use crate::auth::{AuthCredential, AuthResolver};
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;

/// Xiaomi MiMo API provider.
///
/// Two base URLs exist:
///   - PAYG (pay-as-you-go): https://api.xiaomimimo.com/v1
///   - Token Plan (monthly subscription, SGP region): https://token-plan-sgp.xiaomimimo.com/v1
///
/// We try both for balance; whichever returns a successful 2xx response wins.
const PAYG_BASE: &str = "https://api.xiaomimimo.com/v1";
const TOKEN_PLAN_BASE: &str = "https://token-plan-sgp.xiaomimimo.com/v1";
const DASHBOARD_URL: &str = "https://platform.xiaomimimo.com";

pub struct MimoProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl MimoProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn try_balance(&self, key: &str, base: &str) -> Result<(u16, serde_json::Value)> {
        let resp = self
            .http
            .get(format!("{}/user/balance", base))
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body: serde_json::Value = resp.json().await?;
        Ok((status, body))
    }

    async fn fetch_balance(&self, key: &str) -> Result<ProviderResult> {
        // Try PAYG endpoint first; fall back to Token Plan endpoint.
        let (status, body, base_used) = match self.try_balance(key, PAYG_BASE).await {
            Ok((s, b)) if s != 404 => (s, b, PAYG_BASE),
            _ => {
                let (s, b) = self.try_balance(key, TOKEN_PLAN_BASE).await?;
                (s, b, TOKEN_PLAN_BASE)
            }
        };

        if status == 401 || status == 403 {
            return Ok(ProviderResult {
                kind: ProviderKind::Mimo,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: body
                            .pointer("/error/message")
                            .or_else(|| body.get("message"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("Authentication failed")
                            .to_string(),
                        console_url: Some(DASHBOARD_URL.into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
            });
        }

        if status == 404 {
            // No quota endpoint available on either base URL.
            return Ok(ProviderResult {
                kind: ProviderKind::Mimo,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: "No quota API available for this account type".to_string(),
                        console_url: Some(DASHBOARD_URL.into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: None,
                auth_source: None,
            });
        }

        if status >= 400 {
            let msg = body
                .pointer("/error/message")
                .or_else(|| body.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            return Ok(ProviderResult {
                kind: ProviderKind::Mimo,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: msg,
                        console_url: Some(DASHBOARD_URL.into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
            });
        }

        let quota = parse_balance(&body, base_used)?;
        Ok(ProviderResult {
            kind: ProviderKind::Mimo,
            status: ProviderStatus::Available { quota },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
        })
    }
}

#[derive(Deserialize, Default)]
struct BalanceResp {
    /// Total balance in CNY (some APIs expose this as a string, others as f64).
    #[serde(default)]
    balance: Option<serde_json::Value>,
    /// Granted / free credits.
    #[serde(default)]
    granted_balance: Option<serde_json::Value>,
    /// Charged / topped-up credits.
    #[serde(default)]
    charge_balance: Option<serde_json::Value>,
    /// Token quota remaining (Token Plan accounts may use tokens, not CNY).
    #[serde(default)]
    token_balance: Option<serde_json::Value>,
    /// Monthly token quota (Token Plan).
    #[serde(default)]
    token_limit: Option<serde_json::Value>,
    /// Plan / tier name.
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    plan_name: Option<String>,
}

fn parse_f64(v: &serde_json::Value) -> f64 {
    match v {
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
        serde_json::Value::String(s) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub(crate) fn parse_balance(body: &serde_json::Value, base: &str) -> Result<ProviderQuota> {
    // Balance may be nested under a "data" key.
    let data = body.get("data").unwrap_or(body);
    let resp: BalanceResp = serde_json::from_value(data.clone()).unwrap_or_default();

    let mut windows: Vec<QuotaWindow> = Vec::new();

    // Token Plan accounts: report token quota if present.
    if let (Some(tok_bal), Some(tok_lim)) = (&resp.token_balance, &resp.token_limit) {
        let remaining = parse_f64(tok_bal) as i64;
        let limit = parse_f64(tok_lim) as i64;
        if limit > 0 {
            windows.push(QuotaWindow {
                window_type: "tokens_monthly".into(),
                used: (limit - remaining).max(0),
                limit,
                remaining,
                reset_at: None,
                period_seconds: Some(30 * 24 * 3600),
            });
        }
    }

    // PAYG / CNY balance windows.
    if let Some(bal) = &resp.balance {
        let total = (parse_f64(bal) * 10_000.0).round() as i64;
        if total > 0 {
            windows.push(QuotaWindow {
                window_type: "balance_cny".into(),
                used: 0,
                limit: total,
                remaining: total,
                reset_at: None,
                period_seconds: None,
            });
        }
    }
    if let Some(charge) = &resp.charge_balance {
        let v = (parse_f64(charge) * 10_000.0).round() as i64;
        if v > 0 {
            windows.push(QuotaWindow {
                window_type: "paid_cny".into(),
                used: 0,
                limit: v,
                remaining: v,
                reset_at: None,
                period_seconds: None,
            });
        }
    }
    if let Some(granted) = &resp.granted_balance {
        let v = (parse_f64(granted) * 10_000.0).round() as i64;
        if v > 0 {
            windows.push(QuotaWindow {
                window_type: "free_cny".into(),
                used: 0,
                limit: v,
                remaining: v,
                reset_at: None,
                period_seconds: None,
            });
        }
    }

    let plan_label = resp
        .plan_name
        .or(resp.plan)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if base == TOKEN_PLAN_BASE {
                "Token Plan".to_string()
            } else {
                "PAYG".to_string()
            }
        });

    // If no windows were found, show as available but with no data.
    Ok(ProviderQuota {
        plan_name: format!("MiMo · {}", plan_label),
        windows,
        unlimited: false,
    })
}

#[async_trait]
impl crate::providers::Provider for MimoProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Mimo
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = match &auth.credential {
            AuthCredential::Bearer(k) => k.clone(),
            AuthCredential::Token(t) => t.clone(),
        };

        match self.fetch_balance(&key).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Mimo,
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
    fn parses_payg_cny_balance() {
        let body = serde_json::json!({
            "data": {
                "balance": "12.5000",
                "charge_balance": "10.0000",
                "granted_balance": "2.5000",
                "plan": "PAYG"
            }
        });
        let quota = parse_balance(&body, PAYG_BASE).unwrap();
        assert!(quota.plan_name.contains("MiMo"));
        assert_eq!(quota.windows[0].window_type, "balance_cny");
        assert_eq!(quota.windows[0].remaining, 125_000);
        assert_eq!(quota.windows[1].window_type, "paid_cny");
        assert_eq!(quota.windows[1].remaining, 100_000);
        assert_eq!(quota.windows[2].window_type, "free_cny");
        assert_eq!(quota.windows[2].remaining, 25_000);
    }

    #[test]
    fn parses_token_plan_balance() {
        let body = serde_json::json!({
            "data": {
                "token_balance": 800_000,
                "token_limit": 1_000_000,
                "plan_name": "Pro"
            }
        });
        let quota = parse_balance(&body, TOKEN_PLAN_BASE).unwrap();
        assert!(quota.plan_name.contains("Pro"));
        assert_eq!(quota.windows[0].window_type, "tokens_monthly");
        assert_eq!(quota.windows[0].limit, 1_000_000);
        assert_eq!(quota.windows[0].remaining, 800_000);
        assert_eq!(quota.windows[0].used, 200_000);
    }

    #[test]
    fn empty_balance_response_returns_no_windows() {
        let body = serde_json::json!({ "data": {} });
        let quota = parse_balance(&body, PAYG_BASE).unwrap();
        assert!(quota.windows.is_empty());
    }
}
