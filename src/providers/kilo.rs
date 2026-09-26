use crate::auth::AuthResolver;
use crate::providers::{
    ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow, UnavailableInfo,
};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

const API_BASE: &str = "https://api.kilo.ai";
const CONSOLE_URL: &str = "https://app.kilo.ai/profile";

/// Reads the Kilo Pass subscription state (or plain credit balance) from the
/// app.kilo.ai backend.
///
/// Kilo bills a dollar-denominated balance rather than rolling rate-limit
/// windows: a Kilo Pass subscription grants a monthly allotment of paid credits
/// plus bonus credits that unlock once the paid credits are spent. Auth is the
/// JWT copied from the profile page, sent as a bearer token.
pub struct KiloProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl KiloProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    /// Runs one tRPC query and returns its `result.data` payload. The backend
    /// wraps single-query batches in a one-element array.
    async fn trpc_query(&self, token: &str, path: &str) -> Result<serde_json::Value> {
        // A no-argument tRPC query still needs the batch envelope: `batch=1`
        // with `input={"0":{}}` (URL-encoded).
        let url = format!(
            "{}/api/trpc/{}?batch=1&input=%7B%220%22%3A%7B%7D%7D",
            API_BASE, path
        );
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            let msg = body
                .pointer("/0/error/message")
                .or_else(|| body.pointer("/error/message"))
                .or_else(|| body.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(crate::Error::Provider(msg.to_string()));
        }
        Ok(body
            .pointer("/0/result/data")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    async fn fetch_quota(&self, token: &str) -> Result<ProviderResult> {
        let state = self.trpc_query(token, "kiloPass.getState").await?;
        if let Some(quota) = parse_pass_state(&state) {
            return Ok(available(ProviderStatus::Available { quota }, state));
        }

        // No Kilo Pass subscription: fall back to the raw credit balance.
        let blocks = self.trpc_query(token, "user.getCreditBlocks").await?;
        match parse_credit_blocks(&blocks) {
            Some(quota) => Ok(available(ProviderStatus::Available { quota }, blocks)),
            None => Ok(available(
                ProviderStatus::Unavailable {
                    info: UnavailableInfo {
                        reason: "no active Kilo Pass subscription or credit balance".to_string(),
                        console_url: Some(CONSOLE_URL.to_string()),
                    },
                },
                blocks,
            )),
        }
    }
}

fn available(status: ProviderStatus, raw: serde_json::Value) -> ProviderResult {
    ProviderResult {
        kind: ProviderKind::Kilo,
        status,
        fetched_at: Utc::now(),
        raw_response: Some(raw),
        auth_source: None,
        cached_at: None,
        instance_key: None,
        profile_id: None,
        card_title: None,
    }
}

/// Scale a dollar amount to the shared ×10000 integer unit.
fn usd_units(usd: f64) -> i64 {
    (usd * 10_000.0).round() as i64
}

#[derive(Deserialize)]
struct PassState {
    subscription: Option<Subscription>,
}

#[derive(Deserialize)]
struct Subscription {
    #[serde(default)]
    tier: String,
    #[serde(default)]
    cadence: String,
    #[serde(rename = "currentPeriodUsageUsd", default)]
    usage_usd: f64,
    #[serde(rename = "currentPeriodBaseCreditsUsd", default)]
    base_credits_usd: f64,
    #[serde(rename = "currentPeriodBonusCreditsUsd", default)]
    bonus_credits_usd: f64,
    #[serde(rename = "refillAt", default)]
    refill_at: Option<DateTime<Utc>>,
}

/// Builds the Kilo Pass quota, or `None` when there is no subscription.
///
/// The limit of the monthly window is base credits plus bonus credits, since
/// usage can run past the base amount once bonus credits are added.
pub(crate) fn parse_pass_state(data: &serde_json::Value) -> Option<ProviderQuota> {
    let state: PassState = serde_json::from_value(data.clone()).ok()?;
    let sub = state.subscription?;

    let used = usd_units(sub.usage_usd);
    let base = usd_units(sub.base_credits_usd);
    let bonus = usd_units(sub.bonus_credits_usd);
    let limit = base + bonus;

    let mut windows = vec![QuotaWindow {
        window_type: "monthly_allowance".into(),
        used,
        limit,
        remaining: (limit - used).max(0),
        reset_at: sub.refill_at,
        period_seconds: period_seconds_for(&sub.cadence),
    }];
    if bonus > 0 {
        windows.push(QuotaWindow {
            window_type: "bonus_usd".into(),
            used: 0,
            limit: 0,
            remaining: bonus,
            reset_at: None,
            period_seconds: None,
        });
    }

    Some(ProviderQuota {
        plan_name: plan_name(&sub.tier, &sub.cadence),
        windows,
        unlimited: false,
        banked_resets: None,
    })
}

/// Builds a balance-only quota from the credit-block totals, or `None` when the
/// account doesn't hold credits.
pub(crate) fn parse_credit_blocks(data: &serde_json::Value) -> Option<ProviderQuota> {
    let total_m_usd = data.get("totalBalance_mUsd").and_then(|v| v.as_f64())?;
    let balance = (total_m_usd / 100.0).round() as i64;
    if balance <= 0 {
        return None;
    }
    Some(ProviderQuota {
        plan_name: "Kilo".to_string(),
        windows: vec![QuotaWindow {
            window_type: "balance_usd".into(),
            used: 0,
            limit: 0,
            remaining: balance,
            reset_at: None,
            period_seconds: None,
        }],
        unlimited: false,
        banked_resets: None,
    })
}

/// Turns a tier id and cadence into a plan label, e.g. `Kilo Pass $49/mo`.
fn plan_name(tier: &str, cadence: &str) -> String {
    let amount = tier.strip_prefix("tier_").and_then(|n| n.parse::<u32>().ok());
    let period = match cadence {
        "monthly" => "/mo",
        "yearly" | "annual" | "annually" => "/yr",
        _ => "",
    };
    match amount {
        Some(amount) => format!("Kilo Pass ${}{}", amount, period),
        None => "Kilo Pass".to_string(),
    }
}

fn period_seconds_for(cadence: &str) -> Option<i64> {
    match cadence {
        "monthly" => Some(30 * 86400),
        "yearly" | "annual" | "annually" => Some(365 * 86400),
        _ => None,
    }
}

#[async_trait]
impl crate::providers::Provider for KiloProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Kilo
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let token = auth.credential.unwrap_token()?.to_string();

        match self.fetch_quota(&token).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Kilo,
                status: ProviderStatus::NetworkError {
                    message: e.to_string(),
                },
                fetched_at: Utc::now(),
                raw_response: None,
                auth_source: None,
                cached_at: None,
                instance_key: None,
                profile_id: None,
                card_title: None,
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
    fn parses_kilo_pass_state() {
        let data = serde_json::json!({
            "subscription": {
                "tier": "tier_49",
                "cadence": "monthly",
                "status": "active",
                "currentPeriodBaseCreditsUsd": 49,
                "currentPeriodUsageUsd": 21,
                "currentPeriodBonusCreditsUsd": 24.5,
                "isBonusUnlocked": false,
                "refillAt": "2026-10-24T09:09:37.000Z"
            },
            "isEligibleForFirstMonthPromo": false
        });
        let quota = parse_pass_state(&data).unwrap();
        assert_eq!(quota.plan_name, "Kilo Pass $49/mo");
        assert_eq!(quota.windows.len(), 2);

        let cycle = &quota.windows[0];
        assert_eq!(cycle.window_type, "monthly_allowance");
        assert_eq!(cycle.used, 210_000);
        assert_eq!(cycle.limit, 735_000); // ($49 + $24.50) * 10000
        assert_eq!(cycle.remaining, 525_000);
        assert!(cycle.reset_at.is_some());
        assert_eq!(cycle.period_seconds, Some(30 * 86400));

        let bonus = &quota.windows[1];
        assert_eq!(bonus.window_type, "bonus_usd");
        assert_eq!(bonus.remaining, 245_000);
        assert_eq!(bonus.limit, 0);
    }

    #[test]
    fn omits_bonus_row_when_zero() {
        let data = serde_json::json!({
            "subscription": {
                "tier": "tier_19",
                "cadence": "monthly",
                "currentPeriodBaseCreditsUsd": 19,
                "currentPeriodUsageUsd": 5,
                "currentPeriodBonusCreditsUsd": 0,
                "refillAt": "2026-10-24T09:09:37.000Z"
            }
        });
        let quota = parse_pass_state(&data).unwrap();
        assert_eq!(quota.plan_name, "Kilo Pass $19/mo");
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].limit, 190_000);
    }

    #[test]
    fn no_subscription_yields_none() {
        let data = serde_json::json!({ "subscription": null });
        assert!(parse_pass_state(&data).is_none());
    }

    #[test]
    fn parses_credit_block_balance() {
        let data = serde_json::json!({
            "creditBlocks": [],
            "totalBalance_mUsd": 27998539i64,
            "autoTopUpEnabled": false
        });
        let quota = parse_credit_blocks(&data).unwrap();
        assert_eq!(quota.plan_name, "Kilo");
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].window_type, "balance_usd");
        assert_eq!(quota.windows[0].remaining, 279_985); // $27.998539 at x10000
    }

    #[test]
    fn zero_balance_yields_none() {
        let data = serde_json::json!({ "totalBalance_mUsd": 0 });
        assert!(parse_credit_blocks(&data).is_none());
    }
}
