use crate::auth::{AuthCredential, AuthResolver};
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;

pub struct OpenRouterProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl OpenRouterProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_credits(&self, key: &str) -> Result<ProviderResult> {
        // Fetch both the key info and credits in parallel.
        let (key_resp, credits_resp) = tokio::join!(
            self.http
                .get("https://openrouter.ai/api/v1/auth/key")
                .header("Authorization", format!("Bearer {}", key))
                .send(),
            self.http
                .get("https://openrouter.ai/api/v1/credits")
                .header("Authorization", format!("Bearer {}", key))
                .send(),
        );

        let key_body: serde_json::Value = key_resp?.json().await?;
        let credits_body: serde_json::Value = credits_resp?.json().await?;

        let quota = parse_credits(&key_body, &credits_body)?;
        Ok(ProviderResult {
            kind: ProviderKind::OpenRouter,
            status: ProviderStatus::Available { quota },
            fetched_at: Utc::now(),
            raw_response: Some(serde_json::json!({
                "key": key_body,
                "credits": credits_body,
            })),
            auth_source: None,
        })
    }
}

#[derive(Deserialize, Default)]
struct KeyData {
    label: Option<String>,
    usage: Option<f64>,
    is_free_tier: Option<bool>,
    limit: Option<f64>,
    limit_remaining: Option<f64>,
}

#[derive(Deserialize, Default)]
struct CreditsData {
    total_credits: Option<f64>,
    total_usage: Option<f64>,
}

pub(crate) fn parse_credits(
    key_body: &serde_json::Value,
    credits_body: &serde_json::Value,
) -> Result<ProviderQuota> {
    let key_data: KeyData =
        serde_json::from_value(key_body.get("data").cloned().unwrap_or_default())
            .unwrap_or_default();
    let credits_data: CreditsData =
        serde_json::from_value(credits_body.get("data").cloned().unwrap_or_default())
            .unwrap_or_default();

    let is_free = key_data.is_free_tier.unwrap_or(false);

    // Use credits endpoint when available (more accurate total).
    let total = credits_data.total_credits.or(key_data.limit).unwrap_or(0.0);
    let used = credits_data.total_usage.or(key_data.usage).unwrap_or(0.0);
    let remaining = key_data
        .limit_remaining
        .unwrap_or_else(|| (total - used).max(0.0));

    // Scale USD to integer ×10000 for consistent display.
    let total_units = (total * 10_000.0).round() as i64;
    let used_units = (used * 10_000.0).round() as i64;
    let remaining_units = (remaining * 10_000.0).round() as i64;

    let mut windows: Vec<QuotaWindow> = Vec::new();

    if total_units > 0 || used_units > 0 {
        windows.push(QuotaWindow {
            window_type: "credits_usd".into(),
            used: used_units,
            limit: total_units.max(used_units),
            remaining: remaining_units,
            reset_at: None,
            period_seconds: None,
        });
    }

    // If the key has a per-key spend limit, show that separately.
    if let Some(lim) = key_data.limit {
        let lim_units = (lim * 10_000.0).round() as i64;
        let lim_rem_units = key_data
            .limit_remaining
            .map(|r| (r * 10_000.0).round() as i64)
            .unwrap_or(lim_units.saturating_sub(used_units));
        let lim_used = lim_units.saturating_sub(lim_rem_units);
        if lim_units != total_units {
            // Only add if different from the account total (it's a key-level cap).
            windows.push(QuotaWindow {
                window_type: "key_limit_usd".into(),
                used: lim_used,
                limit: lim_units,
                remaining: lim_rem_units,
                reset_at: None,
                period_seconds: None,
            });
        }
    }

    let tier = if is_free { "Free" } else { "Paid" };
    let label = key_data
        .label
        .filter(|s| !s.is_empty())
        .map(|s| format!(" · {}", s))
        .unwrap_or_default();
    let plan_name = format!("OpenRouter {} tier{}", tier, label);

    Ok(ProviderQuota {
        plan_name,
        windows,
        unlimited: is_free && total_units == 0,
    })
}

#[async_trait]
impl crate::providers::Provider for OpenRouterProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenRouter
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = match &auth.credential {
            AuthCredential::Bearer(k) => k.clone(),
            AuthCredential::Token(t) => t.clone(),
        };

        match self.fetch_credits(&key).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::OpenRouter,
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
    fn parses_credits_response() {
        let key_body = serde_json::json!({
            "data": {
                "label": "my-key",
                "usage": 1.2345,
                "is_free_tier": false,
                "limit": null,
                "limit_remaining": null
            }
        });
        let credits_body = serde_json::json!({
            "data": {
                "total_credits": 50.0,
                "total_usage": 1.2345
            }
        });
        let quota = parse_credits(&key_body, &credits_body).unwrap();
        assert!(quota.plan_name.contains("Paid"));
        assert!(quota.plan_name.contains("my-key"));
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].window_type, "credits_usd");
        assert_eq!(quota.windows[0].limit, 500_000);
        assert_eq!(quota.windows[0].used, 12_345);
    }
}
