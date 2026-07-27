//! Home-directory multi-profile discovery for Claude / Codex / Grok.
//!
//! Detects `~/.{tool}/` and `~/.{tool}-{suffix}/`, follows symlinks, and
//! deduplicates by the **canonical path of the auth file** (same resolved
//! place = same profile).

use crate::providers::ProviderKind;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// A tool family that stores credentials under a home profile directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolFamily {
    pub kind: ProviderKind,
    /// Directory base without leading dot, e.g. `"grok"`.
    pub base: &'static str,
    /// Auth file relative to the profile dir.
    pub auth_relpath: &'static str,
    /// Optional env var that points at the profile dir (not the auth file).
    pub env_home: Option<&'static str>,
}

pub const TOOL_FAMILIES: &[ToolFamily] = &[
    ToolFamily {
        kind: ProviderKind::Claude,
        base: "claude",
        auth_relpath: ".credentials.json",
        env_home: Some("CLAUDE_CONFIG_DIR"),
    },
    ToolFamily {
        kind: ProviderKind::Codex,
        base: "codex",
        auth_relpath: "auth.json",
        env_home: None,
    },
    ToolFamily {
        kind: ProviderKind::Grok,
        base: "grok",
        auth_relpath: "auth.json",
        env_home: Some("GROK_HOME"),
    },
];

/// One unique profile after symlink / auth-path dedupe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProfile {
    pub kind: ProviderKind,
    /// Stable id from canonical dir basename (`grok-1`, `claude-p`, `default`).
    pub profile_id: String,
    pub profile_dir: PathBuf,
    pub auth_path: PathBuf,
    /// True when `~/.{base}` (or env home) resolves to this profile.
    pub is_active_default: bool,
}

/// Resolved instance ready for fetch / cache / TUI.
#[derive(Debug, Clone)]
pub struct ProviderInstance {
    pub kind: ProviderKind,
    pub profile_id: String,
    /// Cache / favorites key (`grok` or `grok:grok-1`).
    pub instance_key: String,
    /// Card title (`Grok` or `Grok (grok-1)` or config override).
    pub display_name: String,
    pub auth_path: Option<PathBuf>,
    pub profile_dir: Option<PathBuf>,
    pub is_active_default: bool,
}

impl ProviderInstance {
    pub fn single(kind: ProviderKind) -> Self {
        Self {
            kind,
            profile_id: "default".into(),
            instance_key: kind.slug().into(),
            display_name: kind.display_name().into(),
            auth_path: None,
            profile_dir: None,
            is_active_default: true,
        }
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

fn try_canonicalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Whether `name` is `.{base}` or `.{base}-*`.
fn matches_family_dir(name: &str, base: &str) -> bool {
    let expected = format!(".{base}");
    if name == expected {
        return true;
    }
    let prefix = format!(".{base}-");
    name.starts_with(&prefix) && name.len() > prefix.len()
}

fn profile_id_for(canonical_dir: &Path, base: &str) -> String {
    let name = canonical_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("default");
    let default_name = format!(".{base}");
    if name == default_name {
        "default".into()
    } else if let Some(rest) = name.strip_prefix(&format!(".{base}-")) {
        if rest.is_empty() {
            "default".into()
        } else {
            // Keep full basename without leading dot for uniqueness: grok-1
            format!("{base}-{rest}")
        }
    } else if let Some(rest) = name.strip_prefix('.') {
        rest.to_string()
    } else {
        name.to_string()
    }
}

/// Discover unique profiles for one tool family under `home`.
pub fn discover_family_profiles(home: &Path, family: ToolFamily) -> Vec<DiscoveredProfile> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(env_name) = family.env_home {
        if let Ok(val) = std::env::var(env_name) {
            if !val.is_empty() {
                candidates.push(PathBuf::from(val));
            }
        }
    }
    candidates.push(home.join(format!(".{}", family.base)));

    if let Ok(rd) = std::fs::read_dir(home) {
        for entry in rd.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if matches_family_dir(name_str, family.base) {
                candidates.push(entry.path());
            }
        }
    }

    // Active default target: canonicalize ~/.{base} (and env) for is_active flag.
    let active_targets: HashSet<PathBuf> = {
        let mut set = HashSet::new();
        let default_dir = home.join(format!(".{}", family.base));
        if default_dir.exists() {
            set.insert(try_canonicalize(&default_dir));
        }
        if let Some(env_name) = family.env_home {
            if let Ok(val) = std::env::var(env_name) {
                if !val.is_empty() {
                    let p = PathBuf::from(val);
                    if p.exists() {
                        set.insert(try_canonicalize(&p));
                    }
                }
            }
        }
        set
    };

    // Dedupe by canonical auth path.
    let mut by_auth: BTreeMap<PathBuf, DiscoveredProfile> = BTreeMap::new();
    let mut seen_candidate_dirs: HashSet<PathBuf> = HashSet::new();

