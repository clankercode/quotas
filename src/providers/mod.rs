pub mod codex;
pub mod copilot;
pub mod kimi;
pub mod minimax;
pub mod zai;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::auth::AuthResolver;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Minimax,
    Zai,
    Kimi,
    Copilot,
    Codex,
}

impl ProviderKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            ProviderKind::Minimax => "MiniMax",
            ProviderKind::Zai => "Z.ai",
            ProviderKind::Kimi => "Kimi",
            ProviderKind::Copilot => "GitHub Copilot",
            ProviderKind::Codex => "Codex",
        }
    }

    pub fn all() -> &'static [ProviderKind] {
        &[
            ProviderKind::Minimax,
            ProviderKind::Zai,
            ProviderKind::Kimi,
            ProviderKind::Copilot,
            ProviderKind::Codex,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaWindow {
    pub window_type: String,
    pub used: i64,
    pub limit: i64,
    pub remaining: i64,
    pub reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderQuota {
    pub plan_name: String,
    pub windows: Vec<QuotaWindow>,
    pub unlimited: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnavailableInfo {
    pub reason: String,
    pub console_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderStatus {
    Available { quota: ProviderQuota },
    Unavailable { info: UnavailableInfo },
    AuthRequired,
    NetworkError { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResult {
    pub kind: ProviderKind,
    pub status: ProviderStatus,
    pub fetched_at: DateTime<Utc>,
    pub raw_response: Option<serde_json::Value>,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    async fn fetch(&self) -> crate::Result<ProviderResult>;
    fn auth_resolver(&self) -> &dyn AuthResolver;
}
