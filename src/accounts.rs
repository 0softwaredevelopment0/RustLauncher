//! Account storage backed by the portable SQLite database (`rustlauncher.db`
//! next to the executable), with Argon2id password hashes for offline
//! accounts and tokens for Ely.by / Microsoft accounts.
//!
//! The legacy `accounts.json` is migrated once on first load.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;

use crate::auth::AccountKind;
use crate::db;
use crate::lang::{tr, Language};

/// One stored account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRec {
    pub kind: AccountKind,
    pub username: String,
    pub uuid: String,
    /// Argon2id PHC hash; empty for online accounts (they re-auth instead).
    pub password_hash: String,
    /// Online accounts: cached game token.
    pub access_token: String,
    /// Microsoft accounts: OAuth refresh token.
    pub refresh_token: String,
}

impl AccountRec {
    /// Whether this account can be removed only after extra proof:
    /// offline needs the password, online needs a fresh login.
    #[allow(dead_code)] // kept: removal flow uses kind directly
    pub fn is_offline(&self) -> bool {
        self.kind == AccountKind::Offline
    }
}

/// The account list plus the current selection.
#[derive(Debug, Clone, Default)]
pub struct AccountStore {
    pub accounts: Vec<AccountRec>,
    pub current: Option<String>,
    /// Path of the database behind this store.
    pub db_path: Option<PathBuf>,
}

impl AccountStore {
    /// Load from the database, migrating the legacy `accounts.json` when
    /// present and the DB is still empty. `home_dir` is only used as the
    /// fallback location when the executable directory is unavailable.
    pub fn load(home_dir: &Path) -> AccountStore {
        let path = db::db_path(home_dir);
        let store = Self::load_from(&path).unwrap_or_else(|e| {
            eprintln!("[RustLauncher] account database error: {e:#}");
            AccountStore::default()
        });
        AccountStore {
            db_path: Some(path),
            ..store
        }
    }

    fn load_from(path: &Path) -> Result<AccountStore> {
        let conn = db::open(path)?;
        let mut store = AccountStore::read_all(&conn)?;
        store.migrate_legacy_json(path)?;
        store.db_path = Some(path.to_path_buf());
        Ok(store)
    }

    /// One-time migration of the pre-DB `accounts.json` (offline accounts
    /// without passwords). Accounts that already exist in the DB are kept.
    fn migrate_legacy_json(&mut self, db_path: &Path) -> Result<()> {
        // The json sat next to the DB: in the launcher home.
        let Some(parent) = db_path.parent() else {
            return Ok(());
        };
        let json_path = parent.join("accounts.json");
        let Ok(bytes) = std::fs::read(&json_path) else {
            return Ok(());
        };
        let Ok(legacy) = serde_json::from_slice::<LegacyJson>(&bytes) else {
            return Ok(()); // unreadable legacy file: keep the DB as-is
        };
        let conn = db::open(db_path)?;
        for rec in legacy.accounts {
            if self
                .accounts
                .iter()
                .any(|a| a.kind == AccountKind::Offline && a.username == rec.username)
            {
                continue;
            }
            conn.execute(
                "INSERT OR IGNORE INTO accounts(kind, username, uuid) VALUES ('offline', ?1, ?2)",
                rusqlite::params![rec.username, rec.uuid],
            )
            .context("legacy migration insert failed")?;
            self.accounts.push(AccountRec {
                kind: AccountKind::Offline,
                username: rec.username,
                uuid: rec.uuid,
                password_hash: String::new(),
                access_token: String::new(),
                refresh_token: String::new(),
            });
        }
        if self.current.is_none() {
            self.current = legacy.current;
        }
        let _ = std::fs::rename(&json_path, parent.join("accounts.json.migrated"));
        // Keep only names that exist in the (post-migration) account list.
        self.current = self
            .current
            .take()
            .filter(|c| self.accounts.iter().any(|a| &a.username == c));
        Ok(())
    }

