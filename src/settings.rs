//! Launcher settings persisted as `config.json` in the launcher home.
//!
//! The old Java launcher kept ~30 loosely-typed key/value pairs in SQLite;
//! this port keeps the settings that affect behavior in a typed struct.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::home;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Currently selected account name.
    pub username: String,
    /// Game heap size in MB.
    pub ram: u32,
    pub game_width: u32,
    pub game_height: u32,
    pub use_custom_resolution: bool,
    /// Custom JVM arguments (safety-filtered before launch).
    pub java_args: String,
    /// Explicit java executable path; empty means auto-detect.
    pub java_path: String,
    /// Game directory override; empty means the default game dir.
    pub game_directory: String,
    pub selected_version: String,
    /// Server to auto-connect to ("host" or "host:port").
    pub connect_server_ip: String,
    pub auto_connect: bool,
    /// Write the game console output to a numbered log file.
    pub save_console_log: bool,
    /// Console shows every log line instead of errors only.
    pub all_logs: bool,
    pub dark_theme: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            username: String::new(),
            ram: 4096,
            game_width: 854,
            game_height: 480,
            use_custom_resolution: false,
            java_args: String::new(),
            java_path: String::new(),
            game_directory: String::new(),
            selected_version: String::new(),
            connect_server_ip: String::new(),
            auto_connect: false,
            save_console_log: true,
            all_logs: false,
            dark_theme: true,
        }
    }
}

impl Settings {
    /// Load settings; missing or corrupt file falls back to defaults
    /// (the old launcher silently carried on too, but at least we say why).
    pub fn load(home_dir: &Path) -> Settings {
        let path = home::config_file(home_dir);
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(settings) => settings,
                Err(e) => {
                    eprintln!(
                        "[RustLauncher] corrupt {} ({e}); using defaults",
                        path.display()
                    );
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, home_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(home_dir)?;
        let path = home::config_file(home_dir);
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("rl-set-{}-{tag}", std::process::id()))
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tmp_home("missing");
        let _ = std::fs::remove_dir_all(&dir);
        let s = Settings::load(&dir);
        assert_eq!(s.ram, 4096);
        assert_eq!(s.game_width, 854);
        assert!(s.dark_theme);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tmp_home("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let s = Settings {
            ram: 8192,
            username: "Rizer001".into(),
            java_args: "-XX:+UseG1GC".into(),
            ..Settings::default()
        };
        s.save(&dir).unwrap();
        let loaded = Settings::load(&dir);
        assert_eq!(loaded, s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = tmp_home("corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), b"{ not json").unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s, Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_fields_do_not_break_loading() {
        let dir = tmp_home("extra");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            br#"{"ram": 2048, "someRemovedSetting": true}"#,
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.ram, 2048);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
