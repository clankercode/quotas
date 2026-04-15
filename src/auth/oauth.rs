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
