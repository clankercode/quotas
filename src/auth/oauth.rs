use crate::{Error, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::{AuthCredential, AuthResolver, ResolvedAuth};

fn parse_codex_auth(content: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Tokens {
        #[serde(rename = "access_token")]
        access_token: Option<String>,
    }
    #[derive(Deserialize)]
    struct CodexAuth {
        #[serde(rename = "OPENAI_API_KEY")]
        openai_api_key: Option<String>,
        tokens: Option<Tokens>,
    }
    let parsed: CodexAuth = serde_json::from_str(content).ok()?;
    parsed
        .tokens
        .and_then(|t| t.access_token)
        .or(parsed.openai_api_key)
}

fn parse_claude_credentials(content: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Oauth {
        #[serde(rename = "accessToken")]
        access_token: Option<String>,
    }
    #[derive(Deserialize)]
    struct Credentials {
        #[serde(rename = "claudeAiOauth")]
        claude_ai_oauth: Option<Oauth>,
    }
    let parsed: Credentials = serde_json::from_str(content).ok()?;
    parsed.claude_ai_oauth.and_then(|o| o.access_token)
}

fn parse_gemini_credentials(content: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct GeminiCreds {
        #[serde(rename = "access_token")]
        access_token: Option<String>,
    }
    let parsed: GeminiCreds = serde_json::from_str(content).ok()?;
    parsed.access_token
}

/// Parse Grok Build's `~/.grok/auth.json`.
///
/// Format is a map of account keys → credential objects. Each entry has a
/// `key` (session/OIDC access token). Prefer the newest non-expired entry;
/// if all are expired, still return the newest key so the caller can surface
/// a clear re-auth error from the API.
pub(crate) fn parse_grok_auth(content: &str) -> Option<String> {
    let root: serde_json::Value = serde_json::from_str(content).ok()?;
    let map = root.as_object()?;
    if map.is_empty() {
        return None;
    }

    #[derive(Clone)]
    struct Candidate {
        key: String,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        create_time: Option<chrono::DateTime<chrono::Utc>>,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for (_account, entry) in map {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let key = obj
            .get("key")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let Some(key) = key else {
            continue;
        };
        let expires_at = obj
            .get("expires_at")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        let create_time = obj
            .get("create_time")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        candidates.push(Candidate {
            key,
            expires_at,
            create_time,
        });
    }
    if candidates.is_empty() {
        return None;
    }

    let now = chrono::Utc::now();
    // Prefer unexpired tokens; among them pick latest create_time / expires_at.
    candidates.sort_by(|a, b| {
        let a_fresh = a.expires_at.map(|e| e > now).unwrap_or(true);
        let b_fresh = b.expires_at.map(|e| e > now).unwrap_or(true);
        b_fresh
            .cmp(&a_fresh)
            .then_with(|| b.create_time.cmp(&a.create_time))
            .then_with(|| b.expires_at.cmp(&a.expires_at))
    });
    candidates.into_iter().next().map(|c| c.key)
}

pub struct OAuthFileResolver {
    pub file_paths: Vec<PathBuf>,
    pub parse_fn: fn(&str) -> Option<String>,
    pub source_name: String,
}

impl OAuthFileResolver {
    pub fn codex() -> Self {
        Self {
            file_paths: vec![dirs::home_dir()
                .unwrap_or_default()
                .join(".codex/auth.json")],
            parse_fn: parse_codex_auth,
            source_name: "codex".to_string(),
        }
    }

    pub fn claude() -> Self {
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
            paths.push(PathBuf::from(dir).join(".credentials.json"));
        }
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".claude/.credentials.json"));
        }
        Self {
            file_paths: paths,
            parse_fn: parse_claude_credentials,
            source_name: "claude".to_string(),
        }
    }

    pub fn gemini() -> Self {
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".gemini/oauth_creds.json"));
        }
        if let Ok(gemini_home) = std::env::var("GEMINI_CLI_HOME") {
            if !gemini_home.is_empty() {
                paths.insert(0, PathBuf::from(gemini_home).join("oauth_creds.json"));
            }
        }
        Self {
            file_paths: paths,
            parse_fn: parse_gemini_credentials,
            source_name: "gemini".to_string(),
        }
    }

    /// Grok Build CLI session token from `~/.grok/auth.json` (or `$GROK_HOME/auth.json`).
    pub fn grok() -> Self {
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Ok(home) = std::env::var("GROK_HOME") {
            if !home.is_empty() {
                paths.push(PathBuf::from(home).join("auth.json"));
            }
        }
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".grok/auth.json"));
        }
        // XDG config fallback used by some install layouts.
        if let Some(config) = dirs::config_dir() {
            paths.push(config.join("grok/auth.json"));
        }
        Self {
            file_paths: paths,
            parse_fn: parse_grok_auth,
            source_name: "grok".to_string(),
        }
    }
}

