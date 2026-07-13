pub mod claude;
pub mod codex;
pub mod cursor;
pub mod deepseek;
pub mod antigravity;
pub mod github_copilot;
pub mod grok;
pub mod kimi;
pub mod mimo;
pub mod minimax;
pub mod openrouter;
pub mod siliconflow;
pub mod zai;

use crate::auth::AuthResolver;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Claude,
    Codex,
    Cursor,
    DeepSeek,
    /// Google Antigravity (agy). `gemini` is accepted as a serde alias for
    /// older cache/config that used the previous provider name.
    #[serde(alias = "gemini")]
    Antigravity,
    GitHubCopilot,
    Grok,
    Kimi,
    Mimo,
    Minimax,
    OpenRouter,
    SiliconFlow,
    Zai,
}

impl ProviderKind {
    /// How long before this provider auto-refreshes (seconds).
    /// Drives both the auto-refresh timer and the freshness progress bar.
    pub fn auto_refresh_secs(&self) -> u64 {
        match self {
            ProviderKind::Claude => 600,
            ProviderKind::GitHubCopilot => 600,
            _ => 300,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ProviderKind::Claude => "Claude",
            ProviderKind::Codex => "Codex",
            ProviderKind::Cursor => "Cursor",
            ProviderKind::DeepSeek => "DeepSeek",
            ProviderKind::Antigravity => "Antigravity",
            ProviderKind::GitHubCopilot => "Copilot",
            ProviderKind::Grok => "Grok",
            ProviderKind::Kimi => "Kimi",
            ProviderKind::Mimo => "MiMo",
            ProviderKind::Minimax => "MiniMax",
            ProviderKind::OpenRouter => "OpenRouter",
            ProviderKind::SiliconFlow => "SiliconFlow",
            ProviderKind::Zai => "Z.ai",
        }
    }

    pub fn slug(&self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Codex => "codex",
            ProviderKind::Cursor => "cursor",
            ProviderKind::DeepSeek => "deepseek",
            ProviderKind::Antigravity => "antigravity",
            ProviderKind::GitHubCopilot => "github-copilot",
            ProviderKind::Grok => "grok",
            ProviderKind::Kimi => "kimi",
            ProviderKind::Mimo => "mimo",
            ProviderKind::Minimax => "minimax",
            ProviderKind::OpenRouter => "openrouter",
            ProviderKind::SiliconFlow => "siliconflow",
            ProviderKind::Zai => "zai",
        }
    }

    pub fn all() -> &'static [ProviderKind] {
        &[
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::Cursor,
            ProviderKind::DeepSeek,
            ProviderKind::Antigravity,
            ProviderKind::GitHubCopilot,
            ProviderKind::Grok,
            ProviderKind::Kimi,
            ProviderKind::Minimax,
            ProviderKind::OpenRouter,
            ProviderKind::SiliconFlow,
            ProviderKind::Zai,
            // ProviderKind::Mimo, // disabled — platform API requires browser cookie auth
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
    /// Total length of the rate-limit window, in seconds. Lets the UI
    /// render a "time elapsed" marker on the quota bar so users can see
    /// whether they're burning quota faster than the clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_seconds: Option<i64>,
}

/// Banked / earned rate-limit reset credits (Codex and similar).
///
/// These are not rolling windows — they are redeemable "full reset" tokens
/// that refill usage when consumed. Count is enough for the card; per-credit
/// detail (title, expiry) is for the detail view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BankedResets {
    /// Authoritative total available (backend may cap the `credits` array).
    pub available_count: i64,
    /// Optional per-credit rows when the detail endpoint was fetched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credits: Vec<BankedResetCredit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BankedResetCredit {
    pub id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Grant source label when present (e.g. "Codex Team", referral handle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderQuota {
    pub plan_name: String,
    pub windows: Vec<QuotaWindow>,
    pub unlimited: bool,
    /// Redeemable banked rate-limit resets (e.g. Codex). Absent when the
    /// provider does not report any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banked_resets: Option<BankedResets>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_source: Option<String>,
    /// Set when this result was read from cache (the time it was originally fetched).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_at: Option<DateTime<Utc>>,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    async fn fetch(&self) -> crate::Result<ProviderResult>;
    fn auth_resolver(&self) -> &dyn AuthResolver;
}
