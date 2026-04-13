pub mod env;
pub mod file;
pub mod oauth;
pub mod opencode;
pub mod refresh;

use crate::{Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthCredential {
    Bearer(String),
    Token(String),
    Cookie(String),
}

impl AuthCredential {
    /// Extract the bearer/token string. Returns an error for cookie
    /// credentials (which need to be sent as a Cookie header, not
    /// Authorization).
    pub fn unwrap_token(&self) -> Result<&str> {
        match self {
            AuthCredential::Bearer(s) | AuthCredential::Token(s) => Ok(s.as_str()),
            AuthCredential::Cookie(_) => Err(Error::Auth(
                "cookie credential cannot be used as bearer token".into(),
            )),
        }
    }
}

pub struct ResolvedAuth {
    pub credential: AuthCredential,
    pub source: String,
}

#[async_trait]
pub trait AuthResolver: Send + Sync {
    async fn resolve(&self) -> Result<ResolvedAuth>;
}

pub struct MultiResolver {
    resolvers: Vec<Box<dyn AuthResolver>>,
}

impl MultiResolver {
    pub fn new(resolvers: Vec<Box<dyn AuthResolver>>) -> Self {
        Self { resolvers }
    }
}

#[async_trait]
impl AuthResolver for MultiResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        for resolver in &self.resolvers {
            if let Ok(auth) = resolver.resolve().await {
                return Ok(auth);
            }
        }
        Err(Error::Auth("no valid credentials found".into()))
    }
}
