use crate::Result;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

pub const UPDATE_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const CRATES_IO_URL: &str = "https://crates.io/api/v1/crates/quotas";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub checked_at: DateTime<Utc>,
}

impl UpdateInfo {
    pub fn is_update_available(&self) -> bool {
        is_newer_version(&self.latest_version, &self.current_version)
    }

    pub fn summary(&self) -> String {
        format!(
            "quotas {} available (current {})",
            self.latest_version, self.current_version
        )
    }

    /// GitHub release page for the latest version (opens from the TUI footer).
    pub fn release_page_url(&self) -> String {
        release_page_url(&self.latest_version)
    }
}

/// URL of the GitHub release page for `version` (with or without a leading `v`).
pub fn release_page_url(version: &str) -> String {
    let repo = env!("CARGO_PKG_REPOSITORY").trim_end_matches('/');
    let v = version.trim_start_matches('v');
    format!("{repo}/releases/tag/v{v}")
}

/// Open a URL in the user's default browser. Best-effort — failures are silent
/// so a missing `xdg-open`/`open` never breaks the TUI event loop.
pub fn open_url(url: &str) {
    let _ = open_url_impl(url);
}

fn open_url_impl(url: &str) -> std::io::Result<std::process::Child> {
    if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else if cfg!(target_os = "windows") {
        // Empty title arg after `start` keeps URLs with `&` from being parsed
        // as extra commands.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
    } else {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .or_else(|_| {
                std::process::Command::new("gio")
                    .args(["open", url])
                    .spawn()
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateCache {
    pub latest_version: String,
    pub checked_at: DateTime<Utc>,
}

impl UpdateCache {
    pub fn to_info(&self, current_version: &str) -> UpdateInfo {
        UpdateInfo {
            current_version: current_version.to_string(),
            latest_version: self.latest_version.clone(),
            checked_at: self.checked_at,
        }
    }

    pub fn is_stale_at(&self, now: DateTime<Utc>, ttl: Duration) -> bool {
        let age = now.signed_duration_since(self.checked_at);
        age.num_seconds() < 0 || age.num_seconds() as u64 >= ttl.as_secs()
    }
}

pub fn cache_path() -> Option<PathBuf> {
    let quota_cache = crate::cache::cache_path()?;
    Some(quota_cache.with_file_name("update.json"))
}

pub fn read_cache() -> Option<UpdateCache> {
    let path = cache_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn write_cache(cache: &UpdateCache) {
    let Some(path) = cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(cache) {
        let _ = std::fs::write(path, json);
    }
}

pub fn cached_update_info(current_version: &str) -> Option<UpdateInfo> {
    let info = read_cache()?.to_info(current_version);
    info.is_update_available().then_some(info)
}

pub fn should_check_for_update(now: DateTime<Utc>) -> bool {
    match read_cache() {
        Some(cache) => cache.is_stale_at(now, UPDATE_CACHE_TTL),
        None => true,
    }
}

pub async fn fetch_latest_version(client: &Client) -> Result<String> {
    let body: serde_json::Value = client
        .get(CRATES_IO_URL)
        .header("User-Agent", "quotas-cli update-check")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    parse_latest_version(&body)
        .ok_or_else(|| crate::Error::Provider("crates.io response missing max_version".into()))
}

pub async fn refresh_update_cache(current_version: &str) -> Result<UpdateInfo> {
    let client = Client::new();
    let latest_version = fetch_latest_version(&client).await?;
    let cache = UpdateCache {
        latest_version: latest_version.clone(),
        checked_at: Utc::now(),
    };
    write_cache(&cache);
    Ok(cache.to_info(current_version))
}

pub fn is_newer_version(latest: &str, current: &str) -> bool {
    let Some(latest) = parse_stable_version(latest) else {
        return false;
    };
    let Some(current) = parse_stable_version(current) else {
        return false;
    };
    latest > current
}

pub fn parse_latest_version(body: &serde_json::Value) -> Option<String> {
    body.get("crate")?
        .get("max_version")?
        .as_str()
        .map(str::to_string)
}

fn parse_stable_version(version: &str) -> Option<Vec<u64>> {
    if version.contains('-') {
        return None;
    }
    let mut parts = Vec::new();
    for part in version.split('.') {
        parts.push(part.parse::<u64>().ok()?);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_like_versions() {
        assert!(is_newer_version("0.8.2", "0.8.1"));
        assert!(is_newer_version("0.9.0", "0.8.9"));
        assert!(is_newer_version("1.0.0", "0.99.99"));
        assert!(!is_newer_version("0.8.1", "0.8.1"));
        assert!(!is_newer_version("0.8.1", "0.8.2"));
    }

    #[test]
    fn ignores_prerelease_as_update_for_stable_current() {
        assert!(!is_newer_version("0.8.2-alpha.1", "0.8.1"));
    }

    #[test]
    fn parses_crates_io_latest_version() {
        let body = serde_json::json!({
            "crate": {"max_version": "0.8.2"},
            "versions": []
        });

        assert_eq!(parse_latest_version(&body).as_deref(), Some("0.8.2"));
    }

    #[test]
    fn release_page_url_uses_package_repository() {
        let url = release_page_url("0.8.2");
        assert!(
            url.ends_with("/releases/tag/v0.8.2"),
            "unexpected release url: {url}"
        );
        assert!(
            url.starts_with("https://github.com/"),
            "expected github host: {url}"
        );
        // Leading v should not be doubled.
        assert_eq!(release_page_url("v0.8.2"), release_page_url("0.8.2"));
    }

    #[test]
    fn stale_when_ttl_elapsed() {
        let now = Utc::now();
        let cache = UpdateCache {
            latest_version: "0.8.1".into(),
            checked_at: now - chrono::Duration::hours(25),
        };

        assert!(cache.is_stale_at(now, UPDATE_CACHE_TTL));
    }

    #[test]
    fn fresh_before_ttl_elapsed() {
        let now = Utc::now();
        let cache = UpdateCache {
            latest_version: "0.8.1".into(),
            checked_at: now - chrono::Duration::hours(23),
        };

        assert!(!cache.is_stale_at(now, UPDATE_CACHE_TTL));
    }
}
