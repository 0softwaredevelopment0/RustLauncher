//! Launch profiles: named settings snapshots, ported from the Java
//! `ProfileManager` (JSON files under `profiles/` + an index file).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::settings::Settings;

const INDEX_FILE: &str = "profiles_index.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileIndex {
    pub current: Option<String>,
    pub profiles: Vec<String>,
}

impl ProfileIndex {
    pub fn load(profiles_dir: &Path) -> ProfileIndex {
        let path = profiles_dir.join(INDEX_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => ProfileIndex::default(),
        }
    }

    pub fn save(&self, profiles_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(profiles_dir)?;
        let path = profiles_dir.join(INDEX_FILE);
        std::fs::write(path, serde_json::to_string_pretty(self)?)
            .context("failed to write the profile index")?;
        Ok(())
    }

    /// Load the index and ensure at least the default profile exists.
    pub fn load_or_create(profiles_dir: &Path) -> ProfileIndex {
        let mut index = Self::load(profiles_dir);
        if index.profiles.is_empty() {
            index.profiles.push("Default".to_string());
            index.current = Some("Default".to_string());
            let _ = index.save(profiles_dir);
        }
        if !index
            .profiles
            .iter()
            .any(|p| Some(p) == index.current.as_ref())
        {
            index.current = Some(index.profiles[0].clone());
        }
        index
    }
}

/// Sanitize a profile name into a safe file stem.
pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

pub fn profile_path(profiles_dir: &Path, name: &str) -> PathBuf {
    profiles_dir.join(format!("{}.json", sanitize_name(name)))
}

pub fn load_profile(profiles_dir: &Path, name: &str) -> Settings {
    let path = profile_path(profiles_dir, name);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub fn save_profile(profiles_dir: &Path, name: &str, settings: &Settings) -> Result<()> {
    std::fs::create_dir_all(profiles_dir)?;
    let path = profile_path(profiles_dir, name);
    std::fs::write(path, serde_json::to_string_pretty(settings)?)
        .with_context(|| format!("failed to write profile '{name}'"))
}

/// Save the current settings under `name`, then switch the active profile.
pub fn switch_profile(
    profiles_dir: &Path,
    index: &mut ProfileIndex,
    name: &str,
    current_settings: &Settings,
) -> Result<Settings> {
    if !index.profiles.iter().any(|p| p == name) {
        anyhow::bail!("profile '{name}' does not exist");
    }
    if let Some(cur) = &index.current {
        save_profile(profiles_dir, cur, current_settings)?;
    }
    index.current = Some(name.to_string());
    index.save(profiles_dir)?;
    let mut loaded = load_profile(profiles_dir, name);
    loaded.selected_version = current_settings.selected_version.clone(); // keep the on-screen choice
    Ok(loaded)
}

/// Create a new profile (a numbered suffix is appended when the name exists).
pub fn create_profile(profiles_dir: &Path, index: &mut ProfileIndex, name: &str) -> Result<String> {
    let base = name.trim();
    if base.is_empty() {
        anyhow::bail!("profile name is empty");
    }
    let mut final_name = base.to_string();
    let mut counter = 1;
    while index.profiles.contains(&final_name) {
        final_name = format!("{base} ({counter})");
        counter += 1;
    }
    index.profiles.push(final_name.clone());
    save_profile(profiles_dir, &final_name, &Settings::default())?;
    index.current = Some(final_name.clone());
    index.save(profiles_dir)?;
    Ok(final_name)
}

/// Delete a profile; the last remaining profile cannot be deleted.
pub fn delete_profile(profiles_dir: &Path, index: &mut ProfileIndex, name: &str) -> Result<bool> {
    if index.profiles.len() <= 1 {
        anyhow::bail!("cannot delete the last profile");
    }
    let Some(pos) = index.profiles.iter().position(|p| p == name) else {
        return Ok(false);
    };
    index.profiles.remove(pos);
    let _ = std::fs::remove_file(profile_path(profiles_dir, name));
    if index.current.as_deref() == Some(name) {
        index.current = Some(index.profiles[0].clone());
    }
    index.save(profiles_dir)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rl-prof-{}-{tag}", std::process::id()))
    }

    #[test]
    fn index_creates_default_profile() {
        let dir = tmp("default");
        let _ = std::fs::remove_dir_all(&dir);
        let index = ProfileIndex::load_or_create(&dir);
        assert_eq!(index.profiles, vec!["Default".to_string()]);
        assert_eq!(index.current.as_deref(), Some("Default"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_switch_delete_lifecycle() {
        let dir = tmp("lifecycle");
        let _ = std::fs::remove_dir_all(&dir);
        let mut index = ProfileIndex::load_or_create(&dir);

        let created = create_profile(&dir, &mut index, "Testing").unwrap();
        assert_eq!(created, "Testing");
        // Creating a profile switches to it with default settings.
        assert_eq!(index.current.as_deref(), Some("Testing"));

        // The user changed RAM while on Testing; switching away must keep it.
        let live = Settings {
            ram: 8192,
            ..Settings::default()
        };
        let loaded = switch_profile(&dir, &mut index, "Default", &live).unwrap();
        assert_eq!(loaded.ram, 4096);
        let back = switch_profile(&dir, &mut index, "Testing", &loaded).unwrap();
        assert_eq!(back.ram, 8192);

        // Duplicate names get a suffix.
        let dup = create_profile(&dir, &mut index, "Testing").unwrap();
        assert_eq!(dup, "Testing (1)");

        assert!(delete_profile(&dir, &mut index, "Testing").unwrap());
        assert!(delete_profile(&dir, &mut index, "Testing (1)").unwrap());
        assert!(delete_profile(&dir, &mut index, "Default").is_err()); // last one
        assert_eq!(index.profiles, vec!["Default".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_replaces_forbidden_chars() {
        assert_eq!(
            sanitize_name("a/b\\c:d*e?f\"g<h>i|j"),
            "a_b_c_d_e_f_g_h_i_j"
        );
    }

    #[test]
    fn corrupt_profile_file_falls_back_to_defaults() {
        let dir = tmp("corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.json"), b"{ not json").unwrap();
        let s = load_profile(&dir, "broken");
        assert_eq!(s, Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
