use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{Months, NaiveDate, TimeZone, Utc};
use reqwest::Client;

pub struct GitHubCopilotProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl GitHubCopilotProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_user(&self, token: &str) -> Result<ProviderResult> {
        let resp = self
            .http
            .get("https://api.github.com/copilot_internal/user")
            .header("Authorization", format!("Bearer {}", token))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("editor-version", "vscode/1.99.3")
            .header("editor-plugin-version", "copilot-chat/0.26.7")
            .header("user-agent", "GitHubCopilotChat/0.26.7")
            .header("x-github-api-version", "2025-04-01")
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(crate::Error::Auth(format!(
                "github copilot: {}",
                status.as_u16()
            )));
        }

        if !status.is_success() {
            let msg = body
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Ok(ProviderResult {
                kind: ProviderKind::GitHubCopilot,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: format!("{} (status {})", msg, status.as_u16()),
                        console_url: Some("https://github.com/settings/copilot".into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            });
        }

        let quota = parse_user(&body);
        Ok(ProviderResult {
            kind: ProviderKind::GitHubCopilot,
            status: ProviderStatus::Available { quota },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
            cached_at: None,
        })
    }
}

fn humanize_plan(raw: &str) -> String {
    match raw {
        "individual_free" => "Free".to_string(),
        "individual_pro" => "Pro".to_string(),
        "individual_pro_plus" => "Pro+".to_string(),
        "business" => "Business".to_string(),
        "enterprise" => "Enterprise".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn parse_user(body: &serde_json::Value) -> ProviderQuota {
    let plan_raw = body
        .get("copilot_plan")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let plan_name = humanize_plan(plan_raw);

    let reset_at = body
        .get("quota_reset_date")
        .and_then(|v| v.as_str())
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| Utc.from_utc_datetime(&dt));

    let snapshots = body
        .get("quota_snapshots")
        .cloned()
        .unwrap_or_default();
    let premium = snapshots.get("premium_interactions").cloned().unwrap_or_default();

    let unlimited = premium
        .get("unlimited")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut windows = Vec::new();

    let limit = premium
        .get("entitlement")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let remaining = premium
        .get("remaining")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    if !unlimited && limit > 0 {
        let used = (limit - remaining).max(0);
        // Full calendar-month window: reset_at minus 1 month = period start.
        let period_seconds = reset_at
            .and_then(|r| r.checked_sub_months(Months::new(1)).map(|s| (r - s).num_seconds()))
            .filter(|s| *s > 0);
        windows.push(QuotaWindow {
            window_type: "monthly".to_string(),
            used,
            limit,
            remaining,
            reset_at,
            period_seconds,
        });
    }

    ProviderQuota {
        plan_name,
        windows,
        unlimited,
    }
}

#[async_trait]
impl crate::providers::Provider for GitHubCopilotProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GitHubCopilot
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let token = auth.credential.unwrap_token()?.to_string();

        match self.fetch_user(&token).await {
            Ok(r) => Ok(r),
            Err(crate::Error::Auth(msg)) => Err(crate::Error::Auth(msg)),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::GitHubCopilot,
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

    #[test]
    fn parses_individual_pro() {
        let body = serde_json::json!({
            "copilot_plan": "individual_pro",
            "quota_reset_date": "2026-05-01",
            "quota_snapshots": {
                "premium_interactions": {
                    "entitlement": 300,
                    "remaining": 93,
                    "unlimited": false
                },
                "chat": { "unlimited": true, "entitlement": 0, "remaining": 0 },
                "completions": { "unlimited": true, "entitlement": 0, "remaining": 0 }
            }
        });
        let quota = parse_user(&body);
        assert_eq!(quota.plan_name, "Pro");
        assert!(!quota.unlimited);
        assert_eq!(quota.windows.len(), 1);
        let w = &quota.windows[0];
        assert_eq!(w.window_type, "monthly");
        assert_eq!(w.limit, 300);
        assert_eq!(w.remaining, 93);
        assert_eq!(w.used, 207);
        assert!(w.reset_at.is_some());
    }

    #[test]
    fn parses_pro_plus() {
        let body = serde_json::json!({
            "copilot_plan": "individual_pro_plus",
            "quota_reset_date": "2026-05-01",
            "quota_snapshots": {
                "premium_interactions": {
                    "entitlement": 1500,
                    "remaining": 1200,
                    "unlimited": false
                }
            }
        });
        let quota = parse_user(&body);
        assert_eq!(quota.plan_name, "Pro+");
        assert_eq!(quota.windows[0].limit, 1500);
    }

    #[test]
    fn handles_unlimited_plan() {
        let body = serde_json::json!({
            "copilot_plan": "enterprise",
            "quota_reset_date": "2026-05-01",
            "quota_snapshots": {
                "premium_interactions": {
                    "entitlement": 0,
                    "remaining": 0,
                    "unlimited": true
                }
            }
        });
        let quota = parse_user(&body);
        assert!(quota.unlimited);
        assert_eq!(quota.plan_name, "Enterprise");
        assert!(quota.windows.is_empty());
    }

    #[test]
    fn handles_free_plan() {
        let body = serde_json::json!({
            "copilot_plan": "individual_free",
            "quota_reset_date": "2026-05-01",
            "quota_snapshots": {
                "premium_interactions": {
                    "entitlement": 50,
                    "remaining": 10,
                    "unlimited": false
                }
            }
        });
        let quota = parse_user(&body);
        assert_eq!(quota.plan_name, "Free");
        assert_eq!(quota.windows[0].limit, 50);
        assert_eq!(quota.windows[0].used, 40);
    }
}
