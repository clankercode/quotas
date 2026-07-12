use crate::{Error, Result};
use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::{AuthCredential, AuthResolver, ResolvedAuth};

/// Resolves a Cursor dashboard session cookie (`WorkosCursorSessionToken`)
/// from whichever local Cursor credential we can find:
///
/// 1. **cursor-agent CLI** — `~/.config/cursor/auth.json` (`accessToken`).
/// 2. **Cursor IDE** — the `cursorAuth/accessToken` entry in the IDE's SQLite
///    globalStorage DB (`~/.config/Cursor/User/globalStorage/state.vscdb`).
///
/// Both store a WorkOS JWT. The cookie the dashboard API expects is
/// `"{userId}::{jwt}"` (with `::` URL-encoded as `%3A%3A`), where `userId` is
/// the WorkOS user id embedded in the JWT's `sub` claim — so we don't need any
/// separate CLI config file to supply it.
pub struct CursorAuthResolver {
    agent_auth_json: PathBuf,
    ide_vscdb: PathBuf,
}

impl CursorAuthResolver {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Self {
            agent_auth_json: home.join(".config/cursor/auth.json"),
            ide_vscdb: home.join(".config/Cursor/User/globalStorage/state.vscdb"),
        }
    }

    #[cfg(test)]
    fn with_paths(agent_auth_json: PathBuf, ide_vscdb: PathBuf) -> Self {
        Self {
            agent_auth_json,
            ide_vscdb,
        }
    }

    /// Find a Cursor access token (WorkOS JWT) and a human-readable source
    /// label, preferring the cursor-agent CLI file, then the IDE DB.
    fn access_token(&self) -> Result<(String, String)> {
        if let Ok(content) = std::fs::read_to_string(&self.agent_auth_json) {
            if let Ok(auth) = serde_json::from_str::<AgentAuthJson>(&content) {
                if !auth.access_token.is_empty() {
                    return Ok((
                        auth.access_token,
                        format!("file:{}", self.agent_auth_json.display()),
                    ));
                }
            }
        }
        if self.ide_vscdb.exists() {
            if let Some(token) = read_ide_access_token(&self.ide_vscdb)? {
                return Ok((token, format!("cursor-ide:{}", self.ide_vscdb.display())));
            }
        }
        Err(Error::Auth(format!(
            "no cursor credentials found (looked in {} and {})",
            self.agent_auth_json.display(),
            self.ide_vscdb.display()
        )))
    }
}

impl Default for CursorAuthResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct AgentAuthJson {
    #[serde(default, rename = "accessToken")]
    access_token: String,
}

/// Read `cursorAuth/accessToken` out of the Cursor IDE's SQLite globalStorage.
/// Returns `Ok(None)` when the row is absent. Opens read-only, falling back to
/// an immutable open so a running IDE (WAL-mode DB) doesn't block the read.
fn read_ide_access_token(db: &Path) -> Result<Option<String>> {
    use rusqlite::{Connection, OpenFlags};

    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .or_else(|_| {
            let uri = format!("file:{}?immutable=1", db.display());
            Connection::open_with_flags(
                &uri,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
        })
        .map_err(|e| Error::Auth(format!("open cursor ide db: {}", e)))?;

    let value: rusqlite::Result<String> = conn.query_row(
        "SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'",
        [],
        |row| row.get(0),
    );
    match value {
        Ok(raw) => {
            let raw = raw.trim();
            // The value may be stored as a JSON-quoted string.
            let token = serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.to_string());
            if token.is_empty() {
                Ok(None)
            } else {
                Ok(Some(token))
            }
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(Error::Auth(format!("query cursor ide db: {}", e))),
    }
}

/// Extract the WorkOS user id from a Cursor JWT's `sub` claim. The `sub` looks
/// like `"google-oauth2|user_01ABC..."`; the dashboard cookie wants the part
/// after the auth-provider prefix (`user_01ABC...`).
fn user_id_from_jwt(token: &str) -> Result<String> {
    let payload_b64 = token
        .split('.')
        .nth(1)
        .ok_or_else(|| Error::Auth("cursor token is not a JWT".into()))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64.trim_end_matches('='))
        .map_err(|e| Error::Auth(format!("decode cursor JWT payload: {}", e)))?;
    let claims: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| Error::Auth(format!("parse cursor JWT: {}", e)))?;
    let sub = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Auth("cursor JWT missing sub claim".into()))?;
    Ok(sub.rsplit('|').next().unwrap_or(sub).to_string())
}

