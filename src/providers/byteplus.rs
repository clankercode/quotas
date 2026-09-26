use crate::auth::AuthResolver;
use crate::providers::{
    ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow, UnavailableInfo,
};
use crate::{Error, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const HOST: &str = "open.byteplusapi.com";
const REGION: &str = "ap-southeast-1";
const SERVICE: &str = "ark";
const API_VERSION: &str = "2024-01-01";
const ACTION: &str = "GetCodingPlanUsage";
const CONSOLE_URL: &str =
    "https://console.byteplus.com/ark/region:ark+ap-southeast-1/subscription/coding-plan";

/// Reads the personal Coding Plan quota snapshot from BytePlus ModelArk.
///
/// The console's usage page is backed by the `GetCodingPlanUsage` control-plane
/// action, which rejects the data-plane `ark-` bearer key and requires a
/// Volcengine v4 request signature. Auth is an access-key/secret-key pair
/// rather than a token.
pub struct ByteplusProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl ByteplusProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_quota(&self, access_key: &str, secret_key: &str) -> Result<ProviderResult> {
        // Canonical query: keys sorted. Both values are RFC3986-safe as-is.
        let canonical_query = format!("Action={}&Version={}", ACTION, API_VERSION);
        let now = Utc::now();
        let x_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let short_date = &x_date[..8];
        let content_type = "application/json; charset=utf-8";

        // A GET request doesn't have a body. The payload hash is the digest of
        // the empty string.
        let payload_hash = hex::encode(Sha256::digest(b""));
        let signed_headers = "content-type;host;x-content-sha256;x-date";
        let canonical_headers = format!(
            "content-type:{}\nhost:{}\nx-content-sha256:{}\nx-date:{}\n",
            content_type, HOST, payload_hash, x_date
        );
        let canonical_request = format!(
            "GET\n/\n{}\n{}\n{}\n{}",
            canonical_query, canonical_headers, signed_headers, payload_hash
        );
        let scope = format!("{}/{}/{}/request", short_date, REGION, SERVICE);
        let string_to_sign = format!(
            "HMAC-SHA256\n{}\n{}\n{}",
            x_date,
            scope,
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );

        let k_date = hmac_sign(secret_key.as_bytes(), short_date.as_bytes());
        let k_region = hmac_sign(&k_date, REGION.as_bytes());
        let k_service = hmac_sign(&k_region, SERVICE.as_bytes());
        let k_signing = hmac_sign(&k_service, b"request");
        let signature = hex::encode(hmac_sign(&k_signing, string_to_sign.as_bytes()));

        let authorization = format!(
            "HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            access_key, scope, signed_headers, signature
        );

        let url = format!("https://{}/?{}", HOST, canonical_query);
        let resp = self
            .http
            .get(&url)
            .header("Host", HOST)
            .header("X-Date", &x_date)
            .header("X-Content-Sha256", &payload_hash)
            .header("Content-Type", content_type)
            .header("Authorization", authorization)
            .send()
            .await?;

        let body: serde_json::Value = resp.json().await?;
        Ok(build_result(body))
    }
}

fn hmac_sign(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

/// Turns a `GetCodingPlanUsage` response into a [`ProviderResult`], mapping a
/// backend error envelope or an inactive plan to an Unavailable status.
fn build_result(body: serde_json::Value) -> ProviderResult {
    let make = |status: ProviderStatus| ProviderResult {
        kind: ProviderKind::Byteplus,
        status,
        fetched_at: Utc::now(),
        raw_response: Some(body.clone()),
        auth_source: None,
        cached_at: None,
        instance_key: None,
        profile_id: None,
        card_title: None,
    };

    if let Some(err) = body
        .get("ResponseMetadata")
        .and_then(|m| m.get("Error"))
        .filter(|e| !e.is_null())
    {
        let msg = err
            .get("Message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return make(ProviderStatus::Unavailable {
            info: UnavailableInfo {
                reason: msg.to_string(),
                console_url: Some(CONSOLE_URL.to_string()),
            },
        });
    }

    match parse_quota(&body) {
        Some(quota) => make(ProviderStatus::Available { quota }),
        None => make(ProviderStatus::Unavailable {
            info: UnavailableInfo {
                reason: "no active Coding Plan subscription".to_string(),
                console_url: Some(CONSOLE_URL.to_string()),
            },
        }),
    }
}

