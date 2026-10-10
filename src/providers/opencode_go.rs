//! OpenCode Go subscription usage.
//!
//! OpenCode Go (Go / Go Plus) gives each model a monthly dollar allowance; a
//! 5-hour rolling window is 20% of it and the weekly window is 50%. The usage
//! endpoint reports only used-percent per window, never dollars:
//!
//! ```json
//! {"usage":{"rolling":{"status":"ok","percent":0,"resetsAt":"…"},
//!           "weekly":{…},"monthly":{…}}}
//! ```
//!
//! `status` is `ok` or `rate-limited`. The endpoint is undocumented (see
//! `docs-usage/opencode-go.md`) and sits behind Cloudflare, which rejects
//! generic user agents with error 1010, so every request sends an explicit
//! `User-Agent`.

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
const CONSOLE_URL: &str = "https://opencode.ai/console/";
const USER_AGENT: &str = concat!("quotas-cli/", env!("CARGO_PKG_VERSION"));

/// Windows in display order: (JSON key, window_type, period length).
const WINDOWS: [(&str, &str, i64); 3] = [
    ("rolling", "5h", 5 * 3600),
    ("weekly", "weekly", 7 * 86400),
    ("monthly", "monthly", 30 * 86400),
];

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
            .header("Authorization", format!("Bearer {key}"))
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;

        let status = resp.status().as_u16();
        let text = resp.text().await?;
        let body: Option<serde_json::Value> = serde_json::from_str(&text).ok();

        Ok(result(classify(status, &text, body.as_ref()), body))
    }
}

