use crate::{Error, Result};
use async_trait::async_trait;
use std::path::PathBuf;

use super::{AuthCredential, AuthResolver, ResolvedAuth};

pub struct FileResolver {
    pub file_paths: Vec<PathBuf>,
    #[allow(clippy::type_complexity)]
    pub parse_fn: Box<dyn Fn(&str) -> Option<String> + Send + Sync>,
    pub source_name: String,
}

impl FileResolver {
    pub fn new<F>(file_paths: Vec<PathBuf>, parse_fn: F, source_name: &str) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        Self {
            file_paths,
            parse_fn: Box::new(parse_fn),
            source_name: source_name.to_string(),
        }
    }
}

#[async_trait]
impl AuthResolver for FileResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        for path in &self.file_paths {
            if let Ok(content) = tokio::fs::read_to_string(path).await {
                let key = (self.parse_fn)(&content);
                if let Some(key) = key {
                    return Ok(ResolvedAuth {
                        credential: AuthCredential::Bearer(key),
                        source: format!("file:{}", path.display()),
                    });
                }
            }
        }
        Err(Error::Auth(format!(
            "no credentials in files for {}",
            self.source_name
        )))
    }

    fn have_credentials(&self) -> bool {
        self.file_paths.iter().any(|p| p.exists())
    }
}

/// Reads a raw cookie value from a file (first non-empty, non-comment line).
pub struct CookieFileResolver {
    pub file_paths: Vec<PathBuf>,
    pub source_name: String,
}

impl CookieFileResolver {
    pub fn new(file_paths: Vec<PathBuf>, source_name: &str) -> Self {
        Self {
            file_paths,
            source_name: source_name.to_string(),
        }
    }
}

#[async_trait]
impl AuthResolver for CookieFileResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        for path in &self.file_paths {
            if let Ok(content) = tokio::fs::read_to_string(path).await {
                let value = content.trim();
                if !value.is_empty() {
                    return Ok(ResolvedAuth {
                        credential: AuthCredential::Cookie(value.to_string()),
                        source: format!("cookie:{}", path.display()),
                    });
                }
            }
        }
        Err(Error::Auth(format!(
            "no cookie credentials in files for {}",
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
    use std::fs;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("quotas_test_{}", name))
    }

    #[test]
    fn file_resolver_have_credentials_true_when_file_exists() {
        let path = temp_path("file_resolver_have.txt");
        fs::write(&path, "sk-test-key").unwrap();
        let resolver = FileResolver::new(vec![path.clone()], |c| Some(c.to_string()), "test");
        assert!(resolver.have_credentials());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn file_resolver_have_credentials_false_when_no_files() {
        let path = temp_path("file_resolver_no_such_file.txt");
        let _ = fs::remove_file(&path);
        let resolver = FileResolver::new(vec![path], |c| Some(c.to_string()), "test");
        assert!(!resolver.have_credentials());
    }

    #[test]
    fn file_resolver_have_credentials_checks_any_of_multiple() {
        let missing = temp_path("file_resolver_missing.txt");
        let present = temp_path("file_resolver_present.txt");
        let _ = fs::remove_file(&missing);
        fs::write(&present, "sk-key").unwrap();
        let resolver = FileResolver::new(
            vec![missing, present.clone()],
            |c| Some(c.to_string()),
            "test",
        );
        assert!(resolver.have_credentials());
        let _ = fs::remove_file(&present);
    }

    #[test]
    fn cookie_file_resolver_have_credentials_true_when_file_exists() {
        let path = temp_path("cookie_resolver_have.txt");
        fs::write(&path, "cookie-value").unwrap();
        let resolver = CookieFileResolver::new(vec![path.clone()], "test");
        assert!(resolver.have_credentials());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn cookie_file_resolver_have_credentials_false_when_no_files() {
        let path = temp_path("cookie_resolver_no_such_file.txt");
        let _ = fs::remove_file(&path);
        let resolver = CookieFileResolver::new(vec![path], "test");
        assert!(!resolver.have_credentials());
    }
}
