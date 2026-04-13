use crate::{Error, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::{AuthCredential, AuthResolver, ResolvedAuth};

fn parse_codex_auth(content: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct CodexAuth {
        access_token: String,
    }
    serde_json::from_str::<CodexAuth>(content)
        .ok()
        .map(|a| a.access_token)
}

fn parse_copilot_token(content: &str) -> Option<String> {
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct CopilotToken {
        token: Option<String>,
        expires_at: Option<String>,
    }
    #[derive(Deserialize)]
    struct CopilotTokens {
        tokens: Vec<CopilotToken>,
    }
    serde_json::from_str::<CopilotTokens>(content)
        .ok()
        .and_then(|t| t.tokens.into_iter().next()?.token)
}

pub struct OAuthFileResolver {
    pub file_path: PathBuf,
    pub parse_fn: fn(&str) -> Option<String>,
    pub source_name: String,
    pub prefix: String,
}

impl OAuthFileResolver {
    pub fn codex() -> Self {
        Self {
            file_path: dirs::home_dir()
                .unwrap_or_default()
                .join(".codex/auth.json"),
            parse_fn: parse_codex_auth,
            source_name: "codex".to_string(),
            prefix: "token".to_string(),
        }
    }

    pub fn copilot() -> Self {
        Self {
            file_path: dirs::home_dir()
                .unwrap_or_default()
                .join(".config/github-copilot/tokens.json"),
            parse_fn: parse_copilot_token,
            source_name: "copilot".to_string(),
            prefix: "token".to_string(),
        }
    }
}

#[async_trait]
impl AuthResolver for OAuthFileResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        if !self.file_path.exists() {
            return Err(Error::Auth(format!(
                "OAuth file not found: {}",
                self.file_path.display()
            )));
        }
        let content = tokio::fs::read_to_string(&self.file_path).await?;
        let key = (self.parse_fn)(&content);
        key.map(|k| ResolvedAuth {
            credential: AuthCredential::Token(k),
            source: format!("oauth:{}", self.file_path.display()),
        })
        .ok_or_else(|| Error::Auth("failed to parse OAuth file".into()))
    }
}
