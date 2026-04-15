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

    fn have_credentials(&self) -> bool {
        self.env_vars
            .iter()
            .any(|(name, _)| env::var(name).is_ok_and(|v| !v.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn have_credentials_true_when_env_set() {
        let var_name = "TEST_QUOTAS_ENV_RESOLVER_HAVE_CRED";
        env::set_var(var_name, "sk-test-123");
        let resolver = EnvResolver::new(vec![(var_name, "test")]);
        assert!(resolver.have_credentials());
        env::remove_var(var_name);
    }

    #[test]
    fn have_credentials_false_when_env_unset() {
        let var_name = "TEST_QUOTAS_ENV_RESOLVER_NOPE_XYZ";
        env::remove_var(var_name); // ensure it doesn't exist
        let resolver = EnvResolver::new(vec![(var_name, "test")]);
        assert!(!resolver.have_credentials());
    }

    #[test]
    fn have_credentials_false_when_env_empty() {
        let var_name = "TEST_QUOTAS_ENV_RESOLVER_EMPTY";
        env::set_var(var_name, "");
        let resolver = EnvResolver::new(vec![(var_name, "test")]);
        assert!(!resolver.have_credentials());
        env::remove_var(var_name);
    }

    #[test]
    fn have_credentials_checks_any_of_multiple() {
        let var1 = "TEST_QUOTAS_ENV_RESOLVER_MULTI1";
        let var2 = "TEST_QUOTAS_ENV_RESOLVER_MULTI2";
        env::remove_var(var1);
        env::set_var(var2, "sk-present");
        let resolver = EnvResolver::new(vec![(var1, "first"), (var2, "second")]);
        assert!(resolver.have_credentials());
        env::remove_var(var2);
    }
}
