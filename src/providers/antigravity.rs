use crate::auth::AuthResolver;
use crate::providers::{ProviderKind, ProviderQuota, ProviderResult, ProviderStatus, QuotaWindow};
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Antigravity (agy) / Google Code Assist quota provider.
///
/// Primary endpoint: `POST …/v1internal:retrieveUserQuotaSummary`
/// which returns 2 groups (Gemini models, Claude+GPT), each with a
/// shared weekly + 5h bucket — matching what the agy CLI renders.
pub struct AntigravityProvider {
    http: Client,
    auth: Box<dyn AuthResolver>,
}

impl AntigravityProvider {
    pub fn new(auth: Box<dyn AuthResolver>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn fetch_quota(&self, key: &str) -> Result<ProviderResult> {
        // Project is optional for the summary endpoint (empty body works),
        // but prefer the resolved workspace/default project when known.
        let project = resolve_project_id();
        let body = match project.as_deref() {
            Some(p) if !p.is_empty() => serde_json::json!({ "project": p }),
            _ => serde_json::json!({}),
        };

        // cloudcode-pa rejects calls without a recognized client UA (403
        // PERMISSION_DENIED). The agy binary identifies as "antigravity".
        let resp = self
            .http
            .post("https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary")
            .header("Authorization", format!("Bearer {}", key))
            .header("Content-Type", "application/json")
            .header("User-Agent", "antigravity")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;

        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Ok(ProviderResult {
                kind: ProviderKind::Antigravity,
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
                kind: ProviderKind::Antigravity,
                status: ProviderStatus::Unavailable {
                    info: crate::providers::UnavailableInfo {
                        reason: msg.to_string(),
                        console_url: Some("https://antigravity.google".into()),
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
            kind: ProviderKind::Antigravity,
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

    // agy default project cache
    if let Some(home) = gemini_home_dir() {
        let default = home
            .join("antigravity-cli/cache/default_project_id.txt");
        if let Ok(s) = std::fs::read_to_string(&default) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
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

// ── Summary response (agy CLI primary surface) ──────────────────────────

#[derive(Debug, Deserialize)]
struct QuotaSummaryResponse {
    #[serde(default)]
    groups: Vec<QuotaSummaryGroup>,
    /// Legacy flat list (older / business endpoints may still emit this).
    #[serde(default)]
    buckets: Vec<QuotaSummaryBucket>,
}

#[derive(Debug, Deserialize)]
struct QuotaSummaryGroup {
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
    #[serde(default)]
    buckets: Vec<QuotaSummaryBucket>,
}

#[derive(Debug, Deserialize)]
struct QuotaSummaryBucket {
    #[serde(rename = "bucketId", default)]
    bucket_id: Option<String>,
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
    #[serde(default)]
    window: Option<String>,
    #[serde(rename = "resetTime", default)]
    reset_time: Option<String>,
    #[serde(rename = "remainingFraction", default)]
    remaining_fraction: Option<f64>,
    #[serde(rename = "remainingAmount", default)]
    remaining_amount: Option<String>,
}

/// Parse a `/v1internal:retrieveUserQuotaSummary` (or compatible) body.
///
/// Emits one `QuotaWindow` per bucket, with `window_type` shaped as
/// `{period}/{group}` (e.g. `5h/gemini`, `7d/3p`) so the TUI clusters
/// by period and labels by group — matching the agy CLI's 2×(5h+7d) grid.
pub(crate) fn parse_quota(body: &serde_json::Value) -> Result<ProviderQuota> {
    // Prefer the summary shape; fall back to the older per-model buckets
    // list (`retrieveUserQuota`) so cached/legacy payloads still parse.
    if body.get("groups").is_some() || looks_like_summary_buckets(body) {
        return parse_summary_quota(body);
    }
    parse_legacy_model_quota(body)
}

fn looks_like_summary_buckets(body: &serde_json::Value) -> bool {
    body.get("buckets")
        .and_then(|b| b.as_array())
        .and_then(|arr| arr.first())
        .map(|b| b.get("bucketId").is_some() || b.get("window").is_some())
        .unwrap_or(false)
}

fn parse_summary_quota(body: &serde_json::Value) -> Result<ProviderQuota> {
    let resp: QuotaSummaryResponse = serde_json::from_value(body.clone())
        .map_err(|e| crate::Error::Provider(format!("parse error: {}", e)))?;

    let mut windows = Vec::new();

    if !resp.groups.is_empty() {
        for group in &resp.groups {
            let group_label = group_slug(group);
            for bucket in &group.buckets {
                if let Some(w) = summary_bucket_to_window(bucket, &group_label) {
                    windows.push(w);
                }
            }
        }
    } else {
        // Flat buckets with bucketId/window — no group wrapper.
        for bucket in &resp.buckets {
            let label = bucket
                .bucket_id
                .as_deref()
                .and_then(|id| id.rsplit_once('-').map(|(p, _)| p.to_string()))
                .unwrap_or_else(|| "group".into());
            if let Some(w) = summary_bucket_to_window(bucket, &label) {
                windows.push(w);
            }
        }
    }

    let plan_name = if windows.is_empty() {
        "Antigravity (no quota data)".to_string()
    } else {
        "Antigravity".to_string()
    };

    Ok(ProviderQuota {
        plan_name,
        windows,
        unlimited: false,
    })
}

fn group_slug(group: &QuotaSummaryGroup) -> String {
    // Prefer stable bucketId prefix (gemini-weekly → gemini, 3p-5h → 3p).
    for bucket in &group.buckets {
        if let Some(id) = bucket.bucket_id.as_deref() {
            if let Some((prefix, rest)) = id.rsplit_once('-') {
                if matches!(rest, "weekly" | "5h" | "7d" | "wk") && !prefix.is_empty() {
                    return prefix.to_string();
                }
            }
        }
    }
    // Fallback: first word of displayName lowercased.
    group
        .display_name
        .as_deref()
        .and_then(|s| s.split_whitespace().next())
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "group".into())
}

fn summary_bucket_to_window(bucket: &QuotaSummaryBucket, group: &str) -> Option<QuotaWindow> {
    let period_key = normalize_window_key(
        bucket
            .window
            .as_deref()
            .or_else(|| {
                // Infer from bucketId suffix when `window` is absent.
                bucket
                    .bucket_id
                    .as_deref()
                    .and_then(|id| id.rsplit_once('-').map(|(_, rest)| rest))
            })
            .unwrap_or("window"),
    );

    let (used, limit, remaining) = fraction_to_counts(
        bucket.remaining_fraction,
        bucket.remaining_amount.as_deref(),
    );

    let reset_at = bucket.reset_time.as_ref().and_then(|t| {
        DateTime::parse_from_rfc3339(t)
            .map(|dt| dt.with_timezone(&Utc))
            .ok()
    });

    let period_seconds = match period_key.as_str() {
        "5h" => Some(5 * 3600),
        "7d" => Some(7 * 86400),
        _ => None,
    };

    // Prefer a short display-friendly group token; fall back to group slug.
    let _ = bucket.display_name.as_ref(); // reserved for future richer labels

    Some(QuotaWindow {
        window_type: format!("{period_key}/{group}"),
        used,
        limit,
        remaining,
        reset_at,
        period_seconds,
    })
}

fn normalize_window_key(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "weekly" | "week" | "wk" | "7d" | "168h" => "7d".into(),
        "5h" | "five_hour" | "five-hour" | "5hour" | "300m" => "5h".into(),
        "" => "window".into(),
        other => other.to_string(),
    }
}

/// Convert remainingFraction (0..1) and/or remainingAmount into used/limit/remaining.
/// Percentage scale (limit=100) when only a fraction is present — matches
/// how the agy CLI and other quotas bars render utilization.
fn fraction_to_counts(
    remaining_fraction: Option<f64>,
    remaining_amount: Option<&str>,
) -> (i64, i64, i64) {
    match (
        remaining_amount.and_then(|s| s.parse::<i64>().ok()),
        remaining_fraction,
    ) {
        (Some(remaining), Some(fraction)) if fraction > 0.0 => {
            let limit = (remaining as f64 / fraction).round() as i64;
            let used = limit.saturating_sub(remaining);
            (used, limit, remaining)
        }
        (Some(remaining), _) => (0, remaining, remaining),
        (None, Some(fraction)) => {
            let limit = 100i64;
            // Clamp to [0, 100]; round half-up for the remaining percent.
            let remaining = (fraction.clamp(0.0, 1.0) * limit as f64).round() as i64;
            let used = limit.saturating_sub(remaining);
            (used, limit, remaining)
        }
        (None, None) => (0, 0, 0),
    }
}

// ── Legacy per-model retrieveUserQuota (kept for cache rehydrate) ───────

#[derive(Debug, Deserialize)]
struct LegacyQuotaResponse {
    #[serde(default)]
    buckets: Vec<LegacyBucketInfo>,
}

#[derive(Debug, Deserialize)]
struct LegacyBucketInfo {
    #[serde(rename = "remainingAmount", default)]
    remaining_amount: Option<String>,
    #[serde(rename = "remainingFraction", default)]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime", default)]
    reset_time: Option<String>,
    #[serde(rename = "modelId", default)]
    model_id: Option<String>,
    #[serde(rename = "tokenType", default)]
    token_type: Option<String>,
}

fn parse_legacy_model_quota(body: &serde_json::Value) -> Result<ProviderQuota> {
    let resp: LegacyQuotaResponse = serde_json::from_value(body.clone())
        .map_err(|e| crate::Error::Provider(format!("parse error: {}", e)))?;

    let mut windows: Vec<QuotaWindow> = Vec::new();

    for bucket in &resp.buckets {
        let (used, limit, remaining) = fraction_to_counts(
            bucket.remaining_fraction,
            bucket.remaining_amount.as_deref(),
        );

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
            remaining,
            reset_at,
            period_seconds: None,
        });
    }

    let plan_name = if windows.is_empty() {
        "Antigravity (no quota data)".to_string()
    } else {
        "Antigravity".to_string()
    };

    Ok(ProviderQuota {
        plan_name,
        windows,
        unlimited: false,
    })
}

#[async_trait]
impl crate::providers::Provider for AntigravityProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Antigravity
    }

    async fn fetch(&self) -> Result<ProviderResult> {
        let auth = self.auth.resolve().await?;
        let key = auth.credential.unwrap_token()?.to_string();

        match self.fetch_quota(&key).await {
            Ok(r) => Ok(r),
            Err(e) => Ok(ProviderResult {
                kind: ProviderKind::Antigravity,
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
    use std::path::PathBuf;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn fixture(name: &str) -> serde_json::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/antigravity")
            .join(name);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {}", path.display(), e));
        serde_json::from_str(&raw).expect("parse fixture json")
    }

    #[test]
    fn parses_live_summary_fixture_two_groups_each_with_5h_and_7d() {
        // Captured 2026-07-14 via Antigravity (agy) OAuth against
        // cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary.
        // Live shape: 2 groups × (weekly + 5h) fraction-only buckets.
        let body = fixture("retrieve_user_quota_summary_live.json");
        let quota = parse_quota(&body).unwrap();

        assert_eq!(quota.plan_name, "Antigravity");
        assert_eq!(quota.windows.len(), 4, "2 groups × 2 windows");

        let types: Vec<&str> = quota.windows.iter().map(|w| w.window_type.as_str()).collect();
        assert!(types.contains(&"7d/gemini"), "got {types:?}");
        assert!(types.contains(&"5h/gemini"), "got {types:?}");
        assert!(types.contains(&"7d/3p"), "got {types:?}");
        assert!(types.contains(&"5h/3p"), "got {types:?}");

        let gemini_weekly = quota
            .windows
            .iter()
            .find(|w| w.window_type == "7d/gemini")
            .unwrap();
        // remainingFraction 0.8187191 → ~82% remaining, 18% used
        assert_eq!(gemini_weekly.limit, 100);
        assert_eq!(gemini_weekly.remaining, 82);
        assert_eq!(gemini_weekly.used, 18);
        assert_eq!(gemini_weekly.period_seconds, Some(7 * 86400));
        assert!(gemini_weekly.reset_at.is_some());

        let gemini_5h = quota
            .windows
            .iter()
            .find(|w| w.window_type == "5h/gemini")
            .unwrap();
        // remainingFraction 0.9798726 → 98% remaining
        assert_eq!(gemini_5h.limit, 100);
        assert_eq!(gemini_5h.remaining, 98);
        assert_eq!(gemini_5h.used, 2);
        assert_eq!(gemini_5h.period_seconds, Some(5 * 3600));

        let third_party_weekly = quota
            .windows
            .iter()
            .find(|w| w.window_type == "7d/3p")
            .unwrap();
        assert_eq!(third_party_weekly.remaining, 100);
        assert_eq!(third_party_weekly.used, 0);
    }

    #[test]
    fn parses_legacy_per_model_fixture() {
        // Older retrieveUserQuota shape (per-model WTUS buckets). Still
        // accepted so cached entries rehydrate.
        let body = fixture("retrieve_user_quota_live.json");
        let quota = parse_quota(&body).unwrap();
        assert_eq!(quota.plan_name, "Antigravity");
        assert!(
            quota.windows.len() >= 4,
            "expected many model windows, got {}",
            quota.windows.len()
        );
        // Model windows keep tokenType_modelId form.
        assert!(
            quota
                .windows
                .iter()
                .any(|w| w.window_type.contains("gemini-2.5-flash")),
            "expected a gemini model window"
        );
    }

    #[test]
    fn parses_empty_groups() {
        let body = serde_json::json!({ "groups": [] });
        let quota = parse_quota(&body).unwrap();
        assert_eq!(quota.plan_name, "Antigravity (no quota data)");
        assert!(quota.windows.is_empty());
    }

    #[test]
    fn fraction_to_counts_clamps_and_rounds() {
        assert_eq!(fraction_to_counts(Some(1.0), None), (0, 100, 100));
        assert_eq!(fraction_to_counts(Some(0.0), None), (100, 100, 0));
        assert_eq!(fraction_to_counts(Some(0.5), None), (50, 100, 50));
        // amount + fraction derives absolute limit
        assert_eq!(
            fraction_to_counts(Some(0.5), Some("500")),
            (500, 1000, 500)
        );
    }

    #[test]
    fn normalize_window_key_maps_aliases() {
        assert_eq!(normalize_window_key("weekly"), "7d");
        assert_eq!(normalize_window_key("5h"), "5h");
        assert_eq!(normalize_window_key("168h"), "7d");
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
    fn resolves_project_from_registry_for_workspace() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("GOOGLE_CLOUD_PROJECT");
        std::env::remove_var("GOOGLE_CLOUD_PROJECT_ID");

        let temp_root = std::env::temp_dir().join(format!(
            "quotas-agy-project-registry-{}",
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

        let temp_root =
            std::env::temp_dir().join(format!("quotas-agy-project-slug-{}", std::process::id()));
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
}
