use crate::providers::ProviderKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct StalenessConfig {
    /// Default staleness threshold in seconds (default: 300 = 5 minutes).
    pub default_secs: u64,
    /// Per-provider overrides: provider name -> staleness threshold in seconds.
    #[serde(rename = "providers")]
    pub provider_overrides: BTreeMap<String, u64>,
}

impl StalenessConfig {
    /// Returns the staleness threshold for a given provider slug.
    pub fn staleness_threshold(&self, provider: &str) -> u64 {
        self.provider_overrides
            .get(provider)
            .copied()
            .unwrap_or(self.default_secs)
    }
}

impl Default for StalenessConfig {
    fn default() -> Self {
        Self {
            default_secs: 300,
            provider_overrides: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Ui {
    pub show_all_windows: bool,
    /// Allow cards to span multiple rows (e.g. MiniMax as 2×2). Default: false.
    pub vertical_spanning: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct GitHubCopilotConfig {
    pub token: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub auto_refresh: AutoRefresh,
    pub favorites: FavoritesConfig,
    pub quota_preferences: BTreeMap<String, QuotaPreferences>,
    pub statusline: StatusLine,
    pub staleness: StalenessConfig,
    pub providers: Providers,
    pub tui: TuiConfig,
    pub ui: Ui,
    pub github_copilot: GitHubCopilotConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FavoritesConfig {
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct QuotaPreferences {
    pub favorites: Vec<String>,
    pub hidden: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Providers {
    /// Wildcard `"*"` enables all providers except disabled.
    /// Specific list acts as a whitelist.
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
}

impl Default for Providers {
    fn default() -> Self {
        Self {
            enabled: vec!["*".into()],
            disabled: Vec::new(),
        }
    }
}

impl Config {
    /// Returns which ProviderKinds are enabled per the providers config.
    /// Whitelist mode: only the named providers.
    /// Wildcard mode: all except disabled.
    pub fn providers_enabled_kinds(&self) -> Vec<ProviderKind> {
        use crate::providers::ProviderKind;
        let all = ProviderKind::all();
        // Normalize config values once for comparison
        let disabled_lower: Vec<String> =
            self.providers.disabled.iter().map(|s| s.to_lowercase()).collect();
        if self.providers.enabled.iter().any(|e| e == "*") {
            all.iter()
                .filter(|k| {
                    let slug = k.slug();
                    !disabled_lower.iter().any(|d| d == slug)
                })
                .copied()
                .collect()
        } else {
            let enabled_lower: Vec<String> =
                self.providers.enabled.iter().map(|s| s.to_lowercase()).collect();
            all.iter()
                .filter(|k| enabled_lower.iter().any(|e| e == k.slug()))
                .copied()
                .collect()
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TuiConfig {
    pub auto_refresh: bool,
    pub refresh_on_start: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            auto_refresh: true,
            refresh_on_start: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AutoRefresh {
    pub enabled: bool,
}

impl Default for AutoRefresh {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct StatusLine {
    pub icons: bool,
    pub bg_refresh: bool,
}

impl Default for StatusLine {
    fn default() -> Self {
        Self {
            icons: true,
            bg_refresh: true,
        }
    }
}

impl Config {
    pub fn config_path() -> Option<PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Some(PathBuf::from(xdg).join("quotas/config.toml"));
            }
        }
        let home = dirs::home_dir()?;
        Some(home.join(".config/quotas/config.toml"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str::<Config>(&content).unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let Some(path) = Self::config_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(path, content)
    }

    pub fn is_provider_favorited(&self, provider: &str) -> bool {
        contains_case_insensitive(&self.favorites.providers, provider)
    }

    pub fn toggle_provider_favorite(&mut self, provider: &str) {
        toggle_value(&mut self.favorites.providers, provider);
    }

    pub fn quota_preferences_for(&self, provider: &str) -> QuotaPreferences {
        self.quota_preferences
            .get(&provider.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    pub fn is_quota_favorited(&self, provider: &str, quota_key: &str) -> bool {
        self.quota_preferences
            .get(&provider.to_ascii_lowercase())
            .is_some_and(|prefs| contains_case_insensitive(&prefs.favorites, quota_key))
    }

    pub fn is_quota_hidden(&self, provider: &str, quota_key: &str) -> bool {
        self.quota_preferences
            .get(&provider.to_ascii_lowercase())
            .is_some_and(|prefs| contains_case_insensitive(&prefs.hidden, quota_key))
    }

    pub fn toggle_quota_favorite(&mut self, provider: &str, quota_key: &str) {
        let prefs = self
            .quota_preferences
            .entry(provider.to_ascii_lowercase())
            .or_default();
        toggle_value(&mut prefs.favorites, quota_key);
        if prefs.favorites.is_empty() && prefs.hidden.is_empty() {
            self.quota_preferences.remove(&provider.to_ascii_lowercase());
        }
    }

    pub fn toggle_quota_hidden(&mut self, provider: &str, quota_key: &str) {
        let prefs = self
            .quota_preferences
            .entry(provider.to_ascii_lowercase())
            .or_default();
        toggle_value(&mut prefs.hidden, quota_key);
        if prefs.favorites.is_empty() && prefs.hidden.is_empty() {
            self.quota_preferences.remove(&provider.to_ascii_lowercase());
        }
    }
}

fn contains_case_insensitive(values: &[String], needle: &str) -> bool {
    values.iter().any(|value| value.eq_ignore_ascii_case(needle))
}

fn toggle_value(values: &mut Vec<String>, needle: &str) {
    if let Some(idx) = values
        .iter()
        .position(|value| value.eq_ignore_ascii_case(needle))
    {
        values.remove(idx);
    } else {
        values.push(needle.to_string());
        values.sort_by_key(|value| value.to_ascii_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_enables_auto_refresh() {
        let c = Config::default();
        assert!(c.auto_refresh.enabled);
    }

    #[test]
    fn parses_disable_auto_refresh() {
        let toml_str = "[auto_refresh]\nenabled = false\n";
        let c: Config = toml::from_str(toml_str).unwrap();
        assert!(!c.auto_refresh.enabled);
    }

    #[test]
    fn parses_empty_file_uses_defaults() {
        let c: Config = toml::from_str("").unwrap();
        assert!(c.auto_refresh.enabled);
    }

    #[test]
    fn default_enables_tui_refresh_controls() {
        let c = Config::default();
        assert!(c.tui.auto_refresh);
        assert!(c.tui.refresh_on_start);
    }

    #[test]
    fn parses_disable_tui_refresh_controls() {
        let toml_str = "[tui]\nauto_refresh = false\nrefresh_on_start = false\n";
        let c: Config = toml::from_str(toml_str).unwrap();
        assert!(!c.tui.auto_refresh);
        assert!(!c.tui.refresh_on_start);
    }

    #[test]
    fn providers_default_is_wildcard() {
        let c = Config::default();
        assert_eq!(c.providers.enabled, vec!["*"]);
        assert!(c.providers.disabled.is_empty());
    }

    #[test]
    fn providers_wildcard_with_disabled() {
        let toml_str = r#"
[providers]
enabled = ["*"]
disabled = ["gemini", "kimi"]
"#;
        let c: Config = toml::from_str(toml_str).unwrap();
        let enabled = c.providers_enabled_kinds();
        assert!(!enabled.contains(&ProviderKind::Gemini));
        assert!(!enabled.contains(&ProviderKind::Kimi));
        assert!(enabled.contains(&ProviderKind::Claude));
    }

    #[test]
    fn providers_whitelist_mode() {
        let toml_str = r#"
[providers]
enabled = ["claude", "kimi"]
"#;
        let c: Config = toml::from_str(toml_str).unwrap();
        let enabled = c.providers_enabled_kinds();
        assert_eq!(enabled.len(), 2);
        assert!(enabled.contains(&ProviderKind::Claude));
        assert!(enabled.contains(&ProviderKind::Kimi));
        assert!(!enabled.contains(&ProviderKind::DeepSeek));
    }

    #[test]
    fn parses_persisted_provider_and_quota_preferences() {
        let toml_str = r#"
[favorites]
providers = ["codex", "claude"]

[quota_preferences.codex]
favorites = ["5h", "spark/7d"]
hidden = ["o3/weekly"]
"#;
        let c: Config = toml::from_str(toml_str).unwrap();

        assert!(c.is_provider_favorited("codex"));
        assert!(c.is_provider_favorited("claude"));
        assert!(c.is_quota_favorited("codex", "spark/7d"));
        assert!(c.is_quota_hidden("codex", "o3/weekly"));
    }

    #[test]
    fn serializes_preferences_round_trip() {
        let mut c = Config::default();
        c.toggle_provider_favorite("codex");
        c.toggle_quota_favorite("codex", "7d");
        c.toggle_quota_hidden("codex", "spark/7d");

        let encoded = toml::to_string(&c).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();

        assert!(decoded.is_provider_favorited("codex"));
        assert!(decoded.is_quota_favorited("codex", "7d"));
        assert!(decoded.is_quota_hidden("codex", "spark/7d"));
    }
}