/// Map an HTTP status + raw body text (+ its JSON form, when it parsed) to
/// a provider status. Pure so it can be tested without a network.
pub(crate) fn classify(
    status: u16,
    text: &str,
    body: Option<&serde_json::Value>,
) -> ProviderStatus {
    let unavailable = |reason: String, console: bool| ProviderStatus::Unavailable {
        info: UnavailableInfo {
            reason,
            console_url: console.then(|| CONSOLE_URL.into()),
        },
    };

    if (200..300).contains(&status) {
        return match body.map(parse_response) {
            Some(Ok(quota)) => ProviderStatus::Available { quota },
            Some(Err(e)) => ProviderStatus::NetworkError {
                message: e.to_string(),
            },
            None => ProviderStatus::NetworkError {
                message: "response was not JSON".into(),
            },
        };
    }

    // Cloudflare browser-signature block (error 1010). Cloudflare serves it as
    // JSON or as an HTML challenge page depending on the request, so sniff
    // the raw text as well as the parsed body. Not a credential problem.
    let cf_blocked = body.is_some_and(|b| {
        b.get("error_code").and_then(|v| v.as_i64()) == Some(1010)
            || b.get("error_name").and_then(|v| v.as_str()) == Some("browser_signature_banned")
    }) || (status == 403
        && (text.contains("error-1010")
            || text.contains("Error 1010")
            || text.contains("browser_signature_banned")));
    if cf_blocked {
        return unavailable(
            "Blocked by Cloudflare (user-agent); retry later".into(),
            false,
        );
    }

    // API error envelope: {"type":"error","error":{"type":"AuthError","message":"…"}}
    let api_message = body
        .and_then(|b| b.pointer("/error/message"))
        .and_then(|v| v.as_str())
        .filter(|m| !m.is_empty());

    // 401/403 mean the key was rejected: show the API's message and link the
    // console where a new key is minted. Anything else is a server-side
    // problem; keep the status visible and append the message if there is one.
    match (status, api_message) {
        (401 | 403, Some(msg)) => unavailable(msg.to_string(), true),
        (401 | 403, None) => unavailable(format!("HTTP {status}"), true),
        (_, Some(msg)) => unavailable(format!("HTTP {status}: {msg}"), false),
        (_, None) => unavailable(format!("HTTP {status}"), false),
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
struct WindowEntry {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    percent: Option<f64>,
    #[serde(rename = "resetsAt", default)]
    resets_at: Option<String>,
}

/// Parse the `/zen/go/v1/usage` body into quota windows.
///
/// Each window is `used/100`: the API gives used-percent only. A
/// `rate-limited` window is reported as fully used regardless of `percent`.
/// A window that is absent is skipped; a window that is present but has no
/// numeric `percent` (and is not rate-limited) is a schema change and is
/// rejected rather than rendered as 0% used.
pub(crate) fn parse_response(body: &serde_json::Value) -> Result<ProviderQuota> {
    let usage = body
        .get("usage")
        .and_then(|u| u.as_object())
        .ok_or_else(|| crate::Error::Provider("missing usage object".into()))?;

    let mut windows = Vec::with_capacity(WINDOWS.len());
    for (key, window_type, period) in WINDOWS {
        let Some(raw) = usage.get(key) else {
            continue;
        };
        if !raw.is_object() {
            return Err(crate::Error::Provider(format!(
                "usage.{key}: expected an object"
            )));
        }
        let entry: WindowEntry = serde_json::from_value(raw.clone())
            .map_err(|e| crate::Error::Provider(format!("usage.{key}: {e}")))?;

        let rate_limited = entry.status.as_deref() == Some("rate-limited");
        let used = match (rate_limited, entry.percent) {
            (true, _) => 100,
            (false, Some(pct)) if pct.is_finite() => pct.clamp(0.0, 100.0).round() as i64,
            (false, _) => {
                return Err(crate::Error::Provider(format!(
                    "usage.{key}: missing numeric percent"
                )))
            }
        };

        windows.push(QuotaWindow {
            window_type: window_type.to_string(),
            used,
            limit: 100,
            remaining: 100 - used,
            reset_at: entry.resets_at.as_deref().and_then(parse_rfc3339),
            period_seconds: Some(period),
        });
    }

    if windows.is_empty() {
        return Err(crate::Error::Provider("usage object has no windows".into()));
    }

    Ok(ProviderQuota {
        plan_name: "OpenCode Go".into(),
        windows,
        unlimited: false,
        banked_resets: None,
    })
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
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
    use std::path::PathBuf;

    fn fixture(name: &str) -> serde_json::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/opencode_go")
            .join(name);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {}", path.display(), e));
        serde_json::from_str(&raw).expect("parse fixture json")
    }

    fn unavailable_reason(status: &ProviderStatus) -> &str {
        match status {
            ProviderStatus::Unavailable { info } => info.reason.as_str(),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    fn console_link(status: &ProviderStatus) -> Option<&str> {
        match status {
            ProviderStatus::Unavailable { info } => info.console_url.as_deref(),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    /// Classify a JSON body the way `fetch_usage` would: raw text + parsed form.
    fn classify_json(status: u16, body: &serde_json::Value) -> ProviderStatus {
        classify(status, &body.to_string(), Some(body))
    }

    #[test]
    fn parses_live_usage_fixture() {
        let quota = parse_response(&fixture("usage_live.json")).unwrap();
        assert_eq!(quota.plan_name, "OpenCode Go");
        assert!(!quota.unlimited);
        assert!(quota.banked_resets.is_none());

        let types: Vec<&str> = quota
            .windows
            .iter()
            .map(|w| w.window_type.as_str())
            .collect();
        assert_eq!(types, ["5h", "weekly", "monthly"]);

        let [rolling, weekly, monthly] = quota.windows.as_slice() else {
            panic!("expected 3 windows");
        };
        assert_eq!(
            (rolling.used, rolling.limit, rolling.remaining),
            (0, 100, 100)
        );
        assert_eq!((weekly.used, weekly.limit, weekly.remaining), (62, 100, 38));
        assert_eq!(
            (monthly.used, monthly.limit, monthly.remaining),
            (31, 100, 69)
        );

        assert_eq!(rolling.period_seconds, Some(5 * 3600));
        assert_eq!(weekly.period_seconds, Some(7 * 86400));
        assert_eq!(monthly.period_seconds, Some(30 * 86400));

        let expected = DateTime::parse_from_rfc3339("2026-10-12T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(weekly.reset_at, Some(expected));
        assert!(rolling.reset_at.is_some());
        assert!(monthly.reset_at.is_some());
    }

    #[test]
    fn rate_limited_window_is_fully_used() {
        let quota = parse_response(&fixture("usage_rate_limited.json")).unwrap();
        let rolling = &quota.windows[0];
        assert_eq!(rolling.window_type, "5h");
        assert_eq!(
            (rolling.used, rolling.limit, rolling.remaining),
            (100, 100, 0)
        );
        // Other windows unaffected.
        assert_eq!(quota.windows[1].used, 62);
    }

    #[test]
    fn rate_limited_overrides_stale_percent() {
        let body = serde_json::json!({"usage": {
            "rolling": {"status": "rate-limited", "percent": 97, "resetsAt": "2026-10-11T03:57:22.826Z"}
        }});
        let quota = parse_response(&body).unwrap();
        assert_eq!(quota.windows[0].used, 100);
        assert_eq!(quota.windows[0].remaining, 0);
    }

    #[test]
    fn missing_window_is_skipped() {
        let body = serde_json::json!({"usage": {
            "rolling": {"status": "ok", "percent": 5, "resetsAt": "2026-10-11T03:57:22.826Z"},
            "weekly": {"status": "ok", "percent": 50, "resetsAt": "2026-10-12T00:00:00.000Z"}
        }});
        let quota = parse_response(&body).unwrap();
        let types: Vec<&str> = quota
            .windows
            .iter()
            .map(|w| w.window_type.as_str())
            .collect();
        assert_eq!(types, ["5h", "weekly"]);
    }

    #[test]
    fn float_percent_rounds_and_clamps() {
        let body = serde_json::json!({"usage": {
            "rolling": {"status": "ok", "percent": 61.7},
            "weekly": {"status": "ok", "percent": 140},
            "monthly": {"status": "ok", "percent": -3}
        }});
        let quota = parse_response(&body).unwrap();
        assert_eq!(quota.windows[0].used, 62);
        assert_eq!(quota.windows[0].reset_at, None);
        assert_eq!(quota.windows[1].used, 100);
        assert_eq!(quota.windows[2].used, 0);
    }

    #[test]
    fn missing_usage_object_is_an_error() {
        assert!(parse_response(&serde_json::json!({})).is_err());
        assert!(parse_response(&serde_json::json!({"usage": {}})).is_err());
        assert!(parse_response(&serde_json::json!({"usage": []})).is_err());
        assert!(parse_response(&serde_json::json!([1, 2, 3])).is_err());
        assert!(parse_response(&serde_json::json!("usage")).is_err());
    }

    #[test]
    fn window_without_percent_is_rejected_not_zero() {
        // A present window with no numeric percent is a schema change; it
        // must not render as a healthy 0/100 bar.
        for rolling in [
            serde_json::json!({"status": "ok"}),
            serde_json::json!({"status": "ok", "percent": null}),
            serde_json::json!({"status": "ok", "percent": "62"}),
            serde_json::json!({}),
        ] {
            let body = serde_json::json!({"usage": {"rolling": rolling}});
            assert!(parse_response(&body).is_err(), "accepted {body}");
        }
    }

    #[test]
    fn non_object_window_is_rejected() {
        for rolling in [
            serde_json::json!(["ok", 42, null]),
            serde_json::json!(42),
            serde_json::json!("ok"),
            serde_json::json!(null),
        ] {
            let body = serde_json::json!({"usage": {"rolling": rolling}});
            assert!(parse_response(&body).is_err(), "accepted {body}");
        }
    }

    #[test]
    fn rate_limited_window_needs_no_percent() {
        let body = serde_json::json!({"usage": {
            "rolling": {"status": "rate-limited", "resetsAt": "2026-10-11T03:57:22.826Z"}
        }});
        let quota = parse_response(&body).unwrap();
        assert_eq!(
            (quota.windows[0].used, quota.windows[0].remaining),
            (100, 0)
        );
    }

    #[test]
    fn malformed_reset_is_none_and_extra_keys_are_ignored() {
        let body = serde_json::json!({"usage": {
            "rolling": {"status": "ok", "percent": 5, "resetsAt": "tomorrow-ish", "extra": true},
            "weekly": {"status": "ok", "percent": 50},
            "yearly": {"status": "ok", "percent": 1}
        }});
        let quota = parse_response(&body).unwrap();
        assert_eq!(quota.windows.len(), 2, "unknown window keys are ignored");
        assert_eq!(quota.windows[0].reset_at, None, "unparseable string → None");
        assert_eq!(quota.windows[1].reset_at, None, "absent → None");
        assert_eq!(quota.windows[1].used, 50);
    }

    #[test]
    fn non_string_reset_is_a_schema_error() {
        // Same rule as percent: a type change in the payload is rejected
        // rather than silently degraded.
        let body = serde_json::json!({"usage": {
            "weekly": {"status": "ok", "percent": 50, "resetsAt": 1760227200}
        }});
        assert!(parse_response(&body).is_err());
    }

    #[test]
    fn classify_success_is_available() {
        let body = fixture("usage_live.json");
        match classify_json(200, &body) {
            ProviderStatus::Available { quota } => assert_eq!(quota.windows.len(), 3),
            other => panic!("expected Available, got {other:?}"),
        }
    }

    #[test]
    fn classify_unauthorized_maps_api_message_with_console_link() {
        let body = fixture("error_unauthorized.json");
        let status = classify_json(401, &body);
        assert_eq!(unavailable_reason(&status), "Unauthorized");
        assert_eq!(console_link(&status), Some(CONSOLE_URL));
    }

    #[test]
    fn classify_missing_key_message() {
        let body = serde_json::json!({
            "type": "error",
            "error": {"type": "AuthError", "message": "Missing API key."}
        });
        assert_eq!(
            unavailable_reason(&classify_json(401, &body)),
            "Missing API key."
        );
    }

    #[test]
    fn classify_forbidden_api_message_links_console() {
        let body = serde_json::json!({
            "type": "error",
            "error": {"type": "AuthError", "message": "No active Go subscription"}
        });
        let status = classify_json(403, &body);
        assert_eq!(unavailable_reason(&status), "No active Go subscription");
        assert_eq!(console_link(&status), Some(CONSOLE_URL));

        // Credential rejection with no envelope still links the console.
        let status = classify(403, "", None);
        assert_eq!(unavailable_reason(&status), "HTTP 403");
        assert_eq!(console_link(&status), Some(CONSOLE_URL));
    }

    #[test]
    fn classify_cloudflare_json_block_is_not_an_auth_failure() {
        let body = serde_json::json!({
            "type": "https://developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-1xxx-errors/error-1010/",
            "title": "Error 1010: Access denied",
            "status": 403,
            "error_code": 1010,
            "error_name": "browser_signature_banned"
        });
        let status = classify_json(403, &body);
        assert!(
            unavailable_reason(&status).contains("Cloudflare"),
            "reason was {:?}",
            unavailable_reason(&status)
        );
        assert_eq!(console_link(&status), None, "not a key problem");
    }

    #[test]
    fn classify_cloudflare_html_block_is_detected_from_raw_text() {
        let html = "<!DOCTYPE html><html><head><title>Access denied | opencode.ai used Cloudflare to restrict access</title></head><body><h1>Error 1010</h1><p>The owner of this website has banned your access based on your browser's signature (a48937cf488f0d2e).</p></body></html>";
        let status = classify(403, html, None);
        assert!(
            unavailable_reason(&status).contains("Cloudflare"),
            "reason was {:?}",
            unavailable_reason(&status)
        );
        assert_eq!(console_link(&status), None);

        // Plain text / other HTML 403s are not misreported as Cloudflare.
        let status = classify(403, "<html><body>Forbidden</body></html>", None);
        assert_eq!(unavailable_reason(&status), "HTTP 403");
    }

    #[test]
    fn classify_server_errors_keep_http_status_visible() {
        // No body at all.
        let status = classify(502, "", None);
        assert_eq!(unavailable_reason(&status), "HTTP 502");
        assert_eq!(console_link(&status), None);

        // JSON body without an API error envelope.
        let status = classify_json(503, &serde_json::json!({"title": "Service Unavailable"}));
        assert_eq!(unavailable_reason(&status), "HTTP 503");

        // API error envelope on a non-auth status: status stays, message appended,
        // and no console link because a new key would not help.
        let envelope = serde_json::json!({
            "type": "error",
            "error": {"type": "RateLimitError", "message": "Too many requests"}
        });
        let status = classify_json(429, &envelope);
        assert_eq!(unavailable_reason(&status), "HTTP 429: Too many requests");
        assert_eq!(console_link(&status), None);

        let status = classify_json(500, &envelope);
        assert!(unavailable_reason(&status).starts_with("HTTP 500"));
    }

    #[test]
    fn classify_success_with_bad_body_is_network_error() {
        assert!(matches!(
            classify(200, "<html>maintenance</html>", None),
            ProviderStatus::NetworkError { .. }
        ));
        assert!(matches!(
            classify_json(200, &serde_json::json!({"usage": {}})),
            ProviderStatus::NetworkError { .. }
        ));
        assert!(matches!(
            classify_json(
                200,
                &serde_json::json!({"usage": {"rolling": {"status": "ok"}}})
            ),
            ProviderStatus::NetworkError { .. }
        ));
    }

    /// The provider surfaces auth-resolution failures as `Err(Error::Auth)` so
    /// `main.rs` renders `AuthRequired` (card hidden, listed under "No key").
    #[tokio::test]
    async fn fetch_without_credentials_bubbles_auth_error() {
        use crate::auth::{AuthResolver, ResolvedAuth};
        use crate::providers::Provider;

        struct NoCreds;
        #[async_trait]
        impl AuthResolver for NoCreds {
            async fn resolve(&self) -> crate::Result<ResolvedAuth> {
                Err(crate::Error::Auth("no opencode-go credentials".into()))
            }
            fn have_credentials(&self) -> bool {
                false
            }
        }

        let provider = OpencodeGoProvider::new(Box::new(NoCreds));
        assert_eq!(provider.kind(), ProviderKind::OpencodeGo);
        assert!(!provider.auth_resolver().have_credentials());
        match provider.fetch().await {
            Err(crate::Error::Auth(_)) => {}
            other => panic!("expected Error::Auth, got {other:?}"),
        }
    }

    #[test]
    fn user_agent_is_explicit_and_versioned() {
        // Cloudflare rejects generic UAs; the header must be present and must
        // identify this tool with its version, not a browser or library string.
        assert!(USER_AGENT.starts_with("quotas-cli/"));
        assert!(USER_AGENT.len() > "quotas-cli/".len());
        assert!(!USER_AGENT.contains("reqwest"));
    }
}
