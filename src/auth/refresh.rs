use crate::{Error, Result};
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Kimi CLI OAuth client ID — extracted from kimi-cli source.
const KIMI_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
/// Refresh endpoint (override via env, matching kimi-cli behavior).
const KIMI_OAUTH_HOST_DEFAULT: &str = "https://auth.kimi.com";

/// Claude Code OAuth client ID (extracted from claude-code source).
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// Codex (OpenAI ChatGPT) OAuth client ID (extracted from codex-cli / opencode).
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

const REFRESH_THRESHOLD_SECS: i64 = 300;

#[derive(Debug, Clone)]
pub struct KimiCreds {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: f64,
    pub scope: Option<String>,
    pub token_type: Option<String>,
}

pub fn kimi_creds_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".kimi/credentials/kimi-code.json")
}

pub fn kimi_device_id_path() -> PathBuf {
    // kimi-cli stores device_id at ~/.kimi/device_id in practice.
    dirs::home_dir().unwrap_or_default().join(".kimi/device_id")
}

pub fn read_kimi_creds(path: &Path) -> Result<KimiCreds> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::Auth(format!("read kimi creds: {}", e)))?;
    let v: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| Error::Auth(format!("kimi creds parse: {}", e)))?;
    Ok(KimiCreds {
        access_token: v
            .get("access_token")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        refresh_token: v
            .get("refresh_token")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        expires_at: v.get("expires_at").and_then(|s| s.as_f64()).unwrap_or(0.0),
        scope: v.get("scope").and_then(|s| s.as_str()).map(String::from),
        token_type: v
            .get("token_type")
            .and_then(|s| s.as_str())
            .map(String::from),
    })
}

pub fn write_kimi_creds(path: &Path, creds: &KimiCreds) -> Result<()> {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "access_token".into(),
        serde_json::Value::String(creds.access_token.clone()),
    );
    obj.insert(
        "refresh_token".into(),
        serde_json::Value::String(creds.refresh_token.clone()),
    );
    obj.insert(
        "expires_at".into(),
        serde_json::Value::from(creds.expires_at),
    );
    if let Some(scope) = &creds.scope {
        obj.insert("scope".into(), serde_json::Value::String(scope.clone()));
    }
    if let Some(tt) = &creds.token_type {
        obj.insert("token_type".into(), serde_json::Value::String(tt.clone()));
    }
    let s = serde_json::to_string_pretty(&serde_json::Value::Object(obj))?;
    // Atomic-ish write: write to temp in same dir then rename.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, s)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn kimi_expired(creds: &KimiCreds) -> bool {
    // Match kimi-cli's 300s threshold.
    let now = Utc::now().timestamp() as f64;
    creds.expires_at - now < 300.0
}

pub fn read_device_id() -> String {
    std::fs::read_to_string(kimi_device_id_path())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub async fn refresh_kimi_token(creds: &KimiCreds) -> Result<KimiCreds> {
    #[derive(Deserialize)]
    struct RefreshResponse {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
        #[serde(default)]
        scope: Option<String>,
        #[serde(default)]
        token_type: Option<String>,
    }

    let host = std::env::var("KIMI_CODE_OAUTH_HOST")
        .or_else(|_| std::env::var("KIMI_OAUTH_HOST"))
        .unwrap_or_else(|_| KIMI_OAUTH_HOST_DEFAULT.to_string());
    let url = format!("{}/api/oauth/token", host.trim_end_matches('/'));

    let device_id = read_device_id();
    let hostname = gethostname::gethostname().to_string_lossy().into_owned();
    let uname = std::process::Command::new("uname")
        .arg("-sr")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| std::env::consts::OS.to_string());

    let client = Client::new();
    let form = [
        ("client_id", KIMI_CLIENT_ID.to_string()),
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", creds.refresh_token.clone()),
    ];
    let resp = client
        .post(&url)
        .header("X-Msh-Platform", "kimi_cli")
        .header("X-Msh-Version", "0.8.0")
        .header("X-Msh-Device-Name", hostname.as_str())
        .header("X-Msh-Device-Model", uname.as_str())
        .header("X-Msh-Os-Version", uname.as_str())
        .header("X-Msh-Device-Id", device_id.as_str())
        .form(&form)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!(
            "kimi refresh failed ({}): {}",
            status, text
        )));
    }
    let parsed: RefreshResponse = resp.json().await?;
    let expires_at = Utc::now().timestamp() as f64 + parsed.expires_in as f64;
    Ok(KimiCreds {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_at,
        scope: parsed.scope.or_else(|| creds.scope.clone()),
        token_type: parsed.token_type.or_else(|| creds.token_type.clone()),
    })
}

/// Refresh kimi creds on disk if they're expired. Returns Ok(true) if a refresh
/// happened, Ok(false) if no refresh was needed.
pub async fn refresh_kimi_if_expired(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let creds = read_kimi_creds(path)?;
    if creds.refresh_token.is_empty() {
        return Ok(false);
    }
    if !kimi_expired(&creds) {
        return Ok(false);
    }
    let fresh = refresh_kimi_token(&creds).await?;
    write_kimi_creds(path, &fresh)?;
    Ok(true)
}

