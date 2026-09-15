//! Offline account bookkeeping, persisted as `accounts.json`.
//!
//! Port of the Java `AccountManager`: same offline UUID scheme
//! (see [`crate::auth`]), same case-insensitive dedup, same "current
//! account" semantics.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::auth;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRec {
    pub username: String,
    pub uuid: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountStore {
    pub accounts: Vec<AccountRec>,
    pub current: Option<String>,
}

impl AccountStore {
    pub fn load(path: &Path) -> AccountStore {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => AccountStore::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    /// Add (or select, if the name already exists ignoring case) an offline
    /// account. Returns the stored username.
    pub fn add(&mut self, name: &str) -> Result<String> {
        let name = auth::validate_username(name)?.to_string();
        if let Some(existing) = self
            .accounts
            .iter()
            .find(|a| a.username.eq_ignore_ascii_case(&name))
        {
            self.current = Some(existing.username.clone());
            return Ok(existing.username.clone());
        }
        let rec = AccountRec {
            username: name.clone(),
            uuid: auth::offline_uuid(&name),
        };
        self.accounts.push(rec);
        self.current = Some(name.clone());
        Ok(name)
    }

    /// Remove an account by name; returns true when removed. If the removed
    /// account was current, the first remaining one becomes current.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.username != name);
        let removed = self.accounts.len() < before;
        if removed && self.current.as_deref() == Some(name) {
            self.current = self.accounts.first().map(|a| a.username.clone());
        }
        removed
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("rl-acc-{}-{tag}.json", std::process::id()))
    }

    #[test]
    fn add_validates_and_assigns_uuid() {
        let mut store = AccountStore::default();
        let name = store.add("Rizer001").unwrap();
        assert_eq!(name, "Rizer001");
        assert_eq!(
            store.current().unwrap().uuid,
            auth::offline_uuid("Rizer001")
        );
        assert!(store.add("ab").is_err()); // too short
    }

    #[test]
    fn add_is_case_insensitive_and_selects_existing() {
        let mut store = AccountStore::default();
        store.add("Notch").unwrap();
        let again = store.add("NOTCH").unwrap();
        assert_eq!(again, "Notch");
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.current.as_deref(), Some("Notch"));
    }

    #[test]
    fn remove_falls_back_to_first_account() {
        let mut store = AccountStore::default();
        store.add("alpha").unwrap();
        store.add("beta").unwrap();
        assert!(store.select("beta"));
        assert!(store.remove("beta"));
        assert!(!store.remove("beta"));
        assert_eq!(store.current.as_deref(), Some("alpha"));
    }

    #[test]
    fn save_load_roundtrip() {
        let path = tmp_file("roundtrip");
        let _ = std::fs::remove_file(&path);
        let mut store = AccountStore::default();
        store.add("Rizer001").unwrap();
        store.save(&path).unwrap();
        let loaded = AccountStore::load(&path);
        assert_eq!(loaded, store);
        let _ = std::fs::remove_file(&path);
    }
}
