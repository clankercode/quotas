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

/// Resolves a Volcengine/BytePlus access-key + secret-key pair from the
/// environment. Both halves must be present. The first non-empty variable in
/// each list wins, allowing a provider to accept its own name and the `VOLC_*`
/// names the official tooling uses.
pub struct AkSkEnvResolver {
    access_key_vars: Vec<&'static str>,
    secret_key_vars: Vec<&'static str>,
    label: &'static str,
}

impl AkSkEnvResolver {
    pub fn new(
        access_key_vars: Vec<&'static str>,
        secret_key_vars: Vec<&'static str>,
        label: &'static str,
    ) -> Self {
        Self {
            access_key_vars,
            secret_key_vars,
            label,
        }
    }

    fn first_present(vars: &[&'static str]) -> Option<(&'static str, String)> {
        vars.iter().find_map(|name| {
            env::var(name)
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| (*name, v))
        })
    }
}

#[async_trait]
impl AuthResolver for AkSkEnvResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        let access = Self::first_present(&self.access_key_vars);
        let secret = Self::first_present(&self.secret_key_vars);
        match (access, secret) {
            (Some((access_name, access_key)), Some((_, secret_key))) => Ok(ResolvedAuth {
                credential: AuthCredential::AkSk {
                    access_key,
                    secret_key,
                },
                source: format!("env:{}", access_name),
            }),
            _ => Err(Error::Auth(format!(
                "{} access-key/secret-key env vars not both set",
                self.label
            ))),
        }
    }

    fn have_credentials(&self) -> bool {
        Self::first_present(&self.access_key_vars).is_some()
            && Self::first_present(&self.secret_key_vars).is_some()
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

    #[tokio::test]
    async fn aksk_resolves_when_both_present() {
        let ak = "TEST_QUOTAS_AKSK_AK";
        let sk = "TEST_QUOTAS_AKSK_SK";
        env::set_var(ak, "AKLT-test");
        env::set_var(sk, "secret-test");
        let resolver = AkSkEnvResolver::new(vec![ak], vec![sk], "test");
        assert!(resolver.have_credentials());
        let resolved = resolver.resolve().await.unwrap();
        let (a, s) = resolved.credential.unwrap_aksk().unwrap();
        assert_eq!(a, "AKLT-test");
        assert_eq!(s, "secret-test");
        env::remove_var(ak);
        env::remove_var(sk);
    }

    #[test]
    fn aksk_requires_both_halves() {
        let ak = "TEST_QUOTAS_AKSK_ONLY_AK";
        let sk = "TEST_QUOTAS_AKSK_ONLY_SK";
        env::set_var(ak, "AKLT-test");
        env::remove_var(sk);
        let resolver = AkSkEnvResolver::new(vec![ak], vec![sk], "test");
        assert!(!resolver.have_credentials());
        env::remove_var(ak);
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
