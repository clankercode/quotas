use crate::auth::{AuthCredential, AuthResolver};
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

pub struct ClaudeProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl ClaudeProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_usage(&self, token: &str, is_oauth: bool) -> Result<ProviderResult> {
        let url = "https://api.anthropic.com/api/oauth/usage";
        let mut req = self
            .http
            .get(url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "quotas-cli/0.1");

        if is_oauth {
            req = req
                .header("Authorization", format!("Bearer {}", token))
                .header("anthropic-beta", "oauth-2025-04-20");
        } else {
            req = req.header("x-api-key", token);
        }

        let resp = req.send().await?;
        let status = resp.status();
        let body_text = resp.text().await?;
        let body: serde_json::Value =
            serde_json::from_str(&body_text).unwrap_or(serde_json::Value::Null);

        if status.is_success() {
            let quota = parse_usage(&body);
            return Ok(ProviderResult {
                kind: ProviderKind::Claude,
                status: ProviderStatus::Available { quota },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
            });
        }

        let reason = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));

        let auth_problem = status.as_u16() == 401 || status.as_u16() == 403;

        Ok(ProviderResult {
            kind: ProviderKind::Claude,
            status: if auth_problem {
                ProviderStatus::AuthRequired
            } else {
                ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason,
                        console_url: Some("https://claude.ai/settings/usage".into()),
                    },
                }
            },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
        })
    }
}

#[derive(Deserialize)]
struct RateLimit {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct Utilization {
    five_hour: Option<RateLimit>,
    seven_day: Option<RateLimit>,
    seven_day_opus: Option<RateLimit>,
    seven_day_sonnet: Option<RateLimit>,
    seven_day_oauth_apps: Option<RateLimit>,
    extra_usage: Option<ExtraUsage>,
}

#[derive(Deserialize)]
struct ExtraUsage {
    #[serde(default)]
    is_enabled: bool,
    monthly_limit: Option<f64>,
    used_credits: Option<f64>,
}

pub(crate) fn parse_usage(body: &serde_json::Value) -> ProviderQuota {
    let parsed: Utilization = serde_json::from_value(body.clone()).unwrap_or(Utilization {
        five_hour: None,
        seven_day: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        seven_day_oauth_apps: None,
        extra_usage: None,
    });

    let mut windows: Vec<QuotaWindow> = Vec::new();

    let push = |windows: &mut Vec<QuotaWindow>, name: &str, rl: Option<RateLimit>| {
        let Some(rl) = rl else { return };
        let Some(util_pct) = rl.utilization else {
            return;
        };
        let used = util_pct.round() as i64;
        let reset_at = rl
            .resets_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let period_seconds = match name {
            "5h" => Some(5 * 3600),
            _ => Some(7 * 86400),
        };
        windows.push(QuotaWindow {
            window_type: name.to_string(),
            used,
            limit: 100,
            remaining: (100 - used).max(0),
            reset_at,
            period_seconds,
        });
    };

    push(&mut windows, "5h", parsed.five_hour);
    push(&mut windows, "weekly", parsed.seven_day);
    push(&mut windows, "weekly_opus", parsed.seven_day_opus);
    push(&mut windows, "weekly_sonnet", parsed.seven_day_sonnet);
    push(
        &mut windows,
        "weekly_oauth_apps",
        parsed.seven_day_oauth_apps,
    );

    // Extra usage (monthly paid credits topping up the base plan).
    if let Some(extra) = parsed.extra_usage {
        if extra.is_enabled {
            if let (Some(limit), Some(used)) = (extra.monthly_limit, extra.used_credits) {
                let limit_i = limit.round() as i64;
                let used_i = used.round() as i64;
                windows.push(QuotaWindow {
                    window_type: "extra_credits".to_string(),
                    used: used_i,
                    limit: limit_i,
                    remaining: (limit_i - used_i).max(0),
                    reset_at: None,
                    period_seconds: Some(30 * 86400),
                });
            }
        }
    }

    ProviderQuota {
        plan_name: "Claude (Max/Pro)".to_string(),
        windows,
        unlimited: false,
    }
}

#[async_trait]
impl crate::providers::Provider for ClaudeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let (token, is_oauth) = match &auth.credential {
            AuthCredential::Token(t) => (t.clone(), true),
            AuthCredential::Bearer(k) => {
                let is_oauth = k.starts_with("sk-ant-oat");
                (k.clone(), is_oauth)
            }
        };

        match self.fetch_usage(&token, is_oauth).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Claude,
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
    fn parses_full_usage_payload() {
        let body = serde_json::json!({
            "five_hour": {"utilization": 42.0, "resets_at": "2026-04-13T10:00:00Z"},
            "seven_day": {"utilization": 10.0, "resets_at": "2026-04-20T10:00:00Z"},
            "seven_day_opus": {"utilization": 5.0, "resets_at": "2026-04-20T10:00:00Z"},
            "seven_day_sonnet": {"utilization": 15.0, "resets_at": "2026-04-20T10:00:00Z"},
            "extra_usage": null
        });
        let quota = parse_usage(&body);
        assert_eq!(quota.windows.len(), 4);
        let five = &quota.windows[0];
        assert_eq!(five.window_type, "5h");
        assert_eq!(five.used, 42);
        assert_eq!(five.remaining, 58);
        assert!(five.reset_at.is_some());
    }

    #[test]
    fn parses_empty_usage_payload() {
        let body = serde_json::json!({});
        let quota = parse_usage(&body);
        assert_eq!(quota.windows.len(), 0);
    }
}