/// Parses the `Result` block into a [`ProviderQuota`], or `None` when the plan
/// is inactive: status isn't running, or the usage windows list is empty.
fn parse_quota(body: &serde_json::Value) -> Option<ProviderQuota> {
    #[derive(Deserialize)]
    struct Entry {
        #[serde(rename = "Level", default)]
        level: String,
        #[serde(rename = "Percent", default)]
        percent: f64,
        #[serde(rename = "Cap", default)]
        cap: f64,
        #[serde(rename = "ResetTimestamp", default)]
        reset_timestamp: Option<i64>,
    }

    #[derive(Deserialize)]
    struct ResultBlock {
        #[serde(rename = "Status", default)]
        status: String,
        #[serde(rename = "QuotaUsage", default)]
        quota_usage: Vec<Entry>,
    }

    let result: ResultBlock = serde_json::from_value(body.get("Result").cloned()?).ok()?;
    if result.quota_usage.is_empty() || !result.status.eq_ignore_ascii_case("running") {
        return None;
    }

    let windows = result
        .quota_usage
        .iter()
        .map(|e| {
            // Personal plans report only a percentage against a cap of 100, so
            // used/limit/remaining are a 0-100 scale rather than token counts.
            let limit = if e.cap > 0.0 { e.cap.round() as i64 } else { 100 };
            let used = e.percent.round().clamp(0.0, limit as f64) as i64;
            QuotaWindow {
                window_type: window_type_for(&e.level),
                used,
                limit,
                remaining: (limit - used).max(0),
                // ResetTimestamp is a -1 sentinel when a window is unused.
                reset_at: e
                    .reset_timestamp
                    .filter(|t| *t > 0)
                    .and_then(|t| Utc.timestamp_opt(t, 0).single()),
                period_seconds: period_seconds_for(&e.level),
            }
        })
        .collect();

    Some(ProviderQuota {
        plan_name: "BytePlus Coding Plan".to_string(),
        windows,
        unlimited: false,
        banked_resets: None,
    })
}

/// Maps a BytePlus window level onto the shared quota vocabulary. The "session"
/// window is the plan's 5-hour rolling bucket.
fn window_type_for(level: &str) -> String {
    match level {
        "session" => "5h",
        "weekly" => "weekly",
        "monthly" => "monthly",
        other => other,
    }
    .to_string()
}

fn period_seconds_for(level: &str) -> Option<i64> {
    match level {
        "session" => Some(5 * 3600),
        "weekly" => Some(7 * 86400),
        "monthly" => Some(30 * 86400),
        _ => None,
    }
}

#[async_trait]
impl crate::providers::Provider for ByteplusProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Byteplus
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let (access_key, secret_key) = auth.credential.unwrap_aksk()?;

        match self.fetch_quota(access_key, secret_key).await {
            Ok(r) => Ok(r),
            Err(Error::Auth(msg)) => Err(Error::Auth(msg)),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Byteplus,
                status: ProviderStatus::NetworkError {
                    message: e.to_string(),
                },
                fetched_at: Utc::now(),
                raw_response: None,
                auth_source: None,
                cached_at: None,
                instance_key: None,
                profile_id: None,
                card_title: None,
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
    fn parses_coding_plan_usage() {
        let body = serde_json::json!({
            "ResponseMetadata": {
                "RequestId": "abc",
                "Action": "GetCodingPlanUsage",
                "Version": "2024-01-01"
            },
            "Result": {
                "Status": "Running",
                "UpdateTimestamp": 1790366527i64,
                "QuotaUsage": [
                    {"Level": "session", "Percent": 0.844, "ResetTimestamp": 1790384238i64, "Cap": 100},
                    {"Level": "weekly", "Percent": 12.798, "ResetTimestamp": 1790524800i64, "Cap": 100},
                    {"Level": "monthly", "Percent": 6.399, "ResetTimestamp": 1792943999i64, "Cap": 100}
                ],
                "HasReward": false
            }
        });

        let result = build_result(body);
        let quota = match result.status {
            ProviderStatus::Available { quota } => quota,
            other => panic!("expected available, got {:?}", other),
        };
        assert_eq!(quota.plan_name, "BytePlus Coding Plan");
        assert_eq!(quota.windows.len(), 3);
        assert_eq!(quota.windows[0].window_type, "5h");
        assert_eq!(quota.windows[0].used, 1);
        assert_eq!(quota.windows[0].limit, 100);
        assert_eq!(quota.windows[0].remaining, 99);
        assert!(quota.windows[0].reset_at.is_some());
        assert_eq!(quota.windows[0].period_seconds, Some(5 * 3600));
        assert_eq!(quota.windows[1].window_type, "weekly");
        assert_eq!(quota.windows[1].used, 13);
        assert_eq!(quota.windows[2].window_type, "monthly");
    }

    #[test]
    fn maps_error_envelope_to_unavailable() {
        let body = serde_json::json!({
            "ResponseMetadata": {
                "RequestId": "abc",
                "Action": "GetCodingPlanUsage",
                "Error": {"CodeN": 100024, "Code": "InvalidAuthorization", "Message": "bad sig"}
            }
        });
        match build_result(body).status {
            ProviderStatus::Unavailable { info } => assert_eq!(info.reason, "bad sig"),
            other => panic!("expected unavailable, got {:?}", other),
        }
    }

    #[test]
    fn unsubscribed_when_status_not_running() {
        let body = serde_json::json!({
            "ResponseMetadata": {"RequestId": "abc"},
            "Result": {"Status": "None", "QuotaUsage": []}
        });
        match build_result(body).status {
            ProviderStatus::Unavailable { .. } => {}
            other => panic!("expected unavailable, got {:?}", other),
        }
    }
}
