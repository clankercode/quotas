//! Fuzzy matching for multi-provider auth maps (opencode, pi agent, …).
//!
//! Plugin authors use inconsistent key names (`xai-oauth`, `xai`, `grok`,
//! `openai-codex`, `kimi-coding`, …). Exact slot lookups miss common variants,
//! so we score map keys against a provider's alias set and pick the best hit.

use crate::providers::ProviderKind;

/// Normalize a slot key for comparison: lowercase, `_` → `-`.
pub fn normalize_slot_key(key: &str) -> String {
    key.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == '_' { '-' } else { c })
        .collect()
}

/// Strip common plugin suffixes so `xai-oauth` / `kimi-coding-cn` collapse.
fn stem_key(norm: &str) -> &str {
    let mut s = norm;
    // Peel suffixes repeatedly (order matters: longer first).
    for suffix in [
        "-coding-plan",
        "-coding-cn",
        "-for-coding",
        "-coding",
        "-oauth",
        "-api-key",
        "-apikey",
        "-api",
        "-cn",
        "-plan",
    ] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            if !stripped.is_empty() {
                s = stripped;
            }
        }
    }
    s
}

/// Alias tokens that identify a provider in multi-slot auth files.
pub fn provider_slot_aliases(kind: ProviderKind) -> &'static [&'static str] {
    match kind {
        ProviderKind::Claude => &["anthropic", "claude", "claude-code"],
        ProviderKind::Codex => &["openai", "openai-codex", "codex", "chatgpt"],
        ProviderKind::Grok => &["xai", "x-ai", "grok", "xai-oauth", "grok-oauth"],
        ProviderKind::Kimi => &["kimi", "moonshot", "kimi-coding", "kimi-for-coding"],
        ProviderKind::Minimax => &["minimax", "minimax-coding-plan", "minimax-coding"],
        ProviderKind::Zai => &["zai", "zhipu", "glm", "zai-coding", "zai-coding-plan", "zai-coding-cn"],
        // NOTE: plain `opencode` is the OpenCode Zen slot — a different plan
        // with a different key. It must never resolve for OpencodeGo; the
        // strict-prefix guard in score_key_against_alias keeps it out.
        ProviderKind::OpencodeGo => &["opencode-go", "opencodego", "opencode_go"],
        ProviderKind::GitHubCopilot => &["github-copilot", "copilot", "githubcopilot"],
        ProviderKind::OpenRouter => &["openrouter", "open-router"],
        ProviderKind::DeepSeek => &["deepseek", "deep-seek"],
        ProviderKind::SiliconFlow => &["siliconflow", "silicon-flow"],
        ProviderKind::Antigravity => &["antigravity", "gemini", "google", "agy"],
        ProviderKind::Cursor => &["cursor"],
        ProviderKind::Mimo => &["mimo", "xiaomi", "xiaomimimo"],
    }
}

/// Score how well `key` matches `alias` (both already normalized). Higher is better.
fn score_key_against_alias(key_norm: &str, alias_norm: &str) -> i32 {
    if key_norm.is_empty() || alias_norm.is_empty() {
        return 0;
    }
    if key_norm == alias_norm {
        return 1000;
    }
    let key_stem = stem_key(key_norm);
    let alias_stem = stem_key(alias_norm);
    if key_stem == alias_stem {
        return 900;
    }
    // key is alias + suffix (e.g. key=xai-oauth, alias=xai)
    if key_norm.starts_with(alias_norm)
        && key_norm
            .as_bytes()
            .get(alias_norm.len())
            .is_some_and(|b| *b == b'-')
    {
        return 800;
    }
    if key_stem.starts_with(alias_stem)
        && key_stem
            .as_bytes()
            .get(alias_stem.len())
            .is_some_and(|b| *b == b'-')
    {
        return 750;
    }
    // alias contained in key stem or vice versa (avoid tiny accidental hits)
    if alias_stem.len() >= 3 && key_stem.contains(alias_stem) {
        return 500 + alias_stem.len() as i32;
    }
    // Containment must not be a strict prefix: OpenCode Zen's slot is the
    // plain `opencode` key, which sits inside the OpenCode Go alias
    // `opencode-go`. Prefix-only containment stays below the floor so that
    // key (and suffixed forms like `opencode-oauth`, which stem back to
    // `opencode`) never resolves for ProviderKind::OpencodeGo — falling back
    // to the Zen key would hit the wrong plan. Genuine Go variants
    // (`opencodego`, `opencode-go-oauth`) still match via exact/stem/prefix
    // scoring above. The cost is that a key which is a bare prefix of an
    // alias (e.g. `anth` for `anthropic`) no longer fuzzy-matches; no known
    // plugin uses such slot names.
    if key_stem.len() >= 3
        && alias_stem.contains(key_stem)
        && !alias_stem.starts_with(key_stem)
    {
        return 400 + key_stem.len() as i32;
    }
    0
}

/// Best-matching map key for `kind`, or `None` if nothing scores above the floor.
pub fn find_best_slot_key<'a>(
    map_keys: impl IntoIterator<Item = &'a str>,
    kind: ProviderKind,
) -> Option<String> {
    let aliases: Vec<String> = provider_slot_aliases(kind)
        .iter()
        .map(|a| normalize_slot_key(a))
        .collect();

    let mut best: Option<(i32, String)> = None;
    for key in map_keys {
        let key_norm = normalize_slot_key(key);
        if key_norm.is_empty() {
            continue;
        }
        let mut score = 0;
        for alias in &aliases {
            score = score.max(score_key_against_alias(&key_norm, alias));
        }
        // Floor: require a meaningful hit (exact/stem/prefix-ish), not noise.
        if score < 400 {
            continue;
        }
        match &best {
            None => best = Some((score, key.to_string())),
            Some((best_score, best_key)) => {
                if score > *best_score
                    || (score == *best_score && key.len() < best_key.len())
                {
                    best = Some((score, key.to_string()));
                }
            }
        }
    }
    best.map(|(_, k)| k)
}

