use crate::auth::{AuthResolver, MultiResolver};
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::{Error, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;

pub struct MinimaxProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl MinimaxProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    pub fn with_multi_resolver(auth: MultiResolver) -> Self {
        Self {
            http: Client::new(),
            auth: Box::new(auth),
        }
    }

    async fn fetch_intl(&self, key: &str) -> Result<ProviderResult> {
        let url = "https://api.minimax.io/v1/api/openplatform/coding_plan/remains";
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
            let code = body.get("base_resp").and_then(|b| b.get("status_code"));
            if code == Some(&serde_json::json!(0)) || code.is_none() {
                let quota = self.parse_response(&body)?;
                return Ok(ProviderResult {
                    kind: ProviderKind::Minimax,
                    status: ProviderStatus::Available { quota },
                    fetched_at: Utc::now(),
                    raw_response: Some(body),
                    auth_source: None,
                });
            }
        }

        let msg = body
            .get("base_resp")
            .and_then(|b| b.get("status_msg"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");

        Ok(ProviderResult {
            kind: ProviderKind::Minimax,
            status: ProviderStatus::Unavailable {
                info: crate::providers::UnavailableInfo {
                    reason: msg.to_string(),
                    console_url: Some(
                        "https://platform.minimax.io/user-center/payment/coding-plan".into(),
                    ),
                },
            },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
        })
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<ProviderQuota> {
        parse_response(body)
    }
}

pub(crate) fn parse_response(body: &serde_json::Value) -> Result<ProviderQuota> {
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct ModelRemain {
        model_name: String,
        #[serde(default)]
        start_time: i64,
        #[serde(default)]
        end_time: i64,
        #[serde(default)]
        remains_time: i64,
        #[serde(rename = "current_interval_total_count", default)]
        total_count: i64,
        #[serde(rename = "current_interval_usage_count", default)]
        usage_count: i64,
        #[serde(rename = "current_weekly_total_count", default)]
        weekly_total: i64,
        #[serde(rename = "current_weekly_usage_count", default)]
        weekly_usage: i64,
        #[serde(rename = "weekly_end_time", default)]
        weekly_end_time: i64,
        #[serde(rename = "weekly_start_time", default)]
        weekly_start_time: i64,
    }

    #[derive(Deserialize)]
    struct Response {
        model_remains: Vec<ModelRemain>,
    }

    let resp: Response = serde_json::from_value(body.clone())
        .map_err(|e| Error::Provider(format!("parse error: {}", e)))?;

    // Prefer the coding-plan / MiniMax-M* model as the "primary" card model.
    // Others (TTS / image / music / video) are still surfaced but sorted after.
    let is_coding = |name: &str| {
        let n = name.to_ascii_lowercase();
        n.starts_with("minimax-m") || n.starts_with("coding-plan")
    };

    let plan_name = resp
        .model_remains
        .iter()
        .find(|m| is_coding(&m.model_name))
        .or_else(|| resp.model_remains.first())
        .map(|m| m.model_name.clone())
        .unwrap_or_else(|| "MiniMax Coding Plan".to_string());

    let mut ordered: Vec<&ModelRemain> = resp.model_remains.iter().collect();
    ordered.sort_by_key(|m| if is_coding(&m.model_name) { 0 } else { 1 });

    let mut windows: Vec<QuotaWindow> = Vec::new();
    for m in ordered {
        // 5h window. NOTE: despite its name, `current_interval_usage_count`
        // is the count REMAINING, not consumed — same quirk as `remains_time`.
        // Verified against the openclaw minimax-usage.sh reference implementation.
        if m.total_count > 0 {
            let limit = m.total_count;
            let remaining = m.usage_count.clamp(0, limit);
            let used = limit - remaining;
            let label = format!("5h/{}", short_model_name(&m.model_name));
            let period_seconds = if m.end_time > m.start_time && m.start_time > 0 {
                Some((m.end_time - m.start_time) / 1000)
            } else {
                Some(18000)
            };
            windows.push(QuotaWindow {
                window_type: label,
                used,
                limit,
                remaining,
                reset_at: Utc.timestamp_millis_opt(m.end_time).single(),
                period_seconds,
            });
        }
        // Weekly window — same inverted-naming quirk as the 5h field above.
        if m.weekly_total > 0 {
            let limit = m.weekly_total;
            let remaining = m.weekly_usage.clamp(0, limit);
            let used = limit - remaining;
            let label = format!("wk/{}", short_model_name(&m.model_name));
            let period_seconds =
                if m.weekly_end_time > m.weekly_start_time && m.weekly_start_time > 0 {
                    Some((m.weekly_end_time - m.weekly_start_time) / 1000)
                } else {
                    Some(7 * 86400)
                };
            windows.push(QuotaWindow {
                window_type: label,
                used,
                limit,
                remaining,
                reset_at: Utc.timestamp_millis_opt(m.weekly_end_time).single(),
                period_seconds,
            });
        }
    }

    Ok(ProviderQuota {
        plan_name: format!("MiniMax · {}", plan_name),
        windows,
        unlimited: false,
    })
}

fn short_model_name(name: &str) -> String {
    // Strip common prefixes and length-limit for compact labels.
    let s = name
        .strip_prefix("MiniMax-")
        .or_else(|| name.strip_prefix("minimax-"))
        .unwrap_or(name);
    s.to_string()
}

#[async_trait]
impl crate::providers::Provider for MinimaxProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Minimax
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = auth.credential.unwrap_token()?.to_string();

        match self.fetch_intl(&key).await {
            Ok(mut r) => {
                r.raw_response = Some(r.raw_response.unwrap_or_default());
                Ok(r)
            }
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Minimax,
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
    fn parses_minimax_remains_payload() {
        let body = serde_json::json!({
            "base_resp": {"status_code": 0, "status_msg": ""},
            "model_remains": [{
                "model_name": "MiniMax-M2.7",
                "start_time": 1712937600000i64,
                "end_time": 1712955600000i64,
                "remains_time": 14400000i64,
                "current_interval_total_count": 200,
                "current_interval_usage_count": 187,
                "current_weekly_total_count": 1000,
                "current_weekly_usage_count": 850,
                "weekly_end_time": 1713542400000i64,
                "weekly_start_time": 1712937600000i64,
                "weekly_remains_time": 604800000i64
            }]
        });
        let quota = parse_response(&body).unwrap();
        assert!(quota.plan_name.contains("M2.7"));
        // One 5h + one weekly window for the single model.
        assert_eq!(quota.windows.len(), 2);
        // usage_count fields are REMAINING, not used (API naming quirk).
        let five = &quota.windows[0];
        assert!(five.window_type.starts_with("5h"));
        assert_eq!(five.limit, 200);
        assert_eq!(five.remaining, 187);
        assert_eq!(five.used, 13);
        assert!(five.reset_at.is_some());
        let weekly = &quota.windows[1];
        assert!(weekly.window_type.starts_with("wk"));
        assert_eq!(weekly.limit, 1000);
        assert_eq!(weekly.remaining, 850);
        assert_eq!(weekly.used, 150);
    }

    #[test]
    fn coding_plan_model_sorts_first() {
        let body = serde_json::json!({
            "base_resp": {"status_code": 0, "status_msg": ""},
            "model_remains": [
                {
                    "model_name": "image-01",
                    "start_time": 0, "end_time": 1,
                    "remains_time": 0,
                    "current_interval_total_count": 10,
                    "current_interval_usage_count": 3,
                    "current_weekly_total_count": 0,
                    "current_weekly_usage_count": 0,
                    "weekly_end_time": 0
                },
                {
                    "model_name": "MiniMax-M2.7",
                    "start_time": 0, "end_time": 1,
                    "remains_time": 0,
                    "current_interval_total_count": 100,
                    "current_interval_usage_count": 50,
                    "current_weekly_total_count": 0,
                    "current_weekly_usage_count": 0,
                    "weekly_end_time": 0
                }
            ]
        });
        let quota = parse_response(&body).unwrap();
        assert!(quota.plan_name.contains("M2.7"));
        // Coding-plan model's window first.
        assert_eq!(quota.windows[0].limit, 100);
    }
}