    fn read_all(conn: &Connection) -> Result<AccountStore> {
        let mut stmt = conn.prepare(
            "SELECT kind, username, uuid, password_hash, access_token, refresh_token
             FROM accounts ORDER BY id",
        )?;
        let mut accounts = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let kind: String = row.get(0)?;
            accounts.push(AccountRec {
                kind: AccountKind::from_slug(&kind).unwrap_or(AccountKind::Offline),
                username: row.get(1)?,
                uuid: row.get(2)?,
                password_hash: row.get(3)?,
                access_token: row.get(4)?,
                refresh_token: row.get(5)?,
            });
        }
        // The current selection is not stored in the DB: derive it from the
        // settings file by the caller (gui keeps settings.username).
        let current = accounts.first().map(|a| a.username.clone());
        Ok(AccountStore {
            accounts,
            current,
            db_path: None,
        })
    }

    /// Restore the current selection from the settings username.
    pub fn select_saved(&mut self, name: &str) {
        if !name.is_empty() && self.accounts.iter().any(|a| a.username == name) {
            self.current = Some(name.to_string());
        }
    }

    fn conn(&self) -> Result<Connection> {
        let path = self
            .db_path
            .clone()
            .ok_or_else(|| anyhow!("account database path is not set"))?;
        db::open(&path)
    }

    /// Add an offline account protected by a password (Argon2id-hashed).
    pub fn add_offline(&mut self, name: &str, password: &str, lang: Language) -> Result<String> {
        let name = crate::auth::validate_username(name, lang)?.to_string();
        if password.is_empty() {
            return Err(anyhow!("{}", tr(lang, "a password is required for an offline account")));
        }
        if self
            .accounts
            .iter()
            .any(|a| a.kind == AccountKind::Offline && a.username.eq_ignore_ascii_case(&name))
        {
            return Err(anyhow!(
                "{}",
                tr(lang, "an offline account with this nickname already exists")
            ));
        }
        let hash = db::hash_password(password)?;
        let uuid = crate::auth::offline_uuid(&name);
        self.conn()?.execute(
            "INSERT INTO accounts(kind, username, uuid, password_hash) VALUES ('offline', ?1, ?2, ?3)",
            rusqlite::params![name, uuid, hash],
        )?;
        self.accounts.push(AccountRec {
            kind: AccountKind::Offline,
            username: name.clone(),
            uuid,
            password_hash: hash,
            access_token: String::new(),
            refresh_token: String::new(),
        });
        self.current = Some(name.clone());
        Ok(name)
    }

    /// Add an Ely.by account from a fresh login result.
    pub fn add_elyby(&mut self, username: &str, uuid: &str, access_token: &str) -> Result<String> {
        self.upsert_online(AccountKind::ElyBy, username, uuid, access_token, "")
    }

    /// Add (or refresh) a Microsoft account from a fresh login result.
    pub fn add_mojang(
        &mut self,
        username: &str,
        uuid: &str,
        access_token: &str,
        refresh_token: &str,
    ) -> Result<String> {
        self.upsert_online(
            AccountKind::Mojang,
            username,
            uuid,
            access_token,
            refresh_token,
        )
    }

    fn upsert_online(
        &mut self,
        kind: AccountKind,
        username: &str,
        uuid: &str,
        access_token: &str,
        refresh_token: &str,
    ) -> Result<String> {
        let existing = self
            .accounts
            .iter()
            .position(|a| a.kind == kind && a.username == username);
        self.conn()?.execute(
            "INSERT INTO accounts(kind, username, uuid, access_token, refresh_token)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(kind, username) DO UPDATE SET
                uuid = excluded.uuid,
                access_token = excluded.access_token,
                refresh_token = excluded.refresh_token",
            rusqlite::params![kind.slug(), username, uuid, access_token, refresh_token],
        )?;
        let rec = AccountRec {
            kind,
            username: username.to_string(),
            uuid: uuid.to_string(),
            password_hash: String::new(),
            access_token: access_token.to_string(),
            refresh_token: refresh_token.to_string(),
        };
        match existing {
            Some(i) => self.accounts[i] = rec,
            None => self.accounts.push(rec),
        }
        self.current = Some(username.to_string());
        Ok(username.to_string())
    }

    /// Verify an offline account's password (also required to remove it).
    pub fn verify_offline_password(&self, name: &str, password: &str) -> bool {
        self.accounts
            .iter()
            .find(|a| a.kind == AccountKind::Offline && a.username == name)
            .map(|a| db::verify_password(password, &a.password_hash))
            .unwrap_or(false)
    }

    /// Remove an account by name (kind-aware). The caller must have verified
    /// the password / re-authenticated first.
    pub fn remove(&mut self, name: &str) -> Result<bool> {
        let Some(rec) = self.accounts.iter().find(|a| a.username == name) else {
            return Ok(false);
        };
        let kind = rec.kind;
        self.conn()?.execute(
            "DELETE FROM accounts WHERE kind = ?1 AND username = ?2",
            rusqlite::params![kind.slug(), name],
        )?;
        self.accounts.retain(|a| a.username != name);
        if self.current.as_deref() == Some(name) {
            self.current = self.accounts.first().map(|a| a.username.clone());
        }
        Ok(true)
    }

    pub fn select(&mut self, name: &str) -> bool {
        if self.accounts.iter().any(|a| a.username == name) {
            self.current = Some(name.to_string());
            true
        } else {
            false
        }
    }

    pub fn current(&self) -> Option<&AccountRec> {
        self.current
            .as_deref()
            .and_then(|name| self.accounts.iter().find(|a| a.username == name))
    }

    /// Refresh the cached token of an online account after a re-login.
    #[allow(dead_code)] // reserved for token refresh (v2)
    pub fn update_token(&mut self, name: &str, access_token: &str, refresh_token: &str) {
        if let Some(rec) = self.accounts.iter_mut().find(|a| a.username == name) {
            rec.access_token = access_token.to_string();
            rec.refresh_token = refresh_token.to_string();
        }
        let _ = self.conn().map(|conn| {
            let _ = conn.execute(
                "UPDATE accounts SET access_token = ?1, refresh_token = ?2 WHERE username = ?3",
                rusqlite::params![access_token, refresh_token, name],
            );
        });
    }
}

