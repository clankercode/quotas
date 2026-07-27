//! Auth from pi agent (`~/.pi/agent/auth.json`).
//!
//! Shape is a map of provider plugin slots → oauth / api_key entries, similar
//! to opencode but with plugin-specific key names. Matching is fuzzy so
//! `xai-oauth`, `xai`, `grok`, etc. all resolve for Grok.

use super::slotmap::{extract_slot_credential, find_best_slot_key};
use super::{AuthCredential, AuthResolver, ResolvedAuth};
use crate::providers::ProviderKind;
use crate::{Error, Result};
use async_trait::async_trait;
use std::path::PathBuf;

pub struct PiAuthResolver {
    pub file_paths: Vec<PathBuf>,
    pub kind: ProviderKind,
}

impl PiAuthResolver {
    pub fn new(kind: ProviderKind) -> Self {
        Self {
            file_paths: default_pi_auth_paths(),
            kind,
        }
    }

    pub fn with_paths(kind: ProviderKind, file_paths: Vec<PathBuf>) -> Self {
        Self { file_paths, kind }
    }
}

pub fn default_pi_auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".pi/agent/auth.json"));
    }
    // XDG-style fallback some installs use.
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            paths.push(PathBuf::from(xdg).join("pi/agent/auth.json"));
        }
    }
    paths
}

fn resolve_from_map(map: &serde_json::Map<String, serde_json::Value>, kind: ProviderKind) -> Option<(String, String)> {
    let keys: Vec<&str> = map.keys().map(|s| s.as_str()).collect();
    let slot = find_best_slot_key(keys, kind)?;
    let entry = map.get(&slot)?;
    let token = extract_slot_credential(entry)?;
    Some((slot, token))
}

#[async_trait]
impl AuthResolver for PiAuthResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        for path in &self.file_paths {
            if !path.exists() {
                continue;
            }
            let content = match tokio::fs::read_to_string(path).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            let root: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let Some(map) = root.as_object() else {
                continue;
            };
            if let Some((slot, token)) = resolve_from_map(map, self.kind) {
                return Ok(ResolvedAuth {
                    credential: AuthCredential::Token(token),
                    source: format!("pi:{}:{}", path.display(), slot),
                });
            }
        }
        Err(Error::Auth(format!(
            "pi agent auth.json has no usable slot for {}",
            self.kind.slug()
        )))
    }

    fn have_credentials(&self) -> bool {
        for path in &self.file_paths {
            if !path.exists() {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) else {
                continue;
            };
            let Some(map) = root.as_object() else {
                continue;
            };
            if resolve_from_map(map, self.kind).is_some() {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn resolves_xai_oauth_slot_for_grok() {
        let dir = std::env::temp_dir().join(format!("quotas_pi_auth_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("auth.json");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            write!(
                f,
                r#"{{
                    "minimax": {{ "type": "api_key", "key": "mm-key" }},
                    "xai-oauth": {{
                        "type": "oauth",
                        "access": "xai-access-token",
                        "refresh": "r"
                    }}
                }}"#
            )
            .unwrap();
        }
        let resolver = PiAuthResolver::with_paths(ProviderKind::Grok, vec![path.clone()]);
        assert!(resolver.have_credentials());
        let auth = resolver.resolve().await.unwrap();
        assert_eq!(auth.credential.unwrap_token().unwrap(), "xai-access-token");
        assert!(auth.source.contains("xai-oauth"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resolves_openai_codex_for_codex() {
        let dir = std::env::temp_dir().join(format!("quotas_pi_codex_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("auth.json");
        std::fs::write(
            &path,
            r#"{"openai-codex":{"type":"oauth","access":"codex-tok","refresh":"r","expires":1}}"#,
        )
        .unwrap();
        let resolver = PiAuthResolver::with_paths(ProviderKind::Codex, vec![path]);
        let auth = resolver.resolve().await.unwrap();
        assert_eq!(auth.credential.unwrap_token().unwrap(), "codex-tok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn skips_null_github_copilot_slot() {
        let dir = std::env::temp_dir().join(format!("quotas_pi_copilot_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("auth.json");
        std::fs::write(&path, r#"{"github-copilot":{"type":null}}"#).unwrap();
        let resolver = PiAuthResolver::with_paths(ProviderKind::GitHubCopilot, vec![path]);
        assert!(!resolver.have_credentials());
        assert!(resolver.resolve().await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
