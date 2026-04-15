use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;

pub struct ZaiProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl ZaiProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_quota(&self, key: &str) -> Result<ProviderResult> {
        let url = "https://api.z.ai/api/monitor/usage/quota/limit";
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", key))
            .header("Content-Type", "application/json")
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 200 {
            let code = body.get("code").and_then(|c| c.as_i64());
            if code == Some(200) {
                let quota = self.parse_response(&body)?;
                return Ok(ProviderResult {
                    kind: ProviderKind::Zai,
                    status: ProviderStatus::Available { quota },
                    fetched_at: Utc::now(),
                    raw_response: Some(body),
                    auth_source: None,
                    cached_at: None,
                });
            }
        }

        let msg = body
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");

        Ok(ProviderResult {
            kind: ProviderKind::Zai,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: msg.to_string(),
                    console_url: Some("https://open.bigmodel.cn/finance-center/finance/pay".into()),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
            cached_at: None,
        })
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<ProviderQuota> {
        parse_response(body)
    }
}

pub(crate) fn parse_response(body: &serde_json::Value) -> Result<ProviderQuota> {
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct LimitEntry {
        #[serde(rename = "type", default)]
        limit_type: String,
        #[serde(rename = "rawType", default)]
        raw_type: Option<String>,
        #[serde(default)]
        unit: i64,
        #[serde(default)]
        number: i64,
        #[serde(default)]
        usage: Option<i64>,
        #[serde(rename = "currentValue", default)]
        current_value: Option<i64>,
        #[serde(default)]
        remaining: Option<i64>,
        #[serde(default)]
        percentage: Option<f64>,
        #[serde(rename = "nextResetTime", default)]
        next_reset_time: Option<i64>,
    }

    #[derive(Deserialize)]
    struct Data {
        #[serde(default)]
        level: String,
        #[serde(default)]
        limits: Vec<LimitEntry>,
    }

    let data: Data = serde_json::from_value(body.get("data").cloned().unwrap_or_default())
        .map_err(|e| crate::Error::Provider(format!("parse error: {}", e)))?;

    let windows: Vec<QuotaWindow> = data
        .limits
        .iter()
        .map(|l| {
            let raw = l.raw_type.as_deref().unwrap_or(l.limit_type.as_str());
            let label = l.limit_type.as_str();
            let window_type = match raw {
                "TOKENS_LIMIT" => {
                    // unit 3 = hour buckets → 5h; unit 6 = day buckets → weekly.
                    if label.contains("5h") || label.contains("5 Hour") || l.unit == 3 {
                        "5h".to_string()
                    } else if label.contains("Week") || l.unit == 6 {
                        "weekly".to_string()
                    } else {
                        "tokens".to_string()
                    }
                }
                "TIME_LIMIT" => "monthly_mcp".to_string(),
                other => other.to_string(),
            };

            // Z.ai returns two shapes:
            //   (a) full: {usage: limit, currentValue: used, remaining: N}
            //   (b) sparse: only {percentage: X} — seen for near-empty 5h windows
            // Normalize both to used/limit/remaining.
            let (used, limit, remaining) = match (l.usage, l.current_value, l.remaining) {
                (Some(lim), Some(cur), Some(rem)) => (cur, lim, rem),
                (Some(lim), Some(cur), None) => (cur, lim, (lim - cur).max(0)),
                (Some(lim), None, Some(rem)) => ((lim - rem).max(0), lim, rem),
                _ => {
                    // Fall back to percentage → 0-100 scale.
                    let pct = l.percentage.unwrap_or(0.0).clamp(0.0, 100.0);
                    let used = pct.round() as i64;
                    (used, 100, (100 - used).max(0))
                }
            };

            let period_seconds = match l.unit {
                3 => Some(5 * 3600),
                6 => Some(7 * 86400),
                5 => Some(30 * 86400),
                _ => None,
            };

            QuotaWindow {
                window_type,
                used,
                limit,
                remaining,
                reset_at: l
                    .next_reset_time
                    .and_then(|t| Utc.timestamp_millis_opt(t).single()),
                period_seconds,
            }
        })
        .collect();

    let plan_name = if data.level.is_empty() {
        "Z.ai Coding Plan".to_string()
    } else {
        format!("Z.ai {}", data.level)
    };

    Ok(ProviderQuota {
        plan_name,
        windows,
        unlimited: false,
    })
}

#[async_trait]
impl crate::providers::Provider for ZaiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Zai
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = auth.credential.unwrap_token()?.to_string();

        match self.fetch_quota(&key).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Zai,
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
    fn parses_zai_quota_payload() {
        let body = serde_json::json!({
            "code": 200,
            "msg": "success",
            "data": {
                "level": "pro",
                "limits": [
                    {
                        "type": "5h Token",
                        "rawType": "TOKENS_LIMIT",
                        "unit": 3,
                        "number": 5,
                        "usage": 1_000_000i64,
                        "currentValue": 72_000i64,
                        "remaining": 928_000i64,
                        "percentage": 7,
                        "nextResetTime": 1712956800000i64
                    },
                    {
                        "type": "Weekly Token",
                        "rawType": "TOKENS_LIMIT",
                        "unit": 6,
                        "number": 7,
                        "usage": 5_000_000i64,
                        "currentValue": 2_650_000i64,
                        "remaining": 2_350_000i64,
                        "percentage": 53,
                        "nextResetTime": 1713388800000i64
                    },
                    {
                        "type": "MCP usage(1 Month)",
                        "rawType": "TIME_LIMIT",
                        "unit": 5,
                        "number": 1,
                        "usage": 1000,
                        "currentValue": 42,
                        "remaining": 958,
                        "percentage": 4
                    }
                ]
            }
        });

        let quota = parse_response(&body).unwrap();
        assert_eq!(quota.plan_name, "Z.ai pro");
        assert_eq!(quota.windows.len(), 3);
        assert_eq!(quota.windows[0].window_type, "5h");
        assert_eq!(quota.windows[0].limit, 1_000_000);
        assert_eq!(quota.windows[0].used, 72_000);
        assert_eq!(quota.windows[0].remaining, 928_000);
        assert!(quota.windows[0].reset_at.is_some());
        assert_eq!(quota.windows[1].window_type, "weekly");
        assert_eq!(quota.windows[2].window_type, "monthly_mcp");
    }
}