/// The legacy `accounts.json` layout.
#[derive(serde::Deserialize)]
struct LegacyJson {
    #[serde(default)]
    accounts: Vec<LegacyRec>,
    #[serde(default)]
    current: Option<String>,
}

#[derive(serde::Deserialize)]
struct LegacyRec {
    username: String,
    #[serde(default)]
    uuid: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rl-accdb-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("rustlauncher.db")
    }

    #[test]
    fn offline_add_requires_password_and_validates_name() {
        use crate::lang::Language;
        let path = tmp_db("add");
        let mut store = AccountStore::load_from(&path).unwrap();
        assert!(store.add_offline("Rizer001", "", Language::English).is_err());
        assert!(store.add_offline("ab", "pw", Language::English).is_err());
        let name = store.add_offline("Rizer001", "hunter2", Language::English).unwrap();
        assert_eq!(name, "Rizer001");
        assert!(store.verify_offline_password("Rizer001", "hunter2"));
        assert!(!store.verify_offline_password("Rizer001", "wrong"));
        assert!(store.add_offline("rizer001", "x", Language::English).is_err()); // dup
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn remove_offline_after_password_and_fallback() {
        use crate::lang::Language;
        let path = tmp_db("rm");
        let mut store = AccountStore::load_from(&path).unwrap();
        store.add_offline("alpha", "pw1", Language::English).unwrap();
        store.add_offline("beta", "pw2", Language::English).unwrap();
        store.select("beta");
        assert!(!store.verify_offline_password("beta", "nope"));
        assert!(store.verify_offline_password("beta", "pw2"));
        assert!(store.remove("beta").unwrap());
        assert!(!store.remove("beta").unwrap());
        assert_eq!(store.current.as_deref(), Some("alpha"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn online_upsert_refreshes_token() {
        let path = tmp_db("online");
        let mut store = AccountStore::load_from(&path).unwrap();
        store.add_elyby("Steve", "uuid-1", "tok-1").unwrap();
        store.add_elyby("Steve", "uuid-1", "tok-2").unwrap();
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.accounts[0].access_token, "tok-2");
        store.add_mojang("Alex", "uuid-2", "mt-1", "rt-1").unwrap();
        store.update_token("Alex", "mt-2", "rt-2");
        let alex = store
            .accounts
            .iter()
            .find(|a| a.username == "Alex")
            .unwrap();
        assert_eq!(alex.access_token, "mt-2");
        assert_eq!(alex.refresh_token, "rt-2");
        // Persisted.
        let reloaded = AccountStore::load_from(&path).unwrap();
        assert_eq!(reloaded.accounts.len(), 2);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn legacy_json_is_migrated() {
        let dir = tmp_db("legacy").parent().unwrap().to_path_buf();
        std::fs::write(
            dir.join("accounts.json"),
            r#"{"accounts":[{"username":"OldGuy","uuid":"abc"},{"username":"Skip","uuid":"s"}],"current":"OldGuy"}"#,
        )
        .unwrap();
        let store = AccountStore::load_from(&dir.join("rustlauncher.db")).unwrap();
        // Both legacy accounts are migrated in without passwords.
        assert!(store
            .accounts
            .iter()
            .any(|a| a.username == "Skip" && a.password_hash.is_empty()));
        assert!(store
            .accounts
            .iter()
            .any(|a| a.username == "OldGuy" && a.password_hash.is_empty()));
        assert_eq!(store.current.as_deref(), Some("OldGuy"));
        assert!(!dir.join("accounts.json").exists());
        assert!(dir.join("accounts.json.migrated").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
