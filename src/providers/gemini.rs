use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub struct GeminiProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl GeminiProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_quota(&self, key: &str) -> Result<ProviderResult> {
        let Some(project) = resolve_project_id() else {
            return Ok(ProviderResult {
                kind: ProviderKind::Gemini,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: "Unable to determine Gemini project id".to_string(),
                        console_url: Some(
                            "https://goo.gle/gemini-cli-auth-docs#workspace-gca".into(),
                        ),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: None,
                auth_source: None,
                cached_at: None,
            });
        };

        // The CodeAssist API uses a POST endpoint
        let resp = self
            .http
            .post("https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota")
            .header("Authorization", format!("Bearer {}", key))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "project": project
            }))
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        // Handle auth errors
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Ok(ProviderResult {
                kind: ProviderKind::Gemini,
                status: ProviderStatus::AuthRequired,
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            });
        }

        if !status.is_success() {
            let msg = body
                .get("error")
                .and_then(|v| v.get("message"))
                .and_then(|v| v.as_str())
                .or_else(|| body.get("message").and_then(|v| v.as_str()))
                .unwrap_or("unknown error");
            return Ok(ProviderResult {
                kind: ProviderKind::Gemini,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: msg.to_string(),
                        console_url: Some("https://aistudio.google.com/usage".into()),
                    },
                },
                fetched_at: Utc::now(),
                raw_response: Some(body),
                auth_source: None,
                cached_at: None,
            });
        }

        let quota = parse_quota(&body)?;
        Ok(ProviderResult {
            kind: ProviderKind::Gemini,
            status: ProviderStatus::Available { quota },
            fetched_at: Utc::now(),
            raw_response: Some(body),
            auth_source: None,
            cached_at: None,
        })
    }
}

pub(crate) fn resolve_project_id() -> Option<String> {
    if let Ok(project) = std::env::var("GOOGLE_CLOUD_PROJECT") {
        let project = project.trim();
        if !project.is_empty() {
            return Some(project.to_string());
        }
    }
    if let Ok(project) = std::env::var("GOOGLE_CLOUD_PROJECT_ID") {
        let project = project.trim();
        if !project.is_empty() {
            return Some(project.to_string());
        }
    }

    resolve_project_id_for_workspace(&std::env::current_dir().ok()?, &gemini_home_dir()?)
}

pub(crate) fn resolve_project_id_for_workspace(
    workspace_root: &Path,
    gemini_home: &Path,
) -> Option<String> {
    let registry_path = gemini_home.join("projects.json");
    let registry = load_project_registry(&registry_path);
    let normalized_workspace = normalize_project_path(workspace_root);
    if let Some(project) = registry.projects.get(&normalized_workspace) {
        return Some(project.clone());
    }

    Some(derive_project_slug(
        workspace_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project"),
        &registry.projects,
    ))
}

fn gemini_home_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("GEMINI_CLI_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    dirs::home_dir().map(|home| home.join(".gemini"))
}

#[derive(Debug, Deserialize, Default)]
struct ProjectRegistryFile {
    #[serde(default)]
    projects: BTreeMap<String, String>,
}

