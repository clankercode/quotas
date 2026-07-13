use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

const CLI_CHAT_BASE: &str = "https://cli-chat-proxy.grok.com";
const MGMT_BASE: &str = "https://management-api.x.ai";
const CONSOLE_URL: &str = "https://console.x.ai/team/default/billing";
const GROK_USAGE_URL: &str = "https://grok.com?_s=usage";
const GROK_CLIENT_VERSION: &str = "0.2.93";

pub struct GrokProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl GrokProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    /// Primary path: Grok Build session token → cli-chat-proxy billing.
    /// Fallback: management key → Management API prepaid balance.
    async fn fetch_quota(&self, key: &str) -> Result<ProviderResult> {
        let cli_auth_err = match self.fetch_cli_billing(key).await {
            Ok(r) => return Ok(r),
            // Expired/invalid session token. Try the management path, but
            // remember this error: for a session-only credential the session
            // failure is the relevant message, not the downstream
            // management-key validation error.
            Err(e @ crate::Error::Auth(_)) => Some(e),
            // 404 / not session-capable → this is a management key; fall
            // through to the management path with no error to preserve.
            Err(crate::Error::Provider(_)) => None,
            Err(e) => {
                // Network / parse on primary path: still try management API
                // before failing hard.
                if let Ok(r) = self.fetch_mgmt_billing(key).await {
                    return Ok(r);
                }
                return Err(e);
            }
        };
        // Fallback: management key → Management API prepaid balance. If the
        // primary cli path failed with an auth error, surface *that* rather
        // than the management-key validation error it would otherwise mask.
        match self.fetch_mgmt_billing(key).await {
            Ok(r) => Ok(r),
            Err(mgmt_err) => Err(cli_auth_err.unwrap_or(mgmt_err)),
        }
    }

    async fn fetch_cli_billing(&self, key: &str) -> Result<ProviderResult> {
        // Default billing = calendar monthly $ allowance.
        // `?format=credits` = rolling weekly product usage % (what Grok Build's
        // /usage UI shows under WEEKLY) plus prepaid/on-demand fields.
        let auth_hdr = format!("Bearer {}", key);
        let (default_resp, credits_resp) = tokio::join!(
            self.http
                .get(format!("{}/v1/billing", CLI_CHAT_BASE))
                .header("Authorization", &auth_hdr)
                .header("Accept", "application/json")
                .header("x-grok-client-version", GROK_CLIENT_VERSION)
                .header("x-grok-client-surface", "grok-build")
                .send(),
            self.http
                .get(format!("{}/v1/billing?format=credits", CLI_CHAT_BASE))
                .header("Authorization", &auth_hdr)
                .header("Accept", "application/json")
                .header("x-grok-client-version", GROK_CLIENT_VERSION)
                .header("x-grok-client-surface", "grok-build")
                .send(),
        );

        let default_resp = default_resp?;
        let default_status = default_resp.status();
        let default_body: serde_json::Value = default_resp.json().await?;

        if default_status.as_u16() == 401 || default_status.as_u16() == 403 {
            let msg = extract_error_message(&default_body)
                .unwrap_or_else(|| format!("HTTP {}", default_status.as_u16()));
            return Err(crate::Error::Auth(msg));
        }
        if !default_status.is_success() {
            let msg = extract_error_message(&default_body)
                .unwrap_or_else(|| format!("HTTP {}", default_status.as_u16()));
            // 404 / other → not a session-capable key; try management path.
            return Err(crate::Error::Provider(msg));
        }

        if default_body.get("error").is_some() && default_body.get("config").is_none() {
            let msg =
                extract_error_message(&default_body).unwrap_or_else(|| "billing error".into());
            return Err(crate::Error::Provider(msg));
        }

        let credits_body = match credits_resp {
            Ok(resp) if resp.status().is_success() => resp.json().await.ok(),
            _ => None,
        };

        let quota = parse_cli_billing(&default_body, credits_body.as_ref())?;
        Ok(ProviderResult {
            kind: ProviderKind::Grok,
            status: ProviderStatus::Available { quota },
            fetched_at: Utc::now(),
            raw_response: Some(serde_json::json!({
                "default": default_body,
                "format_credits": credits_body,
            })),
            auth_source: None,
            cached_at: None,
        })
    }

    async fn fetch_mgmt_billing(&self, key: &str) -> Result<ProviderResult> {
        let validation = self.validate_mgmt_key(key).await?;
        let team_id = validation
            .team_id
            .as_ref()
            .filter(|s| !s.is_empty())
            .cloned()
            .or_else(|| {
                validation
                    .scope_id
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .cloned()
            })
            .ok_or_else(|| {
                crate::Error::Provider("management key validation returned no team id".into())
            })?;

        let (balance_resp, preview_resp) = tokio::join!(
            self.http
                .get(format!(
                    "{}/v1/billing/teams/{}/prepaid/balance",
                    MGMT_BASE, team_id
                ))
                .header("Authorization", format!("Bearer {}", key))
                .send(),
            self.http
                .get(format!(
                    "{}/v1/billing/teams/{}/postpaid/invoice/preview",
                    MGMT_BASE, team_id
                ))
                .header("Authorization", format!("Bearer {}", key))
                .send(),
        );

        let balance_resp = balance_resp?;
        let balance_status = balance_resp.status();
        let balance_body: serde_json::Value = balance_resp.json().await?;

        if !balance_status.is_success() {
            let msg = extract_error_message(&balance_body)
                .unwrap_or_else(|| format!("HTTP {}", balance_status.as_u16()));
            return Ok(ProviderResult {
                kind: ProviderKind::Grok,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: msg,
                        console_url: Some(CONSOLE_URL.into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(serde_json::json!({
                    "validation": validation_raw(&validation),
                    "balance": balance_body,
                })),
                auth_source: None,
                cached_at: None,
            });
        }

        let preview_body = match preview_resp {
            Ok(resp) if resp.status().is_success() => resp.json().await.ok(),
            _ => None,
        };

        let quota = parse_mgmt_billing(&balance_body, preview_body.as_ref())?;
        Ok(ProviderResult {
            kind: ProviderKind::Grok,
            status: ProviderStatus::Available { quota },
            fetched_at: Utc::now(),
            raw_response: Some(serde_json::json!({
                "validation": validation_raw(&validation),
                "balance": balance_body,
                "invoice_preview": preview_body,
            })),
            auth_source: None,
            cached_at: None,
        })
    }

    async fn validate_mgmt_key(&self, key: &str) -> Result<ValidationInfo> {
        let resp = self
            .http
            .get(format!("{}/auth/management-keys/validation", MGMT_BASE))
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if !status.is_success() {
            let msg = extract_error_message(&body)
                .unwrap_or_else(|| format!("management key validation failed (HTTP {})", status));
            return Err(crate::Error::Auth(msg));
        }

        let team_id = body
            .get("teamId")
            .or_else(|| body.get("team_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let scope_id = body
            .get("scopeId")
            .or_else(|| body.get("scope_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let name = body
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(ValidationInfo {
            team_id,
            scope_id,
            name,
        })
    }
}

struct ValidationInfo {
    team_id: Option<String>,
    scope_id: Option<String>,
    name: Option<String>,
}

fn validation_raw(v: &ValidationInfo) -> serde_json::Value {
    serde_json::json!({
        "teamId": v.team_id,
        "scopeId": v.scope_id,
        "name": v.name,
    })
}

fn extract_error_message(body: &serde_json::Value) -> Option<String> {
    body.pointer("/error/message")
        .or_else(|| body.get("message"))
        .or_else(|| body.get("error").filter(|e| e.is_string()))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// xAI amounts are USD cents as decimal strings (or numbers). Convert to
/// the project's ×10_000 USD units. Prepaid ledger balances may be negative;
/// take abs for remaining-credit semantics.
fn cents_str_to_units(val: &str) -> i64 {
    let cents: f64 = val.parse().unwrap_or(0.0);
    (cents.abs() * 100.0).round() as i64
}

fn cents_value_to_units(v: &serde_json::Value) -> Option<i64> {
    if let Some(obj) = v.as_object() {
        if let Some(val) = obj.get("val").and_then(|x| x.as_str()) {
            return Some(cents_str_to_units(val));
        }
        // `val` as a JSON number. `as_f64` matches every JSON number
        // (integer or float), so this also covers integer-typed amounts.
        // Prepaid ledger balances may be negative; take abs for
        // remaining-credit semantics (used/limit on cli billing are always
        // non-negative, so abs is a no-op there).
        if let Some(n) = obj.get("val").and_then(|x| x.as_f64()) {
            return Some((n.abs() * 100.0).round() as i64);
        }
    }
    if let Some(s) = v.as_str() {
        return Some(cents_str_to_units(s));
    }
    if let Some(n) = v.as_f64() {
        return Some((n.abs() * 100.0).round() as i64);
    }
    None
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Percentage usage → integer units on a 0–100 scale (74.0% → used=74, limit=100).
fn pct_to_units(pct: f64) -> (i64, i64, i64) {
    let used = pct.clamp(0.0, 100.0).round() as i64;
    let limit = 100;
    let remaining = (limit - used).max(0);
    (used, limit, remaining)
}

fn period_type_label(period_type: &str) -> &'static str {
    let upper = period_type.to_ascii_uppercase();
    if upper.contains("WEEK") {
        "weekly"
    } else if upper.contains("MONTH") {
        "monthly"
    } else if upper.contains("DAY") {
        "daily"
    } else {
        "period"
    }
}

fn append_on_demand(
    windows: &mut Vec<QuotaWindow>,
    config: &serde_json::Value,
    reset_at: Option<DateTime<Utc>>,
    period_seconds: Option<i64>,
) {
    let cap = config
        .get("onDemandCap")
        .or_else(|| config.get("on_demand_cap"))
        .and_then(cents_value_to_units)
        .unwrap_or(0);
    if cap <= 0 {
        return;
    }
    let used = config
        .get("onDemandUsed")
        .or_else(|| config.get("on_demand_used"))
        .and_then(cents_value_to_units)
        .unwrap_or(0);
    windows.push(QuotaWindow {
        window_type: "on_demand_usd".into(),
        used,
        limit: cap.max(used),
        remaining: (cap - used).max(0),
        reset_at,
        period_seconds,
    });
}

/// Merge default monthly $ billing with `?format=credits` weekly product usage.
///
/// `default_body` fields of interest:
/// - `config.monthlyLimit` / `used` (USD cents)
/// - `config.billingPeriodStart` / `billingPeriodEnd` (calendar month)
/// - `config.history[]` prior months
/// - `config.onDemandCap`
///
/// `credits_body` (`?format=credits`) fields of interest:
/// - `config.currentPeriod.{type,start,end}` e.g. `USAGE_PERIOD_TYPE_WEEKLY`
/// - `config.creditUsagePercent` overall period usage
/// - `config.productUsage[]` per-product `{product, usagePercent}`
/// - `config.prepaidBalance`, `onDemandUsed` / `onDemandCap`
/// - `config.isUnifiedBillingUser`, `topUpMethod`
pub(crate) fn parse_cli_billing(
    default_body: &serde_json::Value,
    credits_body: Option<&serde_json::Value>,
) -> Result<ProviderQuota> {
    let default_cfg = default_body
        .get("config")
        .cloned()
        .unwrap_or_else(|| default_body.clone());
    let credits_cfg = credits_body
        .and_then(|b| b.get("config"))
        .cloned()
        .unwrap_or_default();

    let mut windows: Vec<QuotaWindow> = Vec::new();

    // --- Weekly / period product usage (format=credits) ---
    let period = credits_cfg.get("currentPeriod").or_else(|| credits_cfg.get("current_period"));
    let period_label = period
        .and_then(|p| p.get("type"))
        .and_then(|t| t.as_str())
        .map(period_type_label)
        .unwrap_or("weekly");
    let period_start = period
        .and_then(|p| p.get("start"))
        .and_then(|v| v.as_str())
        .and_then(parse_rfc3339)
        .or_else(|| {
            credits_cfg
                .get("billingPeriodStart")
                .or_else(|| credits_cfg.get("billing_period_start"))
                .and_then(|v| v.as_str())
                .and_then(parse_rfc3339)
        });
    let period_end = period
        .and_then(|p| p.get("end"))
        .and_then(|v| v.as_str())
        .and_then(parse_rfc3339)
        .or_else(|| {
            credits_cfg
                .get("billingPeriodEnd")
                .or_else(|| credits_cfg.get("billing_period_end"))
                .and_then(|v| v.as_str())
                .and_then(parse_rfc3339)
        });
    let period_seconds = match (period_start, period_end) {
        (Some(s), Some(e)) => Some((e - s).num_seconds().max(0)),
        _ => None,
    };

    let mut saw_product_usage = false;
    if let Some(products) = credits_cfg
        .get("productUsage")
        .or_else(|| credits_cfg.get("product_usage"))
        .and_then(|v| v.as_array())
    {
        for prod in products {
            let name = prod
                .get("product")
                .and_then(|v| v.as_str())
                .unwrap_or("product");
            let pct = prod
                .get("usagePercent")
                .or_else(|| prod.get("usage_percent"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let (used, limit, remaining) = pct_to_units(pct);
            // Shorten GrokBuild → build for card labels.
            let short = match name {
                "GrokBuild" => "build",
                other => other,
            };
            windows.push(QuotaWindow {
                window_type: format!("{}/{}", period_label, short),
                used,
                limit,
                remaining,
                reset_at: period_end,
                period_seconds,
            });
            saw_product_usage = true;
        }
    }

    // Overall period % if no per-product breakdown (or as summary when products
    // absent). Skip when products already cover the same percentage.
    if !saw_product_usage {
        if let Some(pct) = credits_cfg
            .get("creditUsagePercent")
            .or_else(|| credits_cfg.get("credit_usage_percent"))
            .and_then(|v| v.as_f64())
        {
            let (used, limit, remaining) = pct_to_units(pct);
            windows.push(QuotaWindow {
                window_type: period_label.into(),
                used,
                limit,
                remaining,
                reset_at: period_end,
                period_seconds,
            });
        }
    }

    // --- Monthly $ allowance (default billing) ---
    let month_start = default_cfg
        .get("billingPeriodStart")
        .or_else(|| default_cfg.get("billing_period_start"))
        .and_then(|v| v.as_str())
        .and_then(parse_rfc3339);
    let month_end = default_cfg
        .get("billingPeriodEnd")
        .or_else(|| default_cfg.get("billing_period_end"))
        .and_then(|v| v.as_str())
        .and_then(parse_rfc3339);
    let month_seconds = match (month_start, month_end) {
        (Some(s), Some(e)) => Some((e - s).num_seconds().max(0)),
        _ => None,
    };

    let limit_units = default_cfg
        .get("monthlyLimit")
        .or_else(|| default_cfg.get("monthly_limit"))
        .and_then(cents_value_to_units)
        .unwrap_or(0);
    let used_units = default_cfg
        .get("used")
        .and_then(cents_value_to_units)
        .unwrap_or(0);

    if limit_units > 0 || used_units > 0 {
        // Distinct from the bare `monthly` window type (used by the credits
        // summary here and by other providers e.g. GitHub Copilot) so the TUI
        // can currency-format only this USD allowance without colliding.
        windows.push(QuotaWindow {
            window_type: "monthly_allowance".into(),
            used: used_units,
            limit: limit_units.max(used_units),
            remaining: (limit_units - used_units).max(0),
            reset_at: month_end,
            period_seconds: month_seconds,
        });
    }

    // Prefer on-demand from credits body (has used); fall back to default.
    let on_demand_src = if credits_cfg.get("onDemandCap").is_some()
        || credits_cfg.get("on_demand_cap").is_some()
    {
        &credits_cfg
    } else {
        &default_cfg
    };
    append_on_demand(
        &mut windows,
        on_demand_src,
        period_end.or(month_end),
        period_seconds.or(month_seconds),
    );

    // Prepaid balance (format=credits), when non-zero.
    if let Some(bal) = credits_cfg
        .get("prepaidBalance")
        .or_else(|| credits_cfg.get("prepaid_balance"))
        .and_then(cents_value_to_units)
    {
        if bal > 0 {
            windows.push(QuotaWindow {
                window_type: "balance_usd".into(),
                used: 0,
                limit: bal,
                remaining: bal,
                reset_at: None,
                period_seconds: None,
            });
        }
    }

    let plan_name = if credits_cfg
        .get("isUnifiedBillingUser")
        .or_else(|| credits_cfg.get("is_unified_billing_user"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        "Grok Build".to_string()
    } else {
        "Grok".to_string()
    };

    let unlimited = windows.is_empty();

    Ok(ProviderQuota {
        plan_name,
        windows,
        unlimited,
        banked_resets: None,
    })
}

#[derive(Deserialize, Default)]
struct BalanceBody {
    total: Option<serde_json::Value>,
}

pub(crate) fn parse_mgmt_billing(
    balance_body: &serde_json::Value,
    preview_body: Option<&serde_json::Value>,
) -> Result<ProviderQuota> {
    let balance: BalanceBody =
        serde_json::from_value(balance_body.clone()).unwrap_or_default();

    let remaining_units = balance
        .total
        .as_ref()
        .and_then(cents_value_to_units)
        .unwrap_or(0);

    let mut windows: Vec<QuotaWindow> = Vec::new();

    windows.push(QuotaWindow {
        window_type: "balance_usd".into(),
        used: 0,
        limit: remaining_units,
        remaining: remaining_units,
        reset_at: None,
        period_seconds: None,
    });

    if let Some(preview) = preview_body {
        let core = preview
            .get("coreInvoice")
            .or_else(|| preview.get("core_invoice"));

        if let Some(used) = core
            .and_then(|c| {
                c.get("prepaidCreditsUsed")
                    .or_else(|| c.get("prepaid_credits_used"))
            })
            .and_then(cents_value_to_units)
        {
            if used > 0 {
                let issued = remaining_units.saturating_add(used);
                windows.push(QuotaWindow {
                    window_type: "credits_usd".into(),
                    used,
                    limit: issued.max(used),
                    remaining: remaining_units,
                    reset_at: None,
                    period_seconds: None,
                });
            }
        }

        if let Some(limit) = preview
            .get("effectiveSpendingLimit")
            .or_else(|| preview.get("effective_spending_limit"))
            .and_then(cents_value_to_units)
        {
            if limit > 0 {
                windows.push(QuotaWindow {
                    window_type: "spend_limit_usd".into(),
                    used: 0,
                    limit,
                    remaining: limit,
                    reset_at: None,
                    period_seconds: None,
                });
            }
        }

        if let Some(default_credits) = preview
            .get("defaultCredits")
            .or_else(|| preview.get("default_credits"))
            .and_then(cents_value_to_units)
        {
            if default_credits > 0 {
                windows.push(QuotaWindow {
                    window_type: "granted_usd".into(),
                    used: 0,
                    limit: default_credits,
                    remaining: default_credits,
                    reset_at: None,
                    period_seconds: None,
                });
            }
        }
    }

    Ok(ProviderQuota {
        plan_name: "xAI API".into(),
        windows,
        unlimited: false,
        banked_resets: None,
    })
}

#[async_trait]
impl crate::providers::Provider for GrokProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Grok
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = auth.credential.unwrap_token()?.to_string();

        match self.fetch_quota(&key).await {
            Ok(r) => Ok(r),
            Err(crate::Error::Auth(msg)) => Ok(ProviderResult {
                kind: ProviderKind::Grok,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: format!(
                            "{} — run `grok login`, or set XAI_MANAGEMENT_KEY for API prepaid balance",
                            msg
                        ),
                        console_url: Some(GROK_USAGE_URL.into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: None,
                auth_source: None,
                cached_at: None,
            }),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Grok,
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
            .join("tests/fixtures/grok")
            .join(name);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {}", path.display(), e));
        serde_json::from_str(&raw).expect("parse fixture json")
    }

    #[test]
    fn parses_live_fixtures_weekly_and_monthly() {
        let default = fixture("cli_billing_default.json");
        let credits = fixture("cli_billing_format_credits.json");
        let quota = parse_cli_billing(&default, Some(&credits)).unwrap();
        assert_eq!(quota.plan_name, "Grok Build");

        // weekly/build from productUsage, then monthly $
        let weekly = quota
            .windows
            .iter()
            .find(|w| w.window_type == "weekly/build")
            .expect("weekly/build window");
        assert!(weekly.used > 0);
        assert_eq!(weekly.limit, 100);
        assert!(weekly.reset_at.is_some());
        assert_eq!(weekly.period_seconds, Some(7 * 24 * 3600));

        let monthly = quota
            .windows
            .iter()
            .find(|w| w.window_type == "monthly_allowance")
            .expect("monthly allowance window");
        // live fixture: monthlyLimit 15000 cents = $150
        assert_eq!(monthly.limit, 1_500_000);
        assert!(monthly.used > 0);
        assert!(monthly.reset_at.is_some());
    }

    #[test]
    fn parses_cli_billing_monthly_only() {
        let body = serde_json::json!({
            "config": {
                "monthlyLimit": { "val": 15000 },
                "used": { "val": 2712 },
                "onDemandCap": { "val": 0 },
                "billingPeriodStart": "2026-07-01T00:00:00+00:00",
                "billingPeriodEnd": "2026-08-01T00:00:00+00:00"
            }
        });
        let quota = parse_cli_billing(&body, None).unwrap();
        assert_eq!(quota.windows.len(), 1);
        let w = &quota.windows[0];
        assert_eq!(w.window_type, "monthly_allowance");
        assert_eq!(w.limit, 1_500_000);
        assert_eq!(w.used, 271_200);
        assert_eq!(w.remaining, 1_228_800);
        assert_eq!(w.period_seconds, Some(31 * 24 * 3600));
    }

    #[test]
    fn parses_cli_billing_weekly_product_usage() {
        let default = serde_json::json!({
            "config": {
                "monthlyLimit": { "val": 15000 },
                "used": { "val": 1000 },
                "billingPeriodStart": "2026-07-01T00:00:00Z",
                "billingPeriodEnd": "2026-08-01T00:00:00Z"
            }
        });
        let credits = serde_json::json!({
            "config": {
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": "2026-07-07T10:46:52.885620+00:00",
                    "end": "2026-07-14T10:46:52.885620+00:00"
                },
                "creditUsagePercent": 74.0,
                "productUsage": [
                    { "product": "GrokBuild", "usagePercent": 74.0 }
                ],
                "isUnifiedBillingUser": true,
                "onDemandCap": { "val": 0 },
                "onDemandUsed": { "val": 0 },
                "prepaidBalance": { "val": 0 }
            }
        });
        let quota = parse_cli_billing(&default, Some(&credits)).unwrap();
        assert_eq!(quota.plan_name, "Grok Build");
        assert_eq!(quota.windows[0].window_type, "weekly/build");
        assert_eq!(quota.windows[0].used, 74);
        assert_eq!(quota.windows[0].limit, 100);
        assert_eq!(quota.windows[0].remaining, 26);
        assert_eq!(quota.windows[1].window_type, "monthly_allowance");
    }

    #[test]
    fn parses_cli_billing_with_on_demand() {
        let body = serde_json::json!({
            "config": {
                "monthlyLimit": { "val": "10000" },
                "used": { "val": "1000" },
                "onDemandCap": { "val": "5000" },
                "billingPeriodStart": "2026-07-01T00:00:00Z",
                "billingPeriodEnd": "2026-08-01T00:00:00Z"
            }
        });
        let quota = parse_cli_billing(&body, None).unwrap();
        assert_eq!(quota.windows.len(), 2);
        assert_eq!(quota.windows[1].window_type, "on_demand_usd");
        assert_eq!(quota.windows[1].remaining, 500_000);
    }

    #[test]
    fn parses_prepaid_balance_only() {
        let balance = serde_json::json!({
            "changes": [],
            "total": { "val": "-1250" }
        });
        let quota = parse_mgmt_billing(&balance, None).unwrap();
        assert_eq!(quota.plan_name, "xAI API");
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].window_type, "balance_usd");
        assert_eq!(quota.windows[0].remaining, 125_000);
    }

    #[test]
    fn parses_balance_with_invoice_preview() {
        let balance = serde_json::json!({
            "total": { "val": "-4500" }
        });
        let preview = serde_json::json!({
            "coreInvoice": {
                "prepaidCredits": { "val": "-4500" },
                "prepaidCreditsUsed": { "val": "500" }
            },
            "effectiveSpendingLimit": "20000",
            "defaultCredits": "0"
        });
        let quota = parse_mgmt_billing(&balance, Some(&preview)).unwrap();
        assert_eq!(quota.windows.len(), 3);
        assert_eq!(quota.windows[1].window_type, "credits_usd");
        assert_eq!(quota.windows[1].used, 50_000);
        assert_eq!(quota.windows[1].limit, 500_000);
    }

    #[test]
    fn cents_conversion_handles_positive_and_negative() {
        assert_eq!(cents_str_to_units("-1000"), 100_000);
        assert_eq!(cents_str_to_units("250"), 25_000);
        assert_eq!(cents_str_to_units("0"), 0);
    }
}