    for cand in candidates {
        if !cand.exists() {
            continue;
        }
        let profile_dir = try_canonicalize(&cand);
        if !seen_candidate_dirs.insert(profile_dir.clone()) {
            continue;
        }
        let auth_path = profile_dir.join(family.auth_relpath);
        if !auth_path.is_file() && !auth_path.is_symlink() {
            // Missing auth → shared resource pack, etc.
            continue;
        }
        // Prefer realpath of the auth file; if the file itself is a symlink, follow it.
        let auth_canon = try_canonicalize(&auth_path);
        if !auth_canon.exists() {
            continue;
        }

        let is_active = active_targets.contains(&profile_dir);
        let profile_id = profile_id_for(&profile_dir, family.base);

        by_auth
            .entry(auth_canon.clone())
            .and_modify(|existing| {
                // Prefer active default, else shorter/more canonical id.
                if is_active && !existing.is_active_default {
                    existing.profile_dir = profile_dir.clone();
                    existing.profile_id = profile_id.clone();
                    existing.is_active_default = true;
                }
            })
            .or_insert(DiscoveredProfile {
                kind: family.kind,
                profile_id,
                profile_dir,
                auth_path: auth_canon,
                is_active_default: is_active,
            });
    }

    let mut out: Vec<DiscoveredProfile> = by_auth.into_values().collect();
    out.sort_by(|a, b| {
        // Active first, then profile_id.
        b.is_active_default
            .cmp(&a.is_active_default)
            .then_with(|| a.profile_id.cmp(&b.profile_id))
    });
    out
}

/// Config overlay for a single profile entry.
#[derive(Debug, Clone, Default)]
pub struct ProfileConfigOverlay {
    pub name: Option<String>,
    pub canonical: Option<PathBuf>,
    pub auth: Option<PathBuf>,
    pub enabled: bool,
}

/// Build instances for multi-profile families, applying config overlays.
///
/// `overlays` keys may be `kind:profile_id`, bare `profile_id`, or paths.
pub fn build_profile_instances(
    home: &Path,
    overlays: &BTreeMap<String, ProfileConfigOverlay>,
    autodetect: bool,
) -> Vec<ProviderInstance> {
    let mut instances = Vec::new();

    for family in TOOL_FAMILIES {
        let mut profiles = if autodetect {
            discover_family_profiles(home, *family)
        } else {
            Vec::new()
        };

        // Manual-only overlays that reference this kind and a path not yet found.
        for (key, overlay) in overlays {
            if !overlay.enabled {
                continue;
            }
            let key_kind = key
                .split_once(':')
                .map(|(k, _)| k)
                .unwrap_or("")
                .to_ascii_lowercase();
            let targets_kind = key_kind == family.kind.slug()
                || key_kind == family.base
                || key.starts_with(&format!("{}:", family.kind.slug()));
            if !targets_kind && overlay.canonical.is_none() && overlay.auth.is_none() {
                continue;
            }
            let Some(path) = overlay
                .auth
                .clone()
                .or_else(|| {
                    overlay.canonical.as_ref().map(|d| d.join(family.auth_relpath))
                })
            else {
                continue;
            };
            let auth_canon = try_canonicalize(&path);
            if profiles.iter().any(|p| p.auth_path == auth_canon) {
                continue;
            }
            if !auth_canon.exists() {
                continue;
            }
            let profile_dir = overlay
                .canonical
                .clone()
                .unwrap_or_else(|| {
                    auth_canon
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| auth_canon.clone())
                });
            let profile_dir = try_canonicalize(&profile_dir);
            let profile_id = key
                .split_once(':')
                .map(|(_, id)| id.to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| profile_id_for(&profile_dir, family.base));
            profiles.push(DiscoveredProfile {
                kind: family.kind,
                profile_id,
                profile_dir,
                auth_path: auth_canon,
                is_active_default: false,
            });
        }

        // Apply disable + rename from overlays.
        let multi = profiles.len() > 1;
        for mut p in profiles {
            let overlay = lookup_overlay(overlays, family.kind, &p);
            if let Some(o) = overlay {
                if !o.enabled {
                    continue;
                }
                if let Some(auth) = &o.auth {
                    p.auth_path = try_canonicalize(auth);
                } else if let Some(canon) = &o.canonical {
                    let dir = try_canonicalize(canon);
                    p.profile_dir = dir.clone();
                    p.auth_path = try_canonicalize(&dir.join(family.auth_relpath));
                }
            }

            let custom_name = overlay.and_then(|o| o.name.clone());
            let display_name = custom_name.unwrap_or_else(|| {
                if multi {
                    format!("{} ({})", family.kind.display_name(), p.profile_id)
                } else {
                    family.kind.display_name().to_string()
                }
            });
            let instance_key = if multi {
                format!("{}:{}", family.kind.slug(), p.profile_id)
            } else {
                family.kind.slug().to_string()
            };

            instances.push(ProviderInstance {
                kind: family.kind,
                profile_id: p.profile_id,
                instance_key,
                display_name,
                auth_path: Some(p.auth_path),
                profile_dir: Some(p.profile_dir),
                is_active_default: p.is_active_default,
            });
        }
    }

    instances
}

