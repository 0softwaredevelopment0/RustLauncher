//! The launcher account database — a SQLite file placed next to the running
//! executable (portable layout), storing account records with Argon2id
//! password hashes for Offline accounts.
//!
//! Offline accounts protect their identity with a password: the hash
//! (Argon2id) is stored in the DB, never the password itself. Online
//! accounts (Ely.by / Microsoft) keep their tokens in the same DB.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rusqlite::Connection;

// OsRng comes from the standalone rand_core crate (getrandom feature); the
// copy re-exported inside argon2::password_hash is feature-gated off.
use rand_core::OsRng;

/// Resolve the database path: next to the running executable. Falls back to
/// the launcher home when the executable path cannot be determined (tests,
/// exotic environments).
pub fn db_path(home_dir: &Path) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
        .map(|dir| dir.join("rustlauncher.db"))
        .unwrap_or_else(|| home_dir.join("rustlauncher.db"))
}

/// Open (and initialize) the database, creating the file and schema as needed.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create the database directory {}",
                parent.display()
            )
        })?;
    }
    let conn =
        Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS accounts (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            kind        TEXT NOT NULL CHECK (kind IN ('offline','elyby','mojang')),
            username    TEXT NOT NULL,
            uuid        TEXT NOT NULL,
            password_hash TEXT NOT NULL DEFAULT '',
            access_token  TEXT NOT NULL DEFAULT '',
            refresh_token TEXT NOT NULL DEFAULT '',
            extra       TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_accounts_kind_username
            ON accounts(kind, username);",
    )
    .context("failed to initialize the account database schema")?;
    Ok(conn)
}

/// Hash a password with Argon2id (random 16-byte salt, default OWASP params).
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("failed to hash the password: {e}"))
}

/// Verify a password against a stored PHC-format Argon2id hash.
pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .ok()
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let hash = hash_password("hunter2").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("hunter2", &hash));
        assert!(!verify_password("hunter3", &hash));
    }

    #[test]
    fn verify_rejects_garbage_hash() {
        assert!(!verify_password("x", ""));
        assert!(!verify_password("x", "not-a-hash"));
    }

    #[test]
    fn open_creates_schema_in_temp_dir() {
        let dir = std::env::temp_dir().join(format!("rl-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("rustlauncher.db");
        let conn = open(&path).unwrap();
        conn.execute(
            "INSERT INTO accounts(kind, username, uuid) VALUES ('offline', 'T', 'u')",
            [],
        )
        .unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
