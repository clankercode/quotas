use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub auto_refresh: AutoRefresh,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AutoRefresh {
    pub enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_refresh: AutoRefresh::default(),
        }
    }
}

impl Default for AutoRefresh {
    fn default() -> Self {
        Self { enabled: true }
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
        match toml::from_str::<Config>(&content) {
            Ok(c) => c,
            Err(_) => Self::default(),
        }
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