fn lookup_overlay<'a>(
    overlays: &'a BTreeMap<String, ProfileConfigOverlay>,
    kind: ProviderKind,
    profile: &DiscoveredProfile,
) -> Option<&'a ProfileConfigOverlay> {
    let candidates = [
        format!("{}:{}", kind.slug(), profile.profile_id),
        profile.profile_id.clone(),
        profile.profile_dir.to_string_lossy().to_string(),
        profile.auth_path.to_string_lossy().to_string(),
    ];
    for c in candidates {
        if let Some(o) = overlays.get(&c) {
            return Some(o);
        }
        // Also try with ~/ form if home-relative
        if let Some(home) = dirs::home_dir() {
            if let Ok(rel) = profile.profile_dir.strip_prefix(&home) {
                let tilde = format!("~/{}", rel.display());
                if let Some(o) = overlays.get(&format!("{}:{}", kind.slug(), tilde)) {
                    return Some(o);
                }
                if let Some(o) = overlays.get(&tilde) {
                    return Some(o);
                }
            }
        }
    }
    None
}

/// Expand enabled provider kinds into concrete instances (multi-profile + singles).
pub fn expand_instances(
    kinds: &[ProviderKind],
    home: &Path,
    overlays: &BTreeMap<String, ProfileConfigOverlay>,
    autodetect: bool,
) -> Vec<ProviderInstance> {
    let multi_profile_kinds: HashSet<ProviderKind> =
        TOOL_FAMILIES.iter().map(|f| f.kind).collect();
    let profile_instances = build_profile_instances(home, overlays, autodetect);

    let mut out = Vec::new();
    for &kind in kinds {
        if multi_profile_kinds.contains(&kind) {
            let for_kind: Vec<_> = profile_instances
                .iter()
                .filter(|i| i.kind == kind)
                .cloned()
                .collect();
            if for_kind.is_empty() {
                // Still emit a single placeholder so auth resolvers (env/pi) can run.
                out.push(ProviderInstance::single(kind));
            } else {
                out.extend(for_kind);
            }
        } else {
            out.push(ProviderInstance::single(kind));
        }
    }
    out
}

/// Parse path strings from config (`~/…` supported).
pub fn parse_config_path(s: &str) -> PathBuf {
    expand_tilde(s.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmp_home(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "quotas_profiles_{}_{}",
            label,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_auth(dir: &Path, rel: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(rel), body).unwrap();
    }

    #[test]
    fn matches_family_dir_patterns() {
        assert!(matches_family_dir(".grok", "grok"));
        assert!(matches_family_dir(".grok-1", "grok"));
        assert!(matches_family_dir(".claude-w", "claude"));
        assert!(!matches_family_dir(".claude.backup", "claude"));
        assert!(!matches_family_dir(".grokshared", "grok"));
        assert!(!matches_family_dir(".codex-shared-extra", "claude"));
    }

    #[test]
    fn discovers_and_dedupes_symlinks() {
        let home = tmp_home("dedupe");
        write_auth(
            &home.join(".grok-2"),
            "auth.json",
            r#"{"https://auth.x.ai::a":{"key":"tok-2"}}"#,
        );
        write_auth(
            &home.join(".grok-1"),
            "auth.json",
            r#"{"https://auth.x.ai::a":{"key":"tok-1"}}"#,
        );
        symlink(home.join(".grok-2"), home.join(".grok")).unwrap();

        let family = TOOL_FAMILIES
            .iter()
            .find(|f| f.base == "grok")
            .copied()
            .unwrap();
        let profiles = discover_family_profiles(&home, family);
        assert_eq!(profiles.len(), 2, "{profiles:?}");
        let ids: Vec<_> = profiles.iter().map(|p| p.profile_id.as_str()).collect();
        assert!(ids.contains(&"grok-1"));
        assert!(ids.contains(&"grok-2"));
        assert_eq!(profiles.iter().filter(|p| p.is_active_default).count(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn skips_shared_dirs_without_auth() {
        let home = tmp_home("shared");
        std::fs::create_dir_all(home.join(".codex-shared")).unwrap();
        write_auth(
            &home.join(".codex-w"),
            "auth.json",
            r#"{"tokens":{"access_token":"a"}}"#,
        );
        let family = TOOL_FAMILIES
            .iter()
            .find(|f| f.base == "codex")
            .copied()
            .unwrap();
        let profiles = discover_family_profiles(&home, family);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].profile_id, "codex-w");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn multi_instance_keys_disambiguate() {
        let home = tmp_home("keys");
        write_auth(&home.join(".claude-w"), ".credentials.json", r#"{}"#);
        write_auth(&home.join(".claude-p"), ".credentials.json", r#"{}"#);
        // Empty JSON still counts as auth file present for discovery;
        // real auth parse is separate.
        let overlays = BTreeMap::new();
        let instances = build_profile_instances(&home, &overlays, true);
        let claude: Vec<_> = instances
            .iter()
            .filter(|i| i.kind == ProviderKind::Claude)
            .collect();
        assert_eq!(claude.len(), 2);
        assert!(claude.iter().all(|i| i.instance_key.starts_with("claude:")));
        let _ = std::fs::remove_dir_all(&home);
    }
}
