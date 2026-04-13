use crate::providers::{ProviderKind, ProviderResult, ProviderStatus};
use serde::Serialize;

#[derive(Serialize)]
pub struct JsonOutput {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub providers: Vec<ProviderJson>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderStatusJson {
    Available {
        plan_name: String,
        windows: Vec<WindowJson>,
        unlimited: bool,
    },
    Unavailable {
        reason: String,
        console_url: Option<String>,
    },
    AuthRequired,
    NetworkError {
        message: String,
    },
}

#[derive(Serialize)]
pub struct WindowJson {
    pub window_type: String,
    pub used: i64,
    pub limit: i64,
    pub remaining: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_seconds: Option<i64>,
}

#[derive(Serialize)]
pub struct ProviderJson {
    pub name: String,
    pub status: ProviderStatusJson,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_response: Option<serde_json::Value>,
}

impl JsonOutput {
    pub fn from_results(results: Vec<ProviderResult>) -> Self {
        let providers = results
            .into_iter()
            .map(|r| ProviderJson {
                name: r.kind.display_name().to_lowercase(),
                status: match r.status {
                    ProviderStatus::Available { quota } => ProviderStatusJson::Available {
                        plan_name: quota.plan_name,
                        windows: quota
                            .windows
                            .into_iter()
                            .map(|w| WindowJson {
                                window_type: w.window_type,
                                used: w.used,
                                limit: w.limit,
                                remaining: w.remaining,
                                reset_at: w.reset_at,
                                period_seconds: w.period_seconds,
                            })
                            .collect(),
                        unlimited: quota.unlimited,
                    },
                    ProviderStatus::Unavailable { info } => ProviderStatusJson::Unavailable {
                        reason: info.reason,
                        console_url: info.console_url,
                    },
                    ProviderStatus::AuthRequired => ProviderStatusJson::AuthRequired,
                    ProviderStatus::NetworkError { message } => {
                        ProviderStatusJson::NetworkError { message }
                    }
                },
                fetched_at: r.fetched_at,
                raw_response: r.raw_response,
            })
            .collect();

        Self {
            timestamp: chrono::Utc::now(),
            providers,
        }
    }

    pub fn to_json(&self, pretty: bool) -> String {
        if pretty {
            serde_json::to_string_pretty(self).unwrap_or_default()
        } else {
            serde_json::to_string(self).unwrap_or_default()
        }
    }
}

pub fn filter_results(results: Vec<ProviderResult>, kinds: &[ProviderKind]) -> Vec<ProviderResult> {
    if kinds.is_empty() {
        return results;
    }
    results
        .into_iter()
        .filter(|r| kinds.contains(&r.kind))
        .collect()
}
