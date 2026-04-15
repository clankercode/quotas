use crate::{Error, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::{AuthCredential, AuthResolver, ResolvedAuth};

/// Which opencode provider slot this resolver pulls from.
#[derive(Clone, Copy, Debug)]
pub enum OpencodeSlot {
    Anthropic,
    Openai,
    Minimax,
    Kimi,
    Zai,
}

impl OpencodeSlot {
    fn key(&self) -> &'static str {
        match self {
            OpencodeSlot::Anthropic => "anthropic",
            OpencodeSlot::Openai => "openai",
            OpencodeSlot::Minimax => "minimax-coding-plan",
            OpencodeSlot::Kimi => "kimi-for-coding",
            OpencodeSlot::Zai => "zai-coding-plan",
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OpencodeEntry {
    Oauth {
        access: String,
        #[serde(rename = "type", default)]
        _type: Option<String>,
    },
    Api {
        key: String,
        #[serde(rename = "type", default)]
        _type: Option<String>,
    },
}

pub struct OpencodeAuthResolver {
    pub file_paths: Vec<PathBuf>,
    pub slot: OpencodeSlot,
}

impl OpencodeAuthResolver {
    pub fn new(slot: OpencodeSlot) -> Self {
        let mut paths = Vec::new();
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".local/share/opencode/auth.json"));
            paths.push(home.join(".config/opencode/auth.json"));
        }
        if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
            paths.insert(0, PathBuf::from(data_home).join("opencode/auth.json"));
        }
        Self {
            file_paths: paths,
            slot,
        }
    }
}

#[async_trait]
impl AuthResolver for OpencodeAuthResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        for path in &self.file_paths {
            if !path.exists() {
                continue;
            }
            let content = match tokio::fs::read_to_string(path).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            let map: serde_json::Value = serde_json::from_str(&content)
                .map_err(|e| Error::Auth(format!("opencode auth.json parse: {}", e)))?;
            let Some(entry) = map.get(self.slot.key()) else {
                continue;
            };
            let parsed: OpencodeEntry = match serde_json::from_value(entry.clone()) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let (cred, label) = match parsed {
                OpencodeEntry::Oauth { access, .. } => (AuthCredential::Token(access), "oauth"),
                OpencodeEntry::Api { key, .. } => (AuthCredential::Bearer(key), "api"),
            };
            return Ok(ResolvedAuth {
                credential: cred,
                source: format!("opencode:{}:{}", self.slot.key(), label),
            });
        }
        Err(Error::Auth(format!(
            "opencode auth.json missing slot {}",
            self.slot.key()
        )))
    }

    fn have_credentials(&self) -> bool {
        self.file_paths.iter().any(|p| {
            if !p.exists() {
                return false;
            }
            std::fs::read_to_string(p)
                .ok()
                .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                .is_some_and(|v| v.get(self.slot.key()).is_some())
        })
    }
}

/// Kimi CLI stores an OAuth access token at ~/.kimi/credentials/kimi-code.json.
pub struct KimiCliResolver {
    pub file_path: PathBuf,
}

impl KimiCliResolver {
    pub fn new() -> Self {
        Self {
            file_path: dirs::home_dir()
                .unwrap_or_default()
                .join(".kimi/credentials/kimi-code.json"),
        }
    }
}

impl Default for KimiCliResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuthResolver for KimiCliResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        if !self.file_path.exists() {
            return Err(Error::Auth(format!(
                "kimi-cli credentials not found at {}",
                self.file_path.display()
            )));
        }
        let content = tokio::fs::read_to_string(&self.file_path).await?;
        #[derive(Deserialize)]
        struct KimiCreds {
            access_token: String,
        }
        let parsed: KimiCreds = serde_json::from_str(&content)
            .map_err(|e| Error::Auth(format!("kimi-cli credentials parse: {}", e)))?;
        Ok(ResolvedAuth {
            credential: AuthCredential::Bearer(parsed.access_token),
            source: format!("kimi-cli:{}", self.file_path.display()),
        })
    }

    fn have_credentials(&self) -> bool {
        self.file_path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opencode_oauth_entry() {
        let json = r#"{"anthropic":{"type":"oauth","access":"tok","refresh":"r","expires":0}}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let entry = v.get("anthropic").unwrap().clone();
        let parsed: OpencodeEntry = serde_json::from_value(entry).unwrap();
        match parsed {
            OpencodeEntry::Oauth { access, .. } => assert_eq!(access, "tok"),
            _ => panic!("expected oauth"),
        }
    }

    #[test]
    fn parses_opencode_api_entry() {
        let json = r#"{"minimax-coding-plan":{"type":"api","key":"sk-cp-abc"}}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let entry = v.get("minimax-coding-plan").unwrap().clone();
        let parsed: OpencodeEntry = serde_json::from_value(entry).unwrap();
        match parsed {
            OpencodeEntry::Api { key, .. } => assert_eq!(key, "sk-cp-abc"),
            _ => panic!("expected api"),
        }
    }
}
