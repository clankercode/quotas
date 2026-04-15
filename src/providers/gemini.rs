use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

pub struct GeminiProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl GeminiProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_quota(&self, key: &str) -> Result<ProviderResult> {
        // The CodeAssist API uses a POST endpoint
        let resp = self
            .http
            .post("https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota")
            .header("Authorization", format!("Bearer {}", key))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                // Empty project_id means use the user's default project
                "project": "",
                "userAgent": "quotas-cli"
            }))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        // Handle auth errors
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Ok(ProviderResult {
                kind: ProviderKind::Gemini,
                status: ProviderStatus::AuthRequired,
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            });
        }

        if !status.is_success() {
            let msg = body
                .get("error")
                .and_then(|v| v.get("message"))
                .and_then(|v| v.as_str())
                .or_else(|| body.get("message").and_then(|v| v.as_str()))
                .unwrap_or("unknown error");
            return Ok(ProviderResult {
                kind: ProviderKind::Gemini,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: msg.to_string(),
                        console_url: Some("https://aistudio.google.com/usage".into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            });
        }

        let quota = parse_quota(&body)?;
        Ok(ProviderResult {
            kind: ProviderKind::Gemini,
            status: ProviderStatus::Available { quota },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
            cached_at: None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct QuotaResponse {
    #[serde(default)]
    buckets: Vec<BucketInfo>,
}

#[derive(Debug, Deserialize)]
struct BucketInfo {
    #[serde(rename = "remainingAmount", default)]
    remaining_amount: String,
    #[serde(rename = "remainingFraction", default)]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime", default)]
    reset_time: Option<String>,
    #[serde(rename = "modelId", default)]
    model_id: Option<String>,
    #[serde(rename = "tokenType", default)]
    token_type: Option<String>,
}

pub(crate) fn parse_quota(body: &serde_json::Value) -> Result<ProviderQuota> {
    let resp: QuotaResponse = serde_json::from_value(body.clone())
        .map_err(|e| crate::Error::Provider(format!("parse error: {}", e)))?;

    let mut windows: Vec<QuotaWindow> = Vec::new();

    for bucket in &resp.buckets {
        let remaining_amount: i64 = bucket
            .remaining_amount
            .parse()
            .unwrap_or(0);

        // Calculate limit from remaining_amount and remaining_fraction
        // limit = remaining_amount / remaining_fraction
        let limit = if let Some(fraction) = bucket.remaining_fraction {
            if fraction > 0.0 {
                (remaining_amount as f64 / fraction).round() as i64
            } else {
                remaining_amount
            }
        } else {
            // If no fraction provided, we can't determine the limit
            // Show as unlimited (remaining = limit, used = 0)
            remaining_amount
        };

        let used = limit.saturating_sub(remaining_amount);

        let window_type = match (&bucket.token_type, &bucket.model_id) {
            (Some(tt), Some(mid)) => format!("{}_{}", tt.to_uppercase(), mid),
            (Some(tt), None) => tt.to_uppercase(),
            (None, Some(mid)) => mid.clone(),
            (None, None) => "unknown".to_string(),
        };

        let reset_at = bucket.reset_time.as_ref().and_then(|t| {
            DateTime::parse_from_rfc3339(t)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
        });

        windows.push(QuotaWindow {
            window_type,
            used,
            limit,
            remaining: remaining_amount,
            reset_at,
            period_seconds: None,
        });
    }

    let plan_name = if windows.is_empty() {
        "Gemini API (no quota data)".to_string()
    } else {
        "Gemini API".to_string()
    };

    Ok(ProviderQuota {
        plan_name,
        windows,
        unlimited: false,
    })
}

#[async_trait]
impl crate::providers::Provider for GeminiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Gemini
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = auth.credential.unwrap_token()?.to_string();

        match self.fetch_quota(&key).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Gemini,
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
    fn parses_quota_response() {
        let body = serde_json::json!({
            "buckets": [
                {
                    "remainingAmount": "850",
                    "remainingFraction": 0.85,
                    "resetTime": "2026-04-15T12:00:00Z",
                    "modelId": "gemini-2.5-flash",
                    "tokenType": "RPM"
                },
                {
                    "remainingAmount": "1500000",
                    "remainingFraction": 0.75,
                    "resetTime": "2026-04-15T00:00:00Z",
                    "modelId": "gemini-2.0-flash",
                    "tokenType": "RPD"
                },
                {
                    "remainingAmount": "1000000",
                    "remainingFraction": 0.5,
                    "resetTime": "2026-04-15T00:00:00Z",
                    "modelId": "gemini-2.0-flash",
                    "tokenType": "TPM"
                }
            ]
        });

        let quota = parse_quota(&body).unwrap();
        assert_eq!(quota.plan_name, "Gemini API");
        assert_eq!(quota.windows.len(), 3);

        // RPM window
        assert_eq!(quota.windows[0].window_type, "RPM_gemini-2.5-flash");
        assert_eq!(quota.windows[0].remaining, 850);
        assert_eq!(quota.windows[0].limit, 1000); // 850 / 0.85
        assert_eq!(quota.windows[0].used, 150); // 1000 - 850

        // RPD window
        assert_eq!(quota.windows[1].window_type, "RPD_gemini-2.0-flash");
        assert_eq!(quota.windows[1].remaining, 1500000);
        assert_eq!(quota.windows[1].limit, 2000000); // 1500000 / 0.75

        // TPM window
        assert_eq!(quota.windows[2].window_type, "TPM_gemini-2.0-flash");
        assert_eq!(quota.windows[2].remaining, 1000000);
        assert_eq!(quota.windows[2].limit, 2000000); // 1000000 / 0.5
    }

    #[test]
    fn parses_empty_buckets() {
        let body = serde_json::json!({
            "buckets": []
        });
        let quota = parse_quota(&body).unwrap();
        assert_eq!(quota.plan_name, "Gemini API (no quota data)");
        assert!(quota.windows.is_empty());
    }

    #[test]
    fn handles_missing_fraction() {
        let body = serde_json::json!({
            "buckets": [
                {
                    "remainingAmount": "100",
                    "resetTime": "2026-04-15T12:00:00Z",
                    "modelId": "gemini-2.0-flash",
                    "tokenType": "RPM"
                }
            ]
        });

        let quota = parse_quota(&body).unwrap();
        assert_eq!(quota.windows.len(), 1);
        // Without fraction, limit = remaining_amount
        assert_eq!(quota.windows[0].limit, 100);
        assert_eq!(quota.windows[0].remaining, 100);
        assert_eq!(quota.windows[0].used, 0);
    }
}
