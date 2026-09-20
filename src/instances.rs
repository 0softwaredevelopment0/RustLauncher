//! Game instances: named collections pointing at a game root directory.
//!
//! An instance is just (name, game directory). Launching picks an instance;
//! every instance can run at the same time because each game process gets
//! its own working dir and console stream. Deleting an instance removes only
//! the launcher-side record — the game directory is never touched.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::lang::{tr, Language};

const FILE_NAME: &str = "instances.json";

/// One launch target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    /// The instance's own game root (its `versions/`, `saves/`, … live here).
    pub game_dir: String,
}

/// The persisted list of instances.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceStore {
    pub instances: Vec<Instance>,
}

impl InstanceStore {
    pub fn load(home: &Path) -> InstanceStore {
        let path = home.join(FILE_NAME);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => InstanceStore::default(),
        }
    }

    pub fn save(&self, home: &Path, lang: Language) -> Result<()> {
        let path = home.join(FILE_NAME);
        std::fs::write(path, serde_json::to_string_pretty(self)?)
            .context(tr(lang, "failed to write instances.json"))?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name == name)
    }

    /// Create an instance; a numbered suffix is appended when the name exists.
    /// Returns the final name.
    pub fn create(&mut self, name: &str, game_dir: &str, lang: Language) -> Result<String> {
        let base = name.trim();
        if base.is_empty() {
            anyhow::bail!("{}", tr(lang, "instance name is empty"));
        }
        let dir = game_dir.trim();
        if dir.is_empty() {
            anyhow::bail!("{}", tr(lang, "instance game directory is empty"));
        }
        let mut final_name = base.to_string();
        let mut counter = 1;
        while self.instances.iter().any(|i| i.name == final_name) {
            final_name = format!("{base} ({counter})");
            counter += 1;
        }
        self.instances.push(Instance {
            name: final_name.clone(),
            game_dir: dir.to_string(),
        });
        Ok(final_name)
    }

    /// Delete an instance record. Returns false when the name is unknown.
    /// The game directory on disk is NOT removed.
    pub fn delete(&mut self, name: &str) -> bool {
        let Some(pos) = self.instances.iter().position(|i| i.name == name) else {
            return false;
        };
        self.instances.remove(pos);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rl-inst-{tag}-{}", std::process::id()))
    }

    #[test]
    fn create_get_delete_lifecycle() {
        use crate::lang::Language;
        let mut store = InstanceStore::default();
        let name = store
            .create("My Pack", "C:/Games/MyPack", Language::English)
            .unwrap();
        assert_eq!(name, "My Pack");
        assert_eq!(
            store.get("My Pack").map(|i| i.game_dir.as_str()),
            Some("C:/Games/MyPack")
        );

        // Duplicate names get a numbered suffix.
        let dup = store
            .create("My Pack", "C:/Games/Other", Language::English)
            .unwrap();
        assert_eq!(dup, "My Pack (1)");

        assert!(store.delete("My Pack"));
        assert!(!store.delete("My Pack")); // already gone
        assert!(store.get("My Pack (1)").is_some());
    }

    #[test]
    fn rejects_empty_name_and_dir() {
        use crate::lang::Language;
        let mut store = InstanceStore::default();
        assert!(store.create("", "C:/x", Language::English).is_err());
        assert!(store.create("X", "  ", Language::English).is_err());
    }

    #[test]
    fn persists_roundtrip() {
        use crate::lang::Language;
        let dir = tmp("persist");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut store = InstanceStore::default();
        store.create("A", "C:/Games/A", Language::English).unwrap();
        store.save(&dir, Language::English).unwrap();

        let loaded = InstanceStore::load(&dir);
        assert_eq!(loaded, store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_empty() {
        let dir = tmp("corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), b"{ not json").unwrap();
        let store = InstanceStore::load(&dir);
        assert!(store.instances.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
