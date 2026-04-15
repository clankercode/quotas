use crate::{Error, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::{AuthCredential, AuthResolver, ResolvedAuth};

pub struct CursorAuthResolver {
    auth_json_path: PathBuf,
    cli_config_path: PathBuf,
}

impl CursorAuthResolver {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Self {
            auth_json_path: home.join(".config/cursor/auth.json"),
            cli_config_path: home.join(".config/cursor/cli-config.json"),
        }
    }
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct AuthJson {
    accessToken: String,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct CliConfig {
    authInfo: CliAuthInfo,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct CliAuthInfo {
    userId: u64,
}

#[async_trait]
impl AuthResolver for CursorAuthResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        // Read access token from auth.json
        let auth_content = tokio::fs::read_to_string(&self.auth_json_path).await.map_err(|_| {
            Error::Auth(format!(
                "cursor auth file not found: {}",
                self.auth_json_path.display()
            ))
        })?;
        let auth: AuthJson = serde_json::from_str(&auth_content)
            .map_err(|_| Error::Auth("invalid cursor auth.json".into()))?;

        // Read user ID from cli-config.json
        let config_content = tokio::fs::read_to_string(&self.cli_config_path).await.map_err(|_| {
            Error::Auth(format!(
                "cursor cli-config file not found: {}",
                self.cli_config_path.display()
            ))
        })?;
        let config: CliConfig = serde_json::from_str(&config_content)
            .map_err(|_| Error::Auth("invalid cursor cli-config.json".into()))?;

        // Construct WorkosCursorSessionToken = "{userId}%3A%3A{jwt_token}"
        // %3A%3A is URL-encoded "::"
        let session_token = format!("{}%3A%3A{}", config.authInfo.userId, auth.accessToken);
        Ok(ResolvedAuth {
            credential: AuthCredential::Cookie(session_token),
            source: format!("file:{} + file:{}", self.auth_json_path.display(), self.cli_config_path.display()),
        })
    }
}