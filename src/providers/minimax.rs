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
                    cached_at: None,
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
        #[serde(default)]
        current_interval_remaining_percent: Option<u8>,
        #[serde(default)]
        current_interval_status: Option<i32>,
        #[serde(default)]
        current_weekly_remaining_percent: Option<u8>,
        #[serde(default)]
        current_weekly_status: Option<i32>,
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
        } else if let Some(pct) = m.current_interval_remaining_percent {
            let pct = (pct as i64).min(100);
            let limit: i64 = 100;
            let remaining = pct;
            let used = (100 - pct).clamp(0, 100);
            let label = format!("5h/{}", short_model_name(&m.model_name));
            windows.push(QuotaWindow {
                window_type: label,
                used,
                limit,
                remaining,
                reset_at: Utc.timestamp_millis_opt(m.end_time).single(),
                period_seconds: Some(18000),
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
        } else if let Some(pct) = m.current_weekly_remaining_percent {
            let pct = (pct as i64).min(100);
            let limit: i64 = 100;
            let remaining = pct;
            let used = (100 - pct).clamp(0, 100);
            let label = format!("wk/{}", short_model_name(&m.model_name));
            windows.push(QuotaWindow {
                window_type: label,
                used,
                limit,
                remaining,
                reset_at: Utc.timestamp_millis_opt(m.weekly_end_time).single(),
                period_seconds: Some(7 * 86400),
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
    let s = name
        .strip_prefix("MiniMax-")
        .or_else(|| name.strip_prefix("minimax-"))
        .unwrap_or(name);
    let s = if s.starts_with("coding-plan-") {
        s.strip_prefix("coding-plan-")
            .map(|rest| format!("c-plan-{}", rest))
    } else {
        None
    }
    .unwrap_or_else(|| s.to_string());
    let s = if s.starts_with("Hailuo-2.3-") {
        s.strip_prefix("Hailou-2.3-")
            .map(|rest| format!("H2.3-{}", rest))
    } else {
        None
    }
    .unwrap_or(s);
    let s = s.replace("lyrics_generation", "lyrics_gen");
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
    use std::path::PathBuf;

    fn fixture(name: &str) -> serde_json::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/minimax")
            .join(name);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {}", path.display(), e));
        serde_json::from_str(&raw).expect("parse fixture json")
    }

    #[test]
    fn percent_only_model_emits_two_windows() {
        let body = serde_json::json!({
            "base_resp": {"status_code": 0, "status_msg": ""},
            "model_remains": [{
                "model_name": "general",
                "start_time": 0, "end_time": 1, "remains_time": 0,
                "current_interval_total_count": 0,
                "current_interval_usage_count": 0,
                "current_interval_remaining_percent": 99,
                "current_interval_status": 1,
                "current_weekly_total_count": 0,
                "current_weekly_usage_count": 0,
                "current_weekly_remaining_percent": 98,
                "current_weekly_status": 1,
                "weekly_end_time": 0
            }]
        });
        let quota = parse_response(&body).unwrap();
        assert_eq!(quota.windows.len(), 2);
        let five = quota.windows.iter().find(|w| w.window_type.starts_with("5h")).expect("5h window");
        assert_eq!(five.limit, 100);
        assert_eq!(five.remaining, 99);
        assert_eq!(five.used, 1);
        let weekly = quota.windows.iter().find(|w| w.window_type.starts_with("wk")).expect("wk window");
        assert_eq!(weekly.limit, 100);
        assert_eq!(weekly.remaining, 98);
        assert_eq!(weekly.used, 2);
    }

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
    fn unknown_models_fall_back_to_static_plan_name() {
        let body = serde_json::json!({
            "base_resp": {"status_code": 0, "status_msg": ""},
            "model_remains": [{
                "model_name": "image-01",
                "start_time": 0, "end_time": 1, "remains_time": 0,
                "current_interval_total_count": 10,
                "current_interval_usage_count": 3,
                "current_interval_remaining_percent": 100,
                "current_interval_status": 1,
                "current_weekly_total_count": 0,
                "current_weekly_usage_count": 0,
                "current_weekly_remaining_percent": 100,
                "current_weekly_status": 1,
                "weekly_end_time": 0
            }]
        });
        let quota = parse_response(&body).unwrap();
        assert_eq!(quota.plan_name, "MiniMax · MiniMax Coding Plan");
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

    #[test]
    fn parses_live_fixture_mixed_count_and_percent() {
        let now = chrono::Utc::now().timestamp_millis();
        let mut body = fixture("coding_plan_remains_live.json");
        for m in body["model_remains"].as_array_mut().unwrap() {
            m["start_time"] = serde_json::json!(now - 3_600_000);
            m["end_time"] = serde_json::json!(now + 4 * 3_600_000);
            m["weekly_start_time"] = serde_json::json!(now - 6 * 86_400_000);
            m["weekly_end_time"] = serde_json::json!(now + 1 * 86_400_000);
        }
        let quota = parse_response(&body).unwrap();
        assert_eq!(quota.plan_name, "MiniMax · MiniMax Coding Plan");
        assert_eq!(quota.windows.len(), 4);

        let five_g = quota
            .windows
            .iter()
            .find(|w| w.window_type == "5h/general")
            .expect("5h/general");
        assert_eq!(five_g.limit, 100);
        assert_eq!(five_g.remaining, 99);
        assert_eq!(five_g.used, 1);

        let wk_g = quota
            .windows
            .iter()
            .find(|w| w.window_type == "wk/general")
            .expect("wk/general");
        assert_eq!(wk_g.limit, 100);
        assert_eq!(wk_g.remaining, 98);
        assert_eq!(wk_g.used, 2);

        let five_v = quota
            .windows
            .iter()
            .find(|w| w.window_type == "5h/video")
            .expect("5h/video");
        assert_eq!(five_v.limit, 3);
        assert_eq!(five_v.remaining, 3);
        assert_eq!(five_v.used, 0);

        let wk_v = quota
            .windows
            .iter()
            .find(|w| w.window_type == "wk/video")
            .expect("wk/video");
        assert_eq!(wk_v.limit, 21);
        assert_eq!(wk_v.remaining, 21);
        assert_eq!(wk_v.used, 0);
    }

    #[test]
    fn depleted_window_status_zero_still_renders() {
        let body = serde_json::json!({
            "base_resp": {"status_code": 0, "status_msg": ""},
            "model_remains": [{
                "model_name": "general",
                "start_time": 0, "end_time": 1, "remains_time": 0,
                "current_interval_total_count": 0,
                "current_interval_usage_count": 0,
                "current_interval_remaining_percent": 0,
                "current_interval_status": 0,
                "current_weekly_total_count": 0,
                "current_weekly_usage_count": 0,
                "current_weekly_remaining_percent": 0,
                "current_weekly_status": 0,
                "weekly_end_time": 0
            }]
        });
        let quota = parse_response(&body).unwrap();
        let five = quota.windows.iter().find(|w| w.window_type.starts_with("5h")).expect("5h window");
        assert_eq!(five.limit, 100);
        assert_eq!(five.remaining, 0);
        assert_eq!(five.used, 100);
    }
}