#[async_trait]
impl AuthResolver for OAuthFileResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        for path in &self.file_paths {
            if !path.exists() {
                continue;
            }
            let content = match tokio::fs::read_to_string(path).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Some(key) = (self.parse_fn)(&content) {
                return Ok(ResolvedAuth {
                    credential: AuthCredential::Token(key),
                    source: format!("oauth:{}", path.display()),
                });
            }
        }
        Err(Error::Auth(format!(
            "no OAuth credentials found for {}",
            self.source_name
        )))
    }

    fn have_credentials(&self) -> bool {
        self.file_paths.iter().any(|p| p.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_credentials() {
        let json = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-abc","refreshToken":"r","expiresAt":1}}"#;
        assert_eq!(
            parse_claude_credentials(json).as_deref(),
            Some("sk-ant-oat01-abc")
        );
    }

    #[test]
    fn parses_codex_auth_token_field() {
        let json = r#"{"tokens":{"id_token":"x","access_token":"oauth-abc"}}"#;
        assert_eq!(parse_codex_auth(json).as_deref(), Some("oauth-abc"));
    }

    #[test]
    fn parses_codex_auth_api_key_fallback() {
        let json = r#"{"OPENAI_API_KEY":"sk-proj-xxx"}"#;
        assert_eq!(parse_codex_auth(json).as_deref(), Some("sk-proj-xxx"));
    }

    #[test]
    fn parses_gemini_credentials() {
        let json = r#"{"access_token":"ya29.test-token","token_type":"Bearer","refresh_token":"1//0g-xxx","expiry_date":1776279276916}"#;
        assert_eq!(
            parse_gemini_credentials(json).as_deref(),
            Some("ya29.test-token")
        );
    }

    #[test]
    fn parses_gemini_credentials_missing_token() {
        let json = r#"{"token_type":"Bearer","refresh_token":"1//0g-xxx"}"#;
        assert_eq!(parse_gemini_credentials(json).as_deref(), None);
    }

    #[test]
    fn parses_gemini_credentials_empty() {
        assert_eq!(parse_gemini_credentials("").as_deref(), None);
    }

    #[test]
    fn parses_grok_auth_json_prefers_unexpired() {
        let json = r#"{
            "https://auth.x.ai::old": {
                "key": "expired-token",
                "expires_at": "2020-01-01T00:00:00Z",
                "create_time": "2020-01-01T00:00:00Z"
            },
            "https://auth.x.ai::new": {
                "key": "fresh-token",
                "expires_at": "2099-01-01T00:00:00Z",
                "create_time": "2026-07-11T00:00:00Z"
            }
        }"#;
        assert_eq!(parse_grok_auth(json).as_deref(), Some("fresh-token"));
    }

    #[test]
    fn parses_grok_auth_json_empty_map() {
        assert_eq!(parse_grok_auth("{}").as_deref(), None);
    }

    #[test]
    fn parses_grok_auth_legacy_accounts_key() {
        // Single-entry map still works even without expires_at.
        let json = r#"{
            "https://accounts.x.ai/sign-in": {
                "key": "session-abc"
            }
        }"#;
        assert_eq!(parse_grok_auth(json).as_deref(), Some("session-abc"));
    }

    #[test]
    fn oauth_have_credentials_true_when_file_exists() {
        let path = std::env::temp_dir().join("quotas_test_oauth_have.json");
        std::fs::write(&path, r#"{"access_token":"ya29.test"}"#).unwrap();
        let resolver = OAuthFileResolver {
            file_paths: vec![path.clone()],
            parse_fn: parse_gemini_credentials,
            source_name: "test".into(),
        };
        assert!(resolver.have_credentials());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn oauth_have_credentials_false_when_no_files() {
        let path = std::env::temp_dir().join("quotas_test_oauth_no_such.json");
        let _ = std::fs::remove_file(&path);
        let resolver = OAuthFileResolver {
            file_paths: vec![path],
            parse_fn: parse_gemini_credentials,
            source_name: "test".into(),
        };
        assert!(!resolver.have_credentials());
    }
}