/// Whether a JSON entry looks like a usable credential (not null/empty).
pub fn entry_has_credential(entry: &serde_json::Value) -> bool {
    if entry.is_null() {
        return false;
    }
    let Some(obj) = entry.as_object() else {
        return false;
    };
    // type: null with no secrets
    if obj.get("type").is_some_and(|t| t.is_null())
        && !obj.contains_key("key")
        && !obj.contains_key("access")
        && !obj.contains_key("access_token")
        && !obj.contains_key("token")
    {
        return false;
    }
    for field in ["key", "access", "access_token", "token", "api_key"] {
        if let Some(v) = obj.get(field).and_then(|x| x.as_str()) {
            if !v.is_empty() {
                return true;
            }
        }
    }
    false
}

/// Extract bearer/token string from a multi-slot auth entry.
pub fn extract_slot_credential(entry: &serde_json::Value) -> Option<String> {
    if !entry_has_credential(entry) {
        return None;
    }
    let obj = entry.as_object()?;
    for field in ["access", "access_token", "key", "token", "api_key"] {
        if let Some(v) = obj.get(field).and_then(|x| x.as_str()) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_xai_oauth_for_grok() {
        let keys = ["minimax", "openai-codex", "xai-oauth", "kimi-coding"];
        assert_eq!(
            find_best_slot_key(keys, ProviderKind::Grok).as_deref(),
            Some("xai-oauth")
        );
    }

    #[test]
    fn matches_openai_codex_for_codex() {
        let keys = ["openai-codex", "anthropic", "xai"];
        assert_eq!(
            find_best_slot_key(keys, ProviderKind::Codex).as_deref(),
            Some("openai-codex")
        );
    }

    #[test]
    fn matches_zai_coding_cn() {
        let keys = ["zai-coding-cn", "minimax"];
        assert_eq!(
            find_best_slot_key(keys, ProviderKind::Zai).as_deref(),
            Some("zai-coding-cn")
        );
    }

    #[test]
    fn prefers_exact_over_fuzzy() {
        let keys = ["xai", "xai-oauth-plugin-extra"];
        assert_eq!(
            find_best_slot_key(keys, ProviderKind::Grok).as_deref(),
            Some("xai")
        );
    }

    #[test]
    fn no_match_returns_none() {
        let keys = ["totally-unrelated", "foo"];
        assert!(find_best_slot_key(keys, ProviderKind::Grok).is_none());
    }

    #[test]
    fn opencode_go_uses_go_slot_not_zen() {
        // Both Zen (`opencode`) and Go (`opencode-go`) slots present: pick Go.
        assert_eq!(
            find_best_slot_key(["opencode", "opencode-go"], ProviderKind::OpencodeGo).as_deref(),
            Some("opencode-go")
        );
        // Zen slot alone is a different plan — never claim it for Go.
        assert!(find_best_slot_key(["opencode"], ProviderKind::OpencodeGo).is_none());
        // Suffixed Zen variants stem back to `opencode` and must stay excluded.
        assert!(find_best_slot_key(
            ["opencode-oauth", "opencode-api-key", "opencode-plan", "opencode_cn"],
            ProviderKind::OpencodeGo
        )
        .is_none());
        // Suffixed Go variants still resolve.
        assert_eq!(
            find_best_slot_key(["opencode_go_oauth"], ProviderKind::OpencodeGo).as_deref(),
            Some("opencode_go_oauth")
        );
    }

    #[test]
    fn prefix_guard_keeps_existing_full_name_matches() {
        // The strict-prefix guard must not disturb real plugin slot names.
        for (keys, kind, want) in [
            (["anthropic"], ProviderKind::Claude, "anthropic"),
            (["claude-code"], ProviderKind::Claude, "claude-code"),
            (["zhipu"], ProviderKind::Zai, "zhipu"),
            (["moonshot"], ProviderKind::Kimi, "moonshot"),
            (["xiaomi"], ProviderKind::Mimo, "xiaomi"),
            (["google"], ProviderKind::Antigravity, "google"),
            (["copilot"], ProviderKind::GitHubCopilot, "copilot"),
            (["minimax-coding-plan"], ProviderKind::Minimax, "minimax-coding-plan"),
        ] {
            assert_eq!(
                find_best_slot_key(keys, kind).as_deref(),
                Some(want),
                "{kind:?} should still match {want}"
            );
        }
    }

    #[test]
    fn entry_null_type_without_secrets_is_empty() {
        let entry = json!({ "type": null });
        assert!(!entry_has_credential(&entry));
        assert!(extract_slot_credential(&entry).is_none());
    }

    #[test]
    fn extracts_oauth_access() {
        let entry = json!({
            "type": "oauth",
            "access": "tok-abc",
            "refresh": "r"
        });
        assert_eq!(extract_slot_credential(&entry).as_deref(), Some("tok-abc"));
    }

    #[test]
    fn extracts_api_key() {
        let entry = json!({ "type": "api_key", "key": "sk-xyz" });
        assert_eq!(extract_slot_credential(&entry).as_deref(), Some("sk-xyz"));
    }
}
