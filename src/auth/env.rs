use crate::{Error, Result};
use async_trait::async_trait;
use std::env;

use super::{AuthCredential, AuthResolver, ResolvedAuth};

pub struct EnvResolver {
    pub env_vars: Vec<(&'static str, &'static str)>,
}

impl EnvResolver {
    pub fn new(vars: Vec<(&'static str, &'static str)>) -> Self {
        Self { env_vars: vars }
    }
}

#[async_trait]
impl AuthResolver for EnvResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        for (env_name, _label) in &self.env_vars {
            if let Ok(val) = env::var(env_name) {
                if !val.is_empty() {
                    return Ok(ResolvedAuth {
                        credential: AuthCredential::Bearer(val),
                        source: format!("env:{}", env_name),
                    });
                }
            }
        }
        Err(Error::Auth("env vars not found".into()))
    }
}