#[async_trait]
impl AuthResolver for CursorAuthResolver {
    async fn resolve(&self) -> Result<ResolvedAuth> {
        let (token, source) = self.access_token()?;
        let user_id = user_id_from_jwt(&token)?;
        // %3A%3A is URL-encoded "::".
        let session_token = format!("{}%3A%3A{}", user_id, token);
        Ok(ResolvedAuth {
            credential: AuthCredential::Cookie(session_token),
            source,
        })
    }

    fn have_credentials(&self) -> bool {
        self.agent_auth_json.exists() || self.ide_vscdb.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake JWT (`header.payload.sig`) carrying the given `sub`.
    fn fake_jwt(sub: &str) -> String {
        let payload = serde_json::json!({ "sub": sub }).to_string();
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("aGVhZGVy.{}.c2ln", b64)
    }

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("quotas-cursor-test-{}", name))
    }

    #[test]
    fn user_id_strips_auth_provider_prefix() {
        let tok = fake_jwt("google-oauth2|user_01KBG9A9SXVHENSG6Z95CMHPYZ");
        assert_eq!(
            user_id_from_jwt(&tok).unwrap(),
            "user_01KBG9A9SXVHENSG6Z95CMHPYZ"
        );
    }

    #[test]
    fn user_id_without_prefix_uses_whole_sub() {
        let tok = fake_jwt("user_bare");
        assert_eq!(user_id_from_jwt(&tok).unwrap(), "user_bare");
    }

    #[tokio::test]
    async fn resolves_from_agent_cli_auth_json() {
        let jwt = fake_jwt("google-oauth2|user_AGENT");
        let auth_path = tmp("agent-auth.json");
        std::fs::write(
            &auth_path,
            serde_json::json!({ "accessToken": jwt }).to_string(),
        )
        .unwrap();
        let resolver = CursorAuthResolver::with_paths(auth_path.clone(), tmp("nonexistent.vscdb"));

        assert!(resolver.have_credentials());
        let resolved = resolver.resolve().await.unwrap();
        let cookie = resolved.credential.unwrap_cookie().unwrap();
        assert_eq!(cookie, format!("user_AGENT%3A%3A{}", jwt));

        std::fs::remove_file(&auth_path).ok();
    }

    #[tokio::test]
    async fn resolves_from_ide_vscdb_when_agent_missing() {
        let jwt = fake_jwt("google-oauth2|user_IDE");
        let db_path = tmp("ide-state.vscdb");
        std::fs::remove_file(&db_path).ok();
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute("CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)", [])
                .unwrap();
            // Stored JSON-quoted, mirroring how VS Code persists string values.
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES ('cursorAuth/accessToken', ?1)",
                [serde_json::Value::String(jwt.clone()).to_string()],
            )
            .unwrap();
        }
        let resolver =
            CursorAuthResolver::with_paths(tmp("no-agent-auth.json"), db_path.clone());

        assert!(resolver.have_credentials());
        let resolved = resolver.resolve().await.unwrap();
        let cookie = resolved.credential.unwrap_cookie().unwrap();
        assert_eq!(cookie, format!("user_IDE%3A%3A{}", jwt));

        std::fs::remove_file(&db_path).ok();
    }

    #[tokio::test]
    async fn missing_everything_errors_and_reports_no_credentials() {
        let resolver =
            CursorAuthResolver::with_paths(tmp("absent-auth.json"), tmp("absent.vscdb"));
        assert!(!resolver.have_credentials());
        assert!(resolver.resolve().await.is_err());
    }
}