fn load_project_registry(path: &Path) -> ProjectRegistryFile {
    let Ok(content) = std::fs::read_to_string(path) else {
        return ProjectRegistryFile::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn normalize_project_path(path: &Path) -> String {
    let mut resolved = path.to_string_lossy().to_string();
    if cfg!(windows) {
        resolved = resolved.to_lowercase();
    }
    resolved
}

fn derive_project_slug(base_name: &str, existing_projects: &BTreeMap<String, String>) -> String {
    let slug = slugify(base_name);
    let existing_ids: BTreeSet<&str> = existing_projects.values().map(String::as_str).collect();
    if !existing_ids.contains(slug.as_str()) {
        return slug;
    }

    let mut counter = 1usize;
    loop {
        let candidate = format!("{}-{}", slug, counter);
        if !existing_ids.contains(candidate.as_str()) {
            return candidate;
        }
        counter += 1;
    }
}

fn slugify(text: &str) -> String {
    let slug = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    let slug = slug.trim_matches('-').to_string();
    let mut collapsed = String::new();
    let mut last_dash = false;
    for ch in slug.chars() {
        if ch == '-' {
            if !last_dash {
                collapsed.push(ch);
            }
            last_dash = true;
        } else {
            collapsed.push(ch);
            last_dash = false;
        }
    }
    if collapsed.is_empty() {
        "project".to_string()
    } else {
        collapsed
    }
}

#[derive(Debug, Deserialize)]
struct QuotaResponse {
    #[serde(default)]
    buckets: Vec<BucketInfo>,
}

#[derive(Debug, Deserialize)]
struct BucketInfo {
    #[serde(rename = "remainingAmount", default)]
    remaining_amount: String,
    #[serde(rename = "remainingFraction", default)]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime", default)]
    reset_time: Option<String>,
    #[serde(rename = "modelId", default)]
    model_id: Option<String>,
    #[serde(rename = "tokenType", default)]
    token_type: Option<String>,
}

pub(crate) fn parse_quota(body: &serde_json::Value) -> Result<ProviderQuota> {
    let resp: QuotaResponse = serde_json::from_value(body.clone())
        .map_err(|e| crate::Error::Provider(format!("parse error: {}", e)))?;

    let mut windows: Vec<QuotaWindow> = Vec::new();

    for bucket in &resp.buckets {
        let (remaining_amount, limit) = match (
            bucket.remaining_amount.parse::<i64>().ok(),
            bucket.remaining_fraction,
        ) {
            (Some(remaining), Some(fraction)) if fraction > 0.0 => {
                (remaining, (remaining as f64 / fraction).round() as i64)
            }
            (Some(remaining), _) => (remaining, remaining),
            (None, Some(fraction)) => {
                let limit = 100;
                (fraction.mul_add(limit as f64, 0.0).round() as i64, limit)
            }
            (None, None) => (0, 0),
        };

        let used = limit.saturating_sub(remaining_amount);

        let window_type = match (&bucket.token_type, &bucket.model_id) {
            (Some(tt), Some(mid)) => format!("{}_{}", tt.to_uppercase(), mid),
            (Some(tt), None) => tt.to_uppercase(),
            (None, Some(mid)) => mid.clone(),
            (None, None) => "unknown".to_string(),
        };

        let reset_at = bucket.reset_time.as_ref().and_then(|t| {
            DateTime::parse_from_rfc3339(t)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
        });

        windows.push(QuotaWindow {
            window_type,
            used,
            limit,
            remaining: remaining_amount,
            reset_at,
            period_seconds: None,
        });
    }

    let plan_name = if windows.is_empty() {
        "Gemini API (no quota data)".to_string()
    } else {
        "Gemini API".to_string()
    };

    Ok(ProviderQuota {
        plan_name,
        windows,
        unlimited: false,
    })
}

#[async_trait]
impl crate::providers::Provider for GeminiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Gemini
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = auth.credential.unwrap_token()?.to_string();

        match self.fetch_quota(&key).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Gemini,
                status: ProviderStatus::NetworkError {
                    message: e.to_string(),
                },
                fetched_at: Utc::now(),
                raw_response: None,
                auth_source: None,
                cached_at: None,
            }),
        }
    }

    fn auth_resolver(&self) -> &dyn AuthResolver {
        &*self.auth
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parses_quota_response() {
        let body = serde_json::json!({
            "buckets": [
                {
                    "remainingAmount": "850",
                    "remainingFraction": 0.85,
                    "resetTime": "2026-04-15T12:00:00Z",
                    "modelId": "gemini-2.5-flash",
                    "tokenType": "RPM"
                },
                {
                    "remainingAmount": "1500000",
                    "remainingFraction": 0.75,
                    "resetTime": "2026-04-15T00:00:00Z",
                    "modelId": "gemini-2.0-flash",
                    "tokenType": "RPD"
                },
                {
                    "remainingAmount": "1000000",
                    "remainingFraction": 0.5,
                    "resetTime": "2026-04-15T00:00:00Z",
                    "modelId": "gemini-2.0-flash",
                    "tokenType": "TPM"
                }
            ]
        });

        let quota = parse_quota(&body).unwrap();
        assert_eq!(quota.plan_name, "Gemini API");
        assert_eq!(quota.windows.len(), 3);

        // RPM window
        assert_eq!(quota.windows[0].window_type, "RPM_gemini-2.5-flash");
        assert_eq!(quota.windows[0].remaining, 850);
        assert_eq!(quota.windows[0].limit, 1000); // 850 / 0.85
        assert_eq!(quota.windows[0].used, 150); // 1000 - 850

        // RPD window
        assert_eq!(quota.windows[1].window_type, "RPD_gemini-2.0-flash");
        assert_eq!(quota.windows[1].remaining, 1500000);
        assert_eq!(quota.windows[1].limit, 2000000); // 1500000 / 0.75

        // TPM window
        assert_eq!(quota.windows[2].window_type, "TPM_gemini-2.0-flash");
        assert_eq!(quota.windows[2].remaining, 1000000);
        assert_eq!(quota.windows[2].limit, 2000000); // 1000000 / 0.5
    }

    #[test]
    fn parses_empty_buckets() {
        let body = serde_json::json!({
            "buckets": []
        });
        let quota = parse_quota(&body).unwrap();
        assert_eq!(quota.plan_name, "Gemini API (no quota data)");
        assert!(quota.windows.is_empty());
    }

    #[test]
    fn handles_missing_fraction() {
        let body = serde_json::json!({
            "buckets": [
                {
                    "remainingAmount": "100",
                    "resetTime": "2026-04-15T12:00:00Z",
                    "modelId": "gemini-2.0-flash",
                    "tokenType": "RPM"
                }
            ]
        });

        let quota = parse_quota(&body).unwrap();
        assert_eq!(quota.windows.len(), 1);
        // Without fraction, limit = remaining_amount
        assert_eq!(quota.windows[0].limit, 100);
        assert_eq!(quota.windows[0].remaining, 100);
        assert_eq!(quota.windows[0].used, 0);
    }

    #[test]
    fn resolves_project_from_google_cloud_project_env_first() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("GOOGLE_CLOUD_PROJECT", "project-a");
        std::env::set_var("GOOGLE_CLOUD_PROJECT_ID", "project-b");

        assert_eq!(resolve_project_id().as_deref(), Some("project-a"));

        std::env::remove_var("GOOGLE_CLOUD_PROJECT");
        std::env::remove_var("GOOGLE_CLOUD_PROJECT_ID");
    }

    #[test]
    fn resolves_project_from_google_cloud_project_id_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("GOOGLE_CLOUD_PROJECT");
        std::env::set_var("GOOGLE_CLOUD_PROJECT_ID", "project-b");

        assert_eq!(resolve_project_id().as_deref(), Some("project-b"));

        std::env::remove_var("GOOGLE_CLOUD_PROJECT_ID");
    }

    #[test]
    fn resolves_project_from_gemini_projects_registry_for_workspace() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("GOOGLE_CLOUD_PROJECT");
        std::env::remove_var("GOOGLE_CLOUD_PROJECT_ID");

        let temp_root = std::env::temp_dir().join(format!(
            "quotas-gemini-project-registry-{}",
            std::process::id()
        ));
        let workspace = temp_root.join("Workspace One");
        let gemini_home = temp_root.join("gemini-home");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&gemini_home).unwrap();
        std::fs::write(
            gemini_home.join("projects.json"),
            serde_json::json!({
                "projects": {
                    workspace.to_string_lossy().to_string(): "workspace-slug"
                }
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(
            resolve_project_id_for_workspace(&workspace, &gemini_home).as_deref(),
            Some("workspace-slug")
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn derives_project_slug_from_workspace_basename_when_registry_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("GOOGLE_CLOUD_PROJECT");
        std::env::remove_var("GOOGLE_CLOUD_PROJECT_ID");

        let temp_root = std::env::temp_dir().join(format!(
            "quotas-gemini-project-slug-{}",
            std::process::id()
        ));
        let workspace = temp_root.join("Workspace One");
        let gemini_home = temp_root.join("gemini-home");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&gemini_home).unwrap();

        assert_eq!(
            resolve_project_id_for_workspace(&workspace, &gemini_home).as_deref(),
            Some("workspace-one")
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn derives_normalized_quota_from_fraction_when_amount_is_missing() {
        let body = serde_json::json!({
            "buckets": [
                {
                    "remainingFraction": 0.96,
                    "resetTime": "2026-04-18T04:00:00Z",
                    "modelId": "gemini-2.5-flash",
                    "tokenType": "REQUESTS"
                }
            ]
        });

        let quota = parse_quota(&body).unwrap();

        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].window_type, "REQUESTS_gemini-2.5-flash");
        assert_eq!(quota.windows[0].limit, 100);
        assert_eq!(quota.windows[0].remaining, 96);
        assert_eq!(quota.windows[0].used, 4);
    }
}
