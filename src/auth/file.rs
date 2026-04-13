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
}
