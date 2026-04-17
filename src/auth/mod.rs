pub mod env;
pub mod file;
pub mod cursor;
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

    /// Extract the cookie string. Only valid for Cookie credentials.
    pub fn unwrap_cookie(&self) -> Result<&str> {
        match self {
            AuthCredential::Cookie(s) => Ok(s),
            _ => Err(Error::Auth(
                "non-cookie credential cannot be used as cookie".into(),
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

    /// Synchronous lightweight check whether credentials *might* exist.
    /// Checks env vars, file existence, etc. without doing network I/O.
    /// Default: true (assume credentials exist unless overridden).
    fn have_credentials(&self) -> bool {
        true
    }
}

pub struct StaticResolver {
    pub token: String,
    pub source: String,
}

#[async_trait]
impl AuthResolver for StaticResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        Ok(ResolvedAuth {
            credential: AuthCredential::Bearer(self.token.clone()),
            source: self.source.clone(),
        })
    }

    fn have_credentials(&self) -> bool {
        !self.token.is_empty()
    }
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

    fn have_credentials(&self) -> bool {
        self.resolvers.iter().any(|r| r.have_credentials())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AlwaysHaveCreds;
    #[async_trait]
    impl AuthResolver for AlwaysHaveCreds {
        async fn resolve(&self) -> Result<ResolvedAuth> {
            Err(Error::Auth("stub".into()))
        }
        fn have_credentials(&self) -> bool {
            true
        }
    }

    struct NeverHaveCreds;
    #[async_trait]
    impl AuthResolver for NeverHaveCreds {
        async fn resolve(&self) -> Result<ResolvedAuth> {
            Err(Error::Auth("stub".into()))
        }
        fn have_credentials(&self) -> bool {
            false
        }
    }

    #[test]
    fn multi_resolver_have_credentials_true_if_any() {
        let mr = MultiResolver::new(vec![
            Box::new(NeverHaveCreds),
            Box::new(AlwaysHaveCreds),
        ]);
        assert!(mr.have_credentials());
    }

    #[test]
    fn multi_resolver_have_credentials_false_if_none() {
        let mr = MultiResolver::new(vec![
            Box::new(NeverHaveCreds),
            Box::new(NeverHaveCreds),
        ]);
        assert!(!mr.have_credentials());
    }

    #[test]
    fn multi_resolver_have_credentials_true_with_empty() {
        let mr = MultiResolver::new(vec![]);
        assert!(!mr.have_credentials());
    }
}