// ---------- Claude Code refresh ----------

pub fn claude_creds_path() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join(".credentials.json");
        }
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".claude/.credentials.json")
}

/// Returns Ok(true) if a refresh happened.
pub async fn refresh_claude_if_expired(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::Auth(format!("read claude creds: {}", e)))?;
    let mut root: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| Error::Auth(format!("claude creds parse: {}", e)))?;

    let Some(oauth) = root.get("claudeAiOauth").cloned() else {
        return Ok(false);
    };
    let refresh_token = oauth
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_at_ms = oauth.get("expiresAt").and_then(|v| v.as_i64()).unwrap_or(0);
    if refresh_token.is_empty() {
        return Ok(false);
    }
    let now_ms = Utc::now().timestamp_millis();
    if expires_at_ms - now_ms > REFRESH_THRESHOLD_SECS * 1000 {
        return Ok(false);
    }

    #[derive(Deserialize)]
    struct Resp {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        expires_in: i64,
        #[serde(default)]
        scope: Option<String>,
    }

    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLAUDE_CLIENT_ID,
        "scope": CLAUDE_SCOPES,
    });
    let client = Client::new();
    let resp = client
        .post(CLAUDE_TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!(
            "claude refresh failed ({}): {}",
            status, text
        )));
    }
    let parsed: Resp = resp.json().await?;
    let new_expires_at_ms = Utc::now().timestamp_millis() + parsed.expires_in * 1000;
    let new_refresh = parsed.refresh_token.unwrap_or(refresh_token);

    if let Some(oauth_obj) = root
        .get_mut("claudeAiOauth")
        .and_then(|v| v.as_object_mut())
    {
        oauth_obj.insert(
            "accessToken".into(),
            serde_json::Value::String(parsed.access_token),
        );
        oauth_obj.insert(
            "refreshToken".into(),
            serde_json::Value::String(new_refresh),
        );
        oauth_obj.insert(
            "expiresAt".into(),
            serde_json::Value::from(new_expires_at_ms),
        );
        if let Some(scope) = parsed.scope {
            let scopes: Vec<serde_json::Value> = scope
                .split_whitespace()
                .map(|s| serde_json::Value::String(s.to_string()))
                .collect();
            oauth_obj.insert("scopes".into(), serde_json::Value::Array(scopes));
        }
    }

    let s = serde_json::to_string_pretty(&root)?;
    write_atomic(path, &s)?;
    Ok(true)
}

// ---------- Codex (ChatGPT) refresh ----------

pub fn codex_creds_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".codex/auth.json")
}

pub async fn refresh_codex_if_expired(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::Auth(format!("read codex creds: {}", e)))?;
    let mut root: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| Error::Auth(format!("codex creds parse: {}", e)))?;

    let tokens = root.get("tokens").cloned().unwrap_or_default();
    let refresh_token = tokens
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if refresh_token.is_empty() {
        return Ok(false);
    }

    // last_refresh is ISO8601; if not present or too old, refresh.
    let last_refresh_str = root
        .get("last_refresh")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let needs_refresh = if let Ok(last) = chrono::DateTime::parse_from_rfc3339(last_refresh_str) {
        (Utc::now() - last.with_timezone(&Utc)).num_seconds() > 3000
    } else {
        true
    };
    if !needs_refresh {
        return Ok(false);
    }

    #[derive(Deserialize)]
    struct Resp {
        access_token: String,
        refresh_token: String,
        id_token: Option<String>,
        #[allow(dead_code)]
        expires_in: Option<i64>,
    }

    let form = [
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.clone()),
        ("client_id", CODEX_CLIENT_ID.to_string()),
    ];
    let client = Client::new();
    let resp = client
        .post(CODEX_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&form)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!(
            "codex refresh failed ({}): {}",
            status, text
        )));
    }
    let parsed: Resp = resp.json().await?;

    if let Some(tokens_obj) = root.get_mut("tokens").and_then(|v| v.as_object_mut()) {
        tokens_obj.insert(
            "access_token".into(),
            serde_json::Value::String(parsed.access_token),
        );
        tokens_obj.insert(
            "refresh_token".into(),
            serde_json::Value::String(parsed.refresh_token),
        );
        if let Some(id) = parsed.id_token {
            tokens_obj.insert("id_token".into(), serde_json::Value::String(id));
        }
    }
    root.as_object_mut().map(|o| {
        o.insert(
            "last_refresh".into(),
            serde_json::Value::String(Utc::now().to_rfc3339()),
        )
    });

    let s = serde_json::to_string_pretty(&root)?;
    write_atomic(path, &s)?;
    Ok(true)
}

// ---------- opencode refresh ----------

pub fn opencode_creds_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("opencode/auth.json"));
        }
    }
    let home = dirs::home_dir()?;
    Some(home.join(".local/share/opencode/auth.json"))
}

/// Refresh the anthropic slot of opencode's auth.json (uses Claude's refresh endpoint).
pub async fn refresh_opencode_anthropic_if_expired(path: &Path) -> Result<bool> {
    refresh_opencode_slot_via_claude(path, "anthropic").await
}

