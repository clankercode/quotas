use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub show_all_windows: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub auto_refresh: AutoRefresh,
    pub statusline: StatusLine,
    pub staleness: StalenessConfig,
    pub ui: Ui,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AutoRefresh {
    pub enabled: bool,
}

impl Default for AutoRefresh {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
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
}
