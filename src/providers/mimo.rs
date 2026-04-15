use crate::auth::{AuthCredential, AuthResolver};
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;

/// Xiaomi MiMo API provider.
///
/// Three sources of quota data:
///   - Platform dashboard (cookie auth): https://platform.xiaomimimo.com/api/v1/tokenPlan/usage
///   - PAYG (bearer): https://api.xiaomimimo.com/v1/user/balance
///   - Token Plan SGP (bearer): https://token-plan-sgp.xiaomimimo.com/v1/user/balance
///
/// Cookie auth is tried first when available (it gives monthly token usage);
/// bearer endpoints are tried as fallback or when only an API key is configured.
const PAYG_BASE: &str = "https://api.xiaomimimo.com/v1";
const TOKEN_PLAN_BASE: &str = "https://token-plan-sgp.xiaomimimo.com/v1";
const PLATFORM_USAGE: &str = "https://platform.xiaomimimo.com/api/v1/tokenPlan/usage";
const TOKEN_PLAN_SGP_USAGE: &str = "https://token-plan-sgp.xiaomimimo.com/v1/tokenPlan/usage";
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

    /// Try a JSON endpoint with cookie auth.
    async fn try_json_cookie(
        &self,
        cookie: &str,
        url: &str,
    ) -> Result<Option<(u16, serde_json::Value)>> {
        let resp = self
            .http
            .get(url)
            .header("Cookie", format!("api-platform_serviceToken=\"{cookie}\""))
            .header("accept", "application/json")
            .header("accept-language", "en")
            .send()
            .await?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(body) => Ok(Some((status, body))),
            Err(_) => Ok(None),
        }
    }

    /// Try a JSON endpoint with bearer auth.
    async fn try_json_bearer(
        &self,
        key: &str,
        url: &str,
    ) -> Result<Option<(u16, serde_json::Value)>> {
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {key}"))
            .send()
            .await?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(body) => Ok(Some((status, body))),
            Err(_) => Ok(None),
        }
    }

    /// Verify the bearer API key is valid by calling /v1/models. Returns the
    /// base URL that accepted the key, or None if auth failed on both.
    async fn detect_base_url(&self, key: &str) -> Result<Option<&'static str>> {
        for base in &[TOKEN_PLAN_BASE, PAYG_BASE] {
            if let Some((status, _)) = self.try_json_bearer(key, &format!("{base}/models")).await? {
                if status == 200 {
                    return Ok(Some(base));
                }
            }
        }
        Ok(None)
    }

    /// Fetch via platform dashboard cookie auth.
    async fn fetch_platform(&self, cookie: &str) -> Result<Option<ProviderResult>> {
        let Some((status, body)) = self.try_json_cookie(cookie, PLATFORM_USAGE).await? else {
            return Ok(None);
        };

        if status == 401 || status == 403 {
            return Ok(Some(ProviderResult {
                kind: ProviderKind::Mimo,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: "Cookie expired — refresh from browser".to_string(),
                        console_url: Some(DASHBOARD_URL.into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            }));
        }

        if status >= 400 {
            return Ok(None);
        }

        // Check for code != 0 error in the body.
        if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let msg = body
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Ok(Some(ProviderResult {
                    kind: ProviderKind::Mimo,
                    status: ProviderStatus::Unavailable {
                        info: crate::providers::UnavailableInfo {
                            reason: msg.to_string(),
                            console_url: Some(DASHBOARD_URL.into()),
                        },
                    },
                    fetched_at: Utc::now(),
                    raw_response: Some(body),
                    auth_source: None,
                    cached_at: None,
                }));
            }
        }

        match parse_platform_usage(&body) {
            Ok(quota) => Ok(Some(ProviderResult {
                kind: ProviderKind::Mimo,
                status: ProviderStatus::Available { quota },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            })),
            Err(_) => Ok(None),
        }
    }

    /// Fetch via bearer API key.
    ///
    /// Tries (in order):
    ///   1. tokenPlan/usage on token-plan-sgp (monthUsage format)
    ///   2. tokenPlan/usage on platform.xiaomimimo.com (monthUsage format)
    ///   3. /user/balance on token-plan-sgp (legacy balance format)
    ///   4. /user/balance on api.xiaomimimo.com (PAYG balance format)
    async fn fetch_bearer(&self, key: &str) -> Result<ProviderResult> {
        // 1. Try tokenPlan/usage endpoints with bearer auth.
        for url in &[TOKEN_PLAN_SGP_USAGE, PLATFORM_USAGE] {
            if let Some((status, body)) = self.try_json_bearer(key, url).await? {
                if status == 401 || status == 403 {
                    // Auth error — return immediately.
                    let msg = body
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Authentication failed");
                    return Ok(ProviderResult {
                        kind: ProviderKind::Mimo,
                        status: ProviderStatus::Unavailable {
                            info: crate::providers::UnavailableInfo {
                                reason: msg.to_string(),
                                console_url: Some(DASHBOARD_URL.into()),
                            },
                        },
                        fetched_at: Utc::now(),
                        raw_response: Some(body),
                        auth_source: None,
                        cached_at: None,
                    });
                }
                if status < 400 {
                    // Check code != 0 for platform-style errors.
                    if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
                        if code != 0 {
                            continue; // Not a valid response, try next.
                        }
                    }
                    if let Ok(quota) = parse_platform_usage(&body) {
                        return Ok(ProviderResult {
                            kind: ProviderKind::Mimo,
                            status: ProviderStatus::Available { quota },
                            fetched_at: Utc::now(),
                            raw_response: Some(body),
                            auth_source: None,
                            cached_at: None,
                        });
                    }
                }
            }
        }

        // 2. Fall back to /user/balance endpoints.
        for base in &[TOKEN_PLAN_BASE, PAYG_BASE] {
            if let Some((status, body)) = self
                .try_json_bearer(key, &format!("{base}/user/balance"))
                .await?
            {
                if status == 401 || status == 403 {
                    let reason = body
                        .pointer("/error/message")
                        .or_else(|| body.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Authentication failed")
                        .to_string();
                    return Ok(ProviderResult {
                        kind: ProviderKind::Mimo,
                        status: ProviderStatus::Unavailable {
                            info: crate::providers::UnavailableInfo {
                                reason,
                                console_url: Some(DASHBOARD_URL.into()),
                            },
                        },
                        fetched_at: Utc::now(),
                        raw_response: Some(body),
                        auth_source: None,
                        cached_at: None,
                    });
                }
                if status < 400 {
                    let quota = parse_balance(&body, base)?;
                    return Ok(ProviderResult {
                        kind: ProviderKind::Mimo,
                        status: ProviderStatus::Available { quota },
                        fetched_at: Utc::now(),
                        raw_response: Some(body),
                        auth_source: None,
                        cached_at: None,
                    });
                }
            }
        }

        // 3. No endpoint worked. Check if key is at least valid.
        match self.detect_base_url(key).await? {
            Some(_base) => Ok(ProviderResult {
                kind: ProviderKind::Mimo,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: "No quota API — check usage at platform.xiaomimimo.com"
                            .to_string(),
                        console_url: Some(DASHBOARD_URL.into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: None,
                auth_source: None,
                cached_at: None,
            }),
            None => Ok(ProviderResult {
                kind: ProviderKind::Mimo,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: "Invalid API key".to_string(),
                        console_url: Some(DASHBOARD_URL.into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: None,
                auth_source: None,
                cached_at: None,
            }),
        }
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

fn parse_i64(v: &serde_json::Value) -> i64 {
    match v {
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
        serde_json::Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Parse the platform dashboard response:
/// `{ "data": { "monthUsage": { "items": [{ "name": "month_total_token", "used": N, "limit": N }] } } }`
pub(crate) fn parse_platform_usage(body: &serde_json::Value) -> Result<ProviderQuota> {
    let items = body
        .pointer("/data/monthUsage/items")
        .and_then(|v| v.as_array())
        .ok_or_else(|| crate::Error::Provider("missing data.monthUsage.items".into()))?;

    let mut windows: Vec<QuotaWindow> = Vec::new();
    for item in items {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("token")
            .to_string();
        let used = item.get("used").map(parse_i64).unwrap_or(0);
        let limit = item.get("limit").map(parse_i64).unwrap_or(0);
        if limit > 0 {
            windows.push(QuotaWindow {
                window_type: name,
                used,
                limit,
                remaining: (limit - used).max(0),
                reset_at: None,
                period_seconds: Some(30 * 24 * 3600),
            });
        }
    }

    Ok(ProviderQuota {
        plan_name: "MiMo · Token Plan".into(),
        windows,
        unlimited: false,
    })
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

        match &auth.credential {
            AuthCredential::Cookie(cookie) => {
                // Cookie auth: try platform API first, then fall back to bearer.
                if let Some(result) = self.fetch_platform(cookie).await? {
                    return Ok(result);
                }
                // Cookie didn't work — try bearer endpoints if possible.
                // (Platform cookie is usually the only credential, so this is
                // just a safety net.)
                Err(crate::Error::Auth(
                    "platform cookie auth returned no data".into(),
                ))
            }
            AuthCredential::Bearer(key) | AuthCredential::Token(key) => {
                // Bearer auth: try PAYG / Token Plan SGP endpoints.
                self.fetch_bearer(key).await
            }
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

    #[test]
    fn parses_platform_month_usage() {
        let body = serde_json::json!({
            "code": 0,
            "message": "",
            "data": {
                "monthUsage": {
                    "percent": 0.1661,
                    "items": [{
                        "name": "month_total_token",
                        "used": 265741632,
                        "limit": 1600000000,
                        "percent": 0.1661
                    }]
                }
            }
        });
        let quota = parse_platform_usage(&body).unwrap();
        assert_eq!(quota.plan_name, "MiMo · Token Plan");
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].window_type, "month_total_token");
        assert_eq!(quota.windows[0].used, 265741632);
        assert_eq!(quota.windows[0].limit, 1600000000);
        assert_eq!(quota.windows[0].remaining, 1600000000 - 265741632);
    }

    /// Probe all MiMo endpoints with a bearer API key.
    /// Run with: MIMO_API_KEY=sk-xxx cargo test mimo_bearer_probe -- --ignored
    ///
    /// Tries each URL and prints status + first 200 chars of body.
    #[tokio::test]
    #[ignore]
    async fn mimo_bearer_probe() {
        let key = std::env::var("MIMO_API_KEY")
            .expect("set MIMO_API_KEY to a MiMo API key for this test");
        let http = Client::new();

        let urls = [
            ("tokenPlan/usage (sgp)", TOKEN_PLAN_SGP_USAGE),
            ("tokenPlan/usage (platform)", PLATFORM_USAGE),
            ("balance (sgp)", &format!("{TOKEN_PLAN_BASE}/user/balance")),
            ("balance (payg)", &format!("{PAYG_BASE}/user/balance")),
            ("models (sgp)", &format!("{TOKEN_PLAN_BASE}/models")),
            ("models (payg)", &format!("{PAYG_BASE}/models")),
        ];

        for (label, url) in &urls {
            let resp = http
                .get(*url)
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .await
                .expect("request failed");
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            let preview: String = text.chars().take(300).collect();
            println!("--- {label} [{status}] ---\n{preview}\n");
        }
    }

    /// Probe MiMo endpoints with cookie auth.
    /// Run with: MIMO_COOKIE=base64token cargo test mimo_cookie_probe -- --ignored
    #[tokio::test]
    #[ignore]
    async fn mimo_cookie_probe() {
        let cookie = std::env::var("MIMO_COOKIE")
            .expect("set MIMO_COOKIE to the api-platform_serviceToken value");
        let http = Client::new();

        let urls = [
            ("tokenPlan/usage (platform)", PLATFORM_USAGE),
            ("tokenPlan/usage (sgp)", TOKEN_PLAN_SGP_USAGE),
        ];

        for (label, url) in &urls {
            let resp = http
                .get(*url)
                .header("Cookie", format!("api-platform_serviceToken=\"{cookie}\""))
                .header("accept", "application/json")
                .header("accept-language", "en")
                .send()
                .await
                .expect("request failed");
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            let preview: String = text.chars().take(300).collect();
            println!("--- {label} [{status}] ---\n{preview}\n");
        }
    }

    /// Full provider fetch with bearer API key.
    /// Run with: MIMO_API_KEY=sk-xxx cargo test mimo_full_fetch_bearer -- --ignored
    #[tokio::test]
    #[ignore]
    async fn mimo_full_fetch_bearer() {
        let key = std::env::var("MIMO_API_KEY")
            .expect("set MIMO_API_KEY for this test");
        use crate::auth::{AuthCredential, ResolvedAuth};
        use crate::providers::Provider;

        struct StubAuth(String);
        #[async_trait]
        impl AuthResolver for StubAuth {
            async fn resolve(&self) -> crate::Result<ResolvedAuth> {
                Ok(ResolvedAuth {
                    credential: AuthCredential::Bearer(self.0.clone()),
                    source: "test".into(),
                })
            }
        }

        let provider = MimoProvider::new(Box::new(StubAuth(key)));
        let result = provider.fetch().await.expect("fetch failed");
        println!("status: {:?}", result.status);
        if let Some(raw) = &result.raw_response {
            println!("raw: {}", serde_json::to_string_pretty(raw).unwrap());
        }

        match &result.status {
            ProviderStatus::Available { quota } => {
                println!("plan: {}", quota.plan_name);
                for w in &quota.windows {
                    println!(
                        "  {}: {}/{} remaining={} reset={:?}",
                        w.window_type, w.used, w.limit, w.remaining, w.reset_at
                    );
                }
            }
            other => println!("non-available status: {other:?}"),
        }
    }

    /// Full provider fetch with cookie auth.
    /// Run with: MIMO_COOKIE=base64token cargo test mimo_full_fetch_cookie -- --ignored
    #[tokio::test]
    #[ignore]
    async fn mimo_full_fetch_cookie() {
        let cookie = std::env::var("MIMO_COOKIE")
            .expect("set MIMO_COOKIE for this test");
        use crate::auth::{AuthCredential, ResolvedAuth};
        use crate::providers::Provider;

        struct StubAuth(String);
        #[async_trait]
        impl AuthResolver for StubAuth {
            async fn resolve(&self) -> crate::Result<ResolvedAuth> {
                Ok(ResolvedAuth {
                    credential: AuthCredential::Cookie(self.0.clone()),
                    source: "test".into(),
                })
            }
        }

        let provider = MimoProvider::new(Box::new(StubAuth(cookie)));
        let result = provider.fetch().await.expect("fetch failed");
        println!("status: {:?}", result.status);
        if let Some(raw) = &result.raw_response {
            println!("raw: {}", serde_json::to_string_pretty(raw).unwrap());
        }

        match &result.status {
            ProviderStatus::Available { quota } => {
                println!("plan: {}", quota.plan_name);
                for w in &quota.windows {
                    println!(
                        "  {}: {}/{} remaining={} reset={:?}",
                        w.window_type, w.used, w.limit, w.remaining, w.reset_at
                    );
                }
            }
            other => println!("non-available status: {other:?}"),
        }
    }
}