/// Refresh the openai slot of opencode's auth.json (uses OpenAI's refresh endpoint).
pub async fn refresh_opencode_openai_if_expired(path: &Path) -> Result<bool> {
    refresh_opencode_slot_via_openai(path, "openai").await
}

async fn refresh_opencode_slot_via_claude(path: &Path, slot: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::Auth(format!("read opencode creds: {}", e)))?;
    let mut root: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| Error::Auth(format!("opencode creds parse: {}", e)))?;

    let slot_entry = match root.get(slot).cloned() {
        Some(v) => v,
        None => return Ok(false),
    };
    if slot_entry.get("type").and_then(|v| v.as_str()) != Some("oauth") {
        return Ok(false);
    }
    let refresh_token = slot_entry
        .get("refresh")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_ms = slot_entry
        .get("expires")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if refresh_token.is_empty() {
        return Ok(false);
    }
    let now_ms = Utc::now().timestamp_millis();
    if expires_ms - now_ms > REFRESH_THRESHOLD_SECS * 1000 {
        return Ok(false);
    }

    #[derive(Deserialize)]
    struct Resp {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        expires_in: i64,
    }

    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLAUDE_CLIENT_ID,
        "scope": CLAUDE_SCOPES,
    });
    let client = Client::new();
    let resp = client
        .post(CLAUDE_TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!(
            "opencode anthropic refresh failed ({}): {}",
            status, text
        )));
    }
    let parsed: Resp = resp.json().await?;
    let new_expires_ms = Utc::now().timestamp_millis() + parsed.expires_in * 1000;
    let new_refresh = parsed.refresh_token.unwrap_or(refresh_token);

    if let Some(obj) = root.get_mut(slot).and_then(|v| v.as_object_mut()) {
        obj.insert(
            "access".into(),
            serde_json::Value::String(parsed.access_token),
        );
        obj.insert("refresh".into(), serde_json::Value::String(new_refresh));
        obj.insert("expires".into(), serde_json::Value::from(new_expires_ms));
    }

    let s = serde_json::to_string_pretty(&root)?;
    write_atomic(path, &s)?;
    Ok(true)
}

async fn refresh_opencode_slot_via_openai(path: &Path, slot: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::Auth(format!("read opencode creds: {}", e)))?;
    let mut root: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| Error::Auth(format!("opencode creds parse: {}", e)))?;

    let slot_entry = match root.get(slot).cloned() {
        Some(v) => v,
        None => return Ok(false),
    };
    if slot_entry.get("type").and_then(|v| v.as_str()) != Some("oauth") {
        return Ok(false);
    }
    let refresh_token = slot_entry
        .get("refresh")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_ms = slot_entry
        .get("expires")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if refresh_token.is_empty() {
        return Ok(false);
    }
    let now_ms = Utc::now().timestamp_millis();
    if expires_ms - now_ms > REFRESH_THRESHOLD_SECS * 1000 {
        return Ok(false);
    }

    #[derive(Deserialize)]
    struct Resp {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
    }

    let form = [
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token),
        ("client_id", CODEX_CLIENT_ID.to_string()),
    ];
    let client = Client::new();
    let resp = client
        .post(CODEX_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&form)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(Error::Auth(format!(
            "opencode openai refresh failed ({}): {}",
            status, text
        )));
    }
    let parsed: Resp = resp.json().await?;
    let new_expires_ms = Utc::now().timestamp_millis() + parsed.expires_in * 1000;

    if let Some(obj) = root.get_mut(slot).and_then(|v| v.as_object_mut()) {
        obj.insert(
            "access".into(),
            serde_json::Value::String(parsed.access_token),
        );
        obj.insert(
            "refresh".into(),
            serde_json::Value::String(parsed.refresh_token),
        );
        obj.insert("expires".into(), serde_json::Value::from(new_expires_ms));
    }

    let s = serde_json::to_string_pretty(&root)?;
    write_atomic(path, &s)?;
    Ok(true)
}

fn write_atomic(path: &Path, s: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, s)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_expired_true_when_past() {
        let creds = KimiCreds {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 1.0,
            scope: None,
            token_type: None,
        };
        assert!(kimi_expired(&creds));
    }

    #[test]
    fn kimi_expired_false_when_far_future() {
        let creds = KimiCreds {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: (Utc::now().timestamp() + 3600) as f64,
            scope: None,
            token_type: None,
        };
        assert!(!kimi_expired(&creds));
    }

    #[test]
    fn write_then_read_roundtrip() {
        let tmp = std::env::temp_dir().join("quotas-kimi-refresh-test.json");
        let creds = KimiCreds {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: 1234.5,
            scope: Some("openid".into()),
            token_type: Some("Bearer".into()),
        };
        write_kimi_creds(&tmp, &creds).unwrap();
        let back = read_kimi_creds(&tmp).unwrap();
        assert_eq!(back.access_token, "at");
        assert_eq!(back.refresh_token, "rt");
        assert_eq!(back.expires_at, 1234.5);
        assert_eq!(back.scope.as_deref(), Some("openid"));
        let _ = std::fs::remove_file(&tmp);
    }
}
