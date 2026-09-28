use crate::auth::AuthResolver;
use crate::providers::{
    ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow, UnavailableInfo,
};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
const CONSOLE_URL: &str = "https://opencode.ai/docs/go";

pub struct OpencodeGoProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl OpencodeGoProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_usage(&self, key: &str) -> Result<ProviderResult> {
        let resp = self
            .http
            .get(USAGE_URL)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await?;

        let status = resp.status();

        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Ok(result(ProviderStatus::AuthRequired, None));
        }

        let body: serde_json::Value = resp.json().await?;

        if !status.is_success() {
            let msg = body
                .pointer("/error/message")
                .or_else(|| body.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Ok(result(
                ProviderStatus::Unavailable {
                    info: UnavailableInfo {
                        reason: msg.to_string(),
                        console_url: Some(CONSOLE_URL.into()),
                    },
                },
                Some(body),
            ));
        }

        let quota = parse_usage(&body)?;
        Ok(result(
            ProviderStatus::Available { quota },
            Some(body),
        ))
    }
}

fn result(status: ProviderStatus, raw: Option<serde_json::Value>) -> ProviderResult {
    ProviderResult {
        kind: ProviderKind::OpencodeGo,
        status,
        fetched_at: Utc::now(),
        raw_response: raw,
        auth_source: None,
        cached_at: None,
        instance_key: None,
        profile_id: None,
        card_title: None,
    }
}

#[derive(Deserialize)]
struct UsageResponse {
    usage: Windows,
}

#[derive(Deserialize)]
struct Windows {
    rolling: Option<Window>,
    weekly: Option<Window>,
    monthly: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    #[serde(default)]
    status: String,
    #[serde(default)]
    percent: f64,
    #[serde(rename = "resetsAt", default)]
    resets_at: Option<DateTime<Utc>>,
}

/// Converts an API window to a `QuotaWindow`. A status other than "ok" (e.g.
/// "rate-limited") means the window is exhausted and its reported percent is
/// unreliable; this discards that percent and reports used = 100, keeping the
/// reported reset time.
fn to_window(w: &Window, window_type: &str, period_seconds: i64) -> QuotaWindow {
    let used = if w.status == "ok" {
        w.percent.round().clamp(0.0, 100.0) as i64
    } else {
        100
    };
    QuotaWindow {
        window_type: window_type.to_string(),
        used,
        limit: 100,
        remaining: (100 - used).max(0),
        reset_at: w.resets_at,
        period_seconds: Some(period_seconds),
    }
}

pub(crate) fn parse_usage(body: &serde_json::Value) -> Result<ProviderQuota> {
    let resp: UsageResponse = serde_json::from_value(body.clone())
        .map_err(|e| crate::Error::Provider(format!("parse error: {}", e)))?;

    let mut windows = Vec::new();
    if let Some(w) = &resp.usage.rolling {
        windows.push(to_window(w, "5h", 5 * 3600));
    }
    if let Some(w) = &resp.usage.weekly {
        windows.push(to_window(w, "weekly", 7 * 86400));
    }
    if let Some(w) = &resp.usage.monthly {
        windows.push(to_window(w, "monthly", 30 * 86400));
    }

    Ok(ProviderQuota {
        plan_name: "OpenCode Go".to_string(),
        windows,
        unlimited: false,
        banked_resets: None,
    })
}

#[async_trait]
impl crate::providers::Provider for OpencodeGoProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpencodeGo
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = auth.credential.unwrap_token()?.to_string();

        match self.fetch_usage(&key).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(result(
                ProviderStatus::NetworkError {
                    message: e.to_string(),
                },
                None,
            )),
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
    fn parses_usage_payload() {
        let body = serde_json::json!({
            "usage": {
                "rolling": { "status": "ok", "percent": 0, "resetsAt": "2026-09-29T00:39:33.620Z" },
                "weekly":  { "status": "ok", "percent": 0, "resetsAt": "2026-10-05T00:00:00.000Z" },
                "monthly": { "status": "ok", "percent": 26, "resetsAt": "2026-10-02T17:56:42.000Z" }
            }
        });
        let quota = parse_usage(&body).unwrap();
        assert_eq!(quota.plan_name, "OpenCode Go");
        assert_eq!(quota.windows.len(), 3);
        assert_eq!(quota.windows[0].window_type, "5h");
        assert_eq!(quota.windows[2].window_type, "monthly");
        assert_eq!(quota.windows[2].used, 26);
        assert_eq!(quota.windows[2].remaining, 74);
        assert!(quota.windows[2].reset_at.is_some());
    }

    #[test]
    fn rate_limited_window_is_exhausted() {
        let body = serde_json::json!({
            "usage": {
                "monthly": { "status": "rate-limited", "percent": 40, "resetsAt": "2026-10-02T17:56:42.000Z" }
            }
        });
        let quota = parse_usage(&body).unwrap();
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].used, 100);
        assert_eq!(quota.windows[0].remaining, 0);
        assert!(quota.windows[0].reset_at.is_some());
    }

    #[test]
    fn missing_windows_are_skipped() {
        let body = serde_json::json!({ "usage": { "weekly": { "status": "ok", "percent": 12 } } });
        let quota = parse_usage(&body).unwrap();
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].window_type, "weekly");
        assert_eq!(quota.windows[0].reset_at, None);
    }
}
