use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

pub struct CursorProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl CursorProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_quota(&self, session_token: &str) -> Result<ProviderResult> {
        let make_req = |endpoint: &str, body: serde_json::Value| {
            let token = session_token.to_string();
            self.http
                .post(endpoint)
                .header("Content-Type", "application/json")
                .header("Origin", "https://cursor.com")
                .header("Referer", "https://cursor.com/dashboard/spending")
                .header(
                    "User-Agent",
                    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
                )
                .header("Cookie", format!("WorkosCursorSessionToken={}", token))
                .body(serde_json::to_string(&body).unwrap())
                .send()
        };

        // Fetch plan info
        let plan_resp = make_req(
            "https://cursor.com/api/dashboard/get-plan-info",
            serde_json::json!({}),
        )
        .await?;

        // Fetch usage
        let usage_resp = make_req(
            "https://cursor.com/api/dashboard/get-current-period-usage",
            serde_json::json!({}),
        )
        .await?;

        let plan_status = plan_resp.status();
        let usage_status = usage_resp.status();

        let plan_body: serde_json::Value = plan_resp.json().await?;
        let usage_body: serde_json::Value = usage_resp.json().await?;

        // Handle auth errors
        if plan_status.as_u16() == 401 || usage_status.as_u16() == 401 {
            return Ok(ProviderResult {
                kind: ProviderKind::Cursor,
                status: ProviderStatus::AuthRequired,
                fetched_at: Utc::now(),
                raw_response: Some(serde_json::json!({
                    "plan": plan_body,
                    "usage": usage_body,
                })),
                auth_source: None,
                cached_at: None,
            });
        }

