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
                cached_at: None,
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
            cached_at: None,
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

fn scoped_limit_model_slug(entry: &serde_json::Value) -> Option<String> {
    let model = entry.get("scope")?.get("model")?;
    let name = model
        .get("display_name")
        .and_then(|v| v.as_str())
        .or_else(|| model.get("id").and_then(|v| v.as_str()))?;
    slugify_model_name(name)
}

fn slugify_model_name(name: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut current = String::new();

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            current.extend(ch.to_lowercase());
        } else if !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("_"))
    }
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

    if let Some(obj) = body.as_object() {
        for (key, value) in obj {
            let Some(model) = key.strip_prefix("seven_day_") else {
                continue;
            };
            if model.is_empty() || matches!(model, "opus" | "sonnet" | "oauth_apps") {
                continue;
            }
            let rate_limit = serde_json::from_value(value.clone()).ok();
            push(&mut windows, &format!("weekly_{model}"), rate_limit);
        }
    }

    if let Some(limits) = body.get("limits").and_then(|v| v.as_array()) {
        for entry in limits {
            if entry.get("group").and_then(|v| v.as_str()) != Some("weekly") {
                continue;
            }
            let Some(model) = scoped_limit_model_slug(entry) else {
                continue;
            };
            let name = format!("weekly_{model}");
            if windows.iter().any(|w| w.window_type == name) {
                continue;
            }
            let rate_limit = RateLimit {
                utilization: entry.get("percent").and_then(|v| v.as_f64()),
                resets_at: entry
                    .get("resets_at")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            };
            push(&mut windows, &name, Some(rate_limit));
        }
    }

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
        banked_resets: None,
    }
}

#[async_trait]
impl crate::providers::Provider for ClaudeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let token = auth.credential.unwrap_token()?.to_string();
        let is_oauth =
            matches!(&auth.credential, AuthCredential::Token(_)) || token.starts_with("sk-ant-oat");

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
    fn parses_generic_model_specific_weekly_limits() {
        let body = serde_json::json!({
            "five_hour": {"utilization": 42.0, "resets_at": "2026-04-13T10:00:00Z"},
            "seven_day_fable": {"utilization": 23.0, "resets_at": "2026-04-20T10:00:00Z"},
            "seven_day_oracle": {"utilization": 67.0, "resets_at": "2026-04-20T10:00:00Z"}
        });

        let quota = parse_usage(&body);
        let labels: Vec<_> = quota
            .windows
            .iter()
            .map(|w| w.window_type.as_str())
            .collect();

        assert_eq!(labels, vec!["5h", "weekly_fable", "weekly_oracle"]);

        let fable = quota
            .windows
            .iter()
            .find(|w| w.window_type == "weekly_fable")
            .expect("fable weekly limit should be surfaced");
        assert_eq!(fable.used, 23);
        assert_eq!(fable.limit, 100);
        assert_eq!(fable.remaining, 77);
        assert_eq!(fable.period_seconds, Some(7 * 86400));
        assert!(fable.reset_at.is_some());
    }

    #[test]
    fn parses_model_specific_weekly_limits_from_limits_array() {
        let body = serde_json::json!({
            "five_hour": {"utilization": 6.0, "resets_at": "2026-07-06T07:00:00Z"},
            "seven_day": {"utilization": 59.0, "resets_at": "2026-07-08T12:00:00Z"},
            "limits": [
                {
                    "group": "weekly",
                    "kind": "weekly_scoped",
                    "percent": 47,
                    "resets_at": "2026-07-08T12:00:00.132115+00:00",
                    "scope": {
                        "model": {"display_name": "Fable", "id": null},
                        "surface": null
                    }
                }
            ]
        });

        let quota = parse_usage(&body);
        let fable = quota
            .windows
            .iter()
            .find(|w| w.window_type == "weekly_fable")
            .expect("Fable weekly limit should be surfaced from limits[]");

        assert_eq!(fable.used, 47);
        assert_eq!(fable.limit, 100);
        assert_eq!(fable.remaining, 53);
        assert_eq!(fable.period_seconds, Some(7 * 86400));
        assert!(fable.reset_at.is_some());
    }

    #[test]
    fn parses_empty_usage_payload() {
        let body = serde_json::json!({});
        let quota = parse_usage(&body);
        assert_eq!(quota.windows.len(), 0);
    }
}
