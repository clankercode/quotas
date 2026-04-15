use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;

pub struct KimiProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl KimiProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_payg(&self, key: &str) -> Result<ProviderResult> {
        let url = "https://api.moonshot.ai/v1/users/me/balance";
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 200 {
            let data = body.get("data").cloned().unwrap_or_default();
            let available = data
                .get("available_balance")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            return Ok(ProviderResult {
                kind: ProviderKind::Kimi,
                status: ProviderStatus::Available {
                    quota: ProviderQuota {
                        plan_name: "Kimi PAYG".to_string(),
                        windows: vec![QuotaWindow {
                            window_type: "payg_balance".to_string(),
                            used: 0,
                            limit: 0,
                            remaining: (available * 100.0) as i64,
                            reset_at: None,
                            period_seconds: None,
                        }],
                        unlimited: false,
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            });
        }

        let msg = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");

        Ok(ProviderResult {
            kind: ProviderKind::Kimi,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: msg.to_string(),
                    console_url: Some("https://platform.kimi.ai/".into()),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
            cached_at: None,
        })
    }

    async fn fetch_coding(&self, key: &str) -> Result<ProviderResult> {
        let url = "https://api.kimi.com/coding/v1/usages";
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 200 {
            let quota = parse_coding_response(&body);
            return Ok(ProviderResult {
                kind: ProviderKind::Kimi,
                status: ProviderStatus::Available { quota },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            });
        }

        self.fetch_payg(key).await
    }
}

/// Some fields come back as strings ("100") or as integers (100). Accept either.
fn num_field(v: &serde_json::Value, key: &str) -> i64 {
    match v.get(key) {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn parse_reset(v: &serde_json::Value) -> Option<DateTime<Utc>> {
    let s = v
        .get("reset_at")
        .or_else(|| v.get("resetTime"))
        .or_else(|| v.get("resetAt"))
        .and_then(|x| x.as_str())?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub(crate) fn parse_coding_response(body: &serde_json::Value) -> ProviderQuota {
    let usage = body.get("usage").cloned().unwrap_or_default();
    let limits = body
        .get("limits")
        .and_then(|l| l.as_array())
        .cloned()
        .unwrap_or_default();

    let mut windows = Vec::new();

    let weekly_limit = num_field(&usage, "limit");
    let weekly_used = num_field(&usage, "used");
    let weekly_remaining = if usage.get("remaining").is_some() {
        num_field(&usage, "remaining")
    } else {
        (weekly_limit - weekly_used).max(0)
    };
    let reset_at = parse_reset(&usage);

    if weekly_limit > 0 {
        windows.push(QuotaWindow {
            window_type: "weekly".to_string(),
            used: weekly_used,
            limit: weekly_limit,
            remaining: weekly_remaining,
            reset_at,
            period_seconds: Some(7 * 86400),
        });
    }

    for limit_entry in limits {
        let detail = limit_entry.get("detail").cloned().unwrap_or_default();
        let window = limit_entry.get("window").cloned().unwrap_or_default();
        let limit = num_field(&detail, "limit");
        let used = if detail.get("used").is_some() {
            num_field(&detail, "used")
        } else {
            let remaining = num_field(&detail, "remaining");
            (limit - remaining).max(0)
        };
        let remaining = if detail.get("remaining").is_some() {
            num_field(&detail, "remaining")
        } else {
            (limit - used).max(0)
        };
        let duration = window
            .get("duration")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                window
                    .get("duration")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
            })
            .unwrap_or(0);
        let unit = window
            .get("timeUnit")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let unit_norm = unit.trim_start_matches("TIME_UNIT_");

        let window_type = match (duration, unit_norm) {
            (300, "MINUTE") => "5h".to_string(),
            (0, _) => detail
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("window")
                .to_string(),
            _ => format!("{}{}", duration, unit_norm.chars().next().unwrap_or(' ')),
        };

        let unit_secs: i64 = match unit_norm {
            "SECOND" => 1,
            "MINUTE" => 60,
            "HOUR" => 3600,
            "DAY" => 86400,
            "WEEK" => 7 * 86400,
            _ => 0,
        };
        let period_seconds = if duration > 0 && unit_secs > 0 {
            Some(duration * unit_secs)
        } else {
            None
        };

        let entry_reset = parse_reset(&detail);
        windows.push(QuotaWindow {
            window_type,
            used,
            limit,
            remaining,
            reset_at: entry_reset,
            period_seconds,
        });
    }

    ProviderQuota {
        plan_name: "Kimi Coding Plan".to_string(),
        windows,
        unlimited: false,
    }
}

#[async_trait]
impl crate::providers::Provider for KimiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Kimi
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = auth.credential.unwrap_token()?.to_string();

        match self.fetch_coding(&key).await {
            Ok(r) => Ok(r),
            Err(_) => self.fetch_payg(&key).await,
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
    fn parses_kimi_coding_usages() {
        let body = serde_json::json!({
            "usage": {
                "limit": 1_000_000i64,
                "used": 400_000i64,
                "remaining": 600_000i64,
                "name": "Weekly limit",
                "reset_at": "2026-04-14T00:00:00Z"
            },
            "limits": [{
                "detail": {
                    "limit": 200_000i64,
                    "used": 50_000i64,
                    "remaining": 150_000i64,
                    "name": "5h limit"
                },
                "window": {"duration": 300, "timeUnit": "MINUTE"}
            }]
        });
        let quota = parse_coding_response(&body);
        assert_eq!(quota.windows.len(), 2);
        let weekly = &quota.windows[0];
        assert_eq!(weekly.window_type, "weekly");
        assert_eq!(weekly.limit, 1_000_000);
        assert_eq!(weekly.used, 400_000);
        assert_eq!(weekly.remaining, 600_000);
        assert!(weekly.reset_at.is_some());
        let five = &quota.windows[1];
        assert_eq!(five.window_type, "5h");
        assert_eq!(five.used, 50_000);
        assert_eq!(five.remaining, 150_000);
    }
}