        if !plan_status.is_success() || !usage_status.is_success() {
            let msg = plan_body
                .get("error")
                .or_else(|| usage_body.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Ok(ProviderResult {
                kind: ProviderKind::Cursor,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: msg.to_string(),
                        console_url: Some("https://cursor.com/dashboard/spending".into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(serde_json::json!({
                    "plan": plan_body,
                    "usage": usage_body,
                })),
                auth_source: None,
                cached_at: None,
            });
        }

        let quota = parse_quota(&plan_body, &usage_body)?;
        Ok(ProviderResult {
            kind: ProviderKind::Cursor,
            status: ProviderStatus::Available { quota },
            fetched_at: Utc::now(),
            raw_response: Some(serde_json::json!({
                "plan": plan_body,
                "usage": usage_body,
            })),
            auth_source: None,
            cached_at: None,
        })
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PlanInfoResponse {
    #[serde(default, rename = "planInfo")]
    plan_info: Option<PlanInfo>,
    #[serde(default, rename = "nextUpgrade")]
    next_upgrade: Option<NextUpgrade>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PlanInfo {
    #[serde(default, rename = "planName")]
    plan_name: Option<String>,
    #[serde(default, rename = "price")]
    price_str: Option<String>,
    #[serde(default, rename = "billingCycleEnd")]
    billing_cycle_end: Option<String>,
    #[serde(default, rename = "includedAmountCents")]
    included_amount_cents: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct NextUpgrade {
    #[serde(default, rename = "name")]
    name: Option<String>,
    #[serde(default, rename = "price")]
    price_str: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UsageResponse {
    #[serde(default, rename = "billingCycleEnd")]
    billing_cycle_end: Option<String>,
    #[serde(default, rename = "billingCycleStart")]
    billing_cycle_start: Option<String>,
    #[serde(default, rename = "planUsage")]
    plan_usage: Option<PlanUsage>,
    #[serde(default, rename = "spendLimitUsage")]
    spend_limit_usage: Option<SpendLimitUsage>,
}

#[derive(Debug, Deserialize)]
struct PlanUsage {
    #[serde(default, rename = "totalSpend")]
    total_spend: Option<f64>,
    #[serde(default, rename = "includedSpend")]
    included_spend: Option<f64>,
    #[serde(default, rename = "bonusSpend")]
    bonus_spend: Option<f64>,
    #[serde(default, rename = "apiPercentUsed")]
    api_percent_used: Option<f64>,
    #[serde(default, rename = "autoPercentUsed")]
    auto_percent_used: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct SpendLimitUsage {
    #[serde(default, rename = "individualLimit")]
    individual_limit: Option<i64>,
    #[serde(default, rename = "individualRemaining")]
    individual_remaining: Option<i64>,
}

fn parse_quota(
    plan_body: &serde_json::Value,
    usage_body: &serde_json::Value,
) -> Result<ProviderQuota> {
    let plan: PlanInfoResponse = serde_json::from_value(plan_body.clone())
        .map_err(|e| crate::Error::Provider(format!("parse plan error: {}", e)))?;
    let usage: UsageResponse = serde_json::from_value(usage_body.clone())
        .map_err(|e| crate::Error::Provider(format!("parse usage error: {}", e)))?;

    let mut windows = Vec::new();

    // All Cursor usage resets at the billing-cycle boundary, so stamp every
    // window with the same reset timestamp (and elapsed period, so the card's
    // pace overlay works). Timestamps are millisecond epochs.
    let parse_ms = |ts: &str| {
        ts.parse::<i64>()
            .ok()
            .map(|ms| DateTime::from_timestamp(ms / 1000, 0).unwrap_or_else(Utc::now))
    };
    let reset_at = usage.billing_cycle_end.as_deref().and_then(parse_ms);
    let period_seconds = match (
        usage.billing_cycle_start.as_deref().and_then(parse_ms),
        reset_at,
    ) {
        (Some(start), Some(end)) => Some((end - start).num_seconds().max(0)),
        _ => None,
    };

    // Billing cycle spend window from planUsage
    if let Some(plan_usage) = &usage.plan_usage {
        let included_spend = plan_usage.included_spend.unwrap_or(0.0) as i64;
        let total_spend = plan_usage.total_spend.unwrap_or(0.0) as i64;
        let bonus_spend = plan_usage.bonus_spend.unwrap_or(0.0) as i64;
        let used = total_spend - bonus_spend; // base usage without bonus
        let limit = included_spend;
        let remaining = limit.saturating_sub(used);

        windows.push(QuotaWindow {
            window_type: "billing_cycle".to_string(),
            used,
            limit,
            remaining,
            reset_at,
            period_seconds,
        });

        // Show bonus as a separate window
        if bonus_spend > 0 {
            windows.push(QuotaWindow {
                window_type: "bonus".to_string(),
                used: 0,
                limit: bonus_spend,
                remaining: bonus_spend,
                reset_at,
                period_seconds,
            });
        }
    }

    // Spend limit usage window
    if let Some(spend_limit) = &usage.spend_limit_usage {
        if let (Some(limit), Some(remaining)) = (
            spend_limit.individual_limit,
            spend_limit.individual_remaining,
        ) {
            let used = limit.saturating_sub(remaining);
            windows.push(QuotaWindow {
                window_type: "spend_limit".to_string(),
                used,
                limit,
                remaining,
                reset_at,
                period_seconds,
            });
        }
    }

    // API usage percentage window
    if let Some(plan_usage) = &usage.plan_usage {
        if let Some(api_pct) = plan_usage.api_percent_used {
            windows.push(QuotaWindow {
                window_type: "api_usage_pct".to_string(),
                used: (api_pct * 100.0) as i64,
                limit: 10000, // percentage * 100
                remaining: ((100.0 - api_pct) * 100.0) as i64,
                reset_at,
                period_seconds,
            });
        }
        if let Some(auto_pct) = plan_usage.auto_percent_used {
            windows.push(QuotaWindow {
                window_type: "auto_usage_pct".to_string(),
                used: (auto_pct * 100.0) as i64,
                limit: 10000,
                remaining: ((100.0 - auto_pct) * 100.0) as i64,
                reset_at,
                period_seconds,
            });
        }
    }

    let plan_name = plan
        .plan_info
        .as_ref()
        .and_then(|p| p.plan_name.clone())
        .unwrap_or_else(|| "Cursor".to_string());

    Ok(ProviderQuota {
        plan_name,
        windows,
        unlimited: false,
    })
}

#[async_trait]
impl crate::providers::Provider for CursorProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Cursor
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let session_token = auth.credential.unwrap_cookie()?.to_string();

        match self.fetch_quota(&session_token).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Cursor,
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
            .join("tests/fixtures/cursor")
            .join(name);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {}", path.display(), e));
        serde_json::from_str(&raw).expect("parse fixture json")
    }

    #[test]
    fn parses_cursor_live_fixture_with_resets() {
        let plan = fixture("plan_default.json");
        let usage = fixture("usage_default.json");
        let quota = parse_quota(&plan, &usage).unwrap();

        assert_eq!(quota.plan_name, "Pro");

        // Every emitted window carries the billing-cycle reset + elapsed period
        // so the card can show "resets in X" and a pace overlay.
        let expected_reset = DateTime::from_timestamp(1785957659, 0).unwrap();
        let expected_period = 1785957659 - 1783279259; // end - start (seconds)
        assert!(!quota.windows.is_empty());
        for w in &quota.windows {
            assert_eq!(
                w.reset_at,
                Some(expected_reset),
                "{} missing reset_at",
                w.window_type
            );
            assert_eq!(
                w.period_seconds,
                Some(expected_period),
                "{} missing period_seconds",
                w.window_type
            );
        }

        let auto = quota
            .windows
            .iter()
            .find(|w| w.window_type == "auto_usage_pct")
            .expect("auto_usage_pct window");
        assert_eq!(auto.used, 2201); // 22.0133% * 100
        assert_eq!(auto.limit, 10000);

        let api = quota
            .windows
            .iter()
            .find(|w| w.window_type == "api_usage_pct")
            .expect("api_usage_pct window");
        assert_eq!(api.used, 0);

        // Live spendLimitUsage has no individualLimit -> no spend_limit window.
        assert!(!quota.windows.iter().any(|w| w.window_type == "spend_limit"));
    }
}
