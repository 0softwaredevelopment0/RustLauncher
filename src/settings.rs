//! Launcher settings persisted as `config.json` in the launcher home.
//!
//! The old Java launcher kept ~30 loosely-typed key/value pairs in SQLite;
//! this port keeps the settings that affect behavior in a typed struct.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::home;
use crate::jvm;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Currently selected account name.
    pub username: String,
    pub game_width: u32,
    pub game_height: u32,
    pub use_custom_resolution: bool,
    /// JVM arguments. Heap size lives here (`-Xms`/`-Xmx`); the game refuses
    /// to launch without them. Defaults to [`jvm::DEFAULT_JVM_ARGS`].
    pub java_args: String,
    /// When false (default) the launcher auto-detects Java; when true,
    /// `java_path` must point at a java executable.
    pub use_custom_java: bool,
    /// Explicit java executable path, used only in the custom mode.
    pub java_path: String,
    /// Game directory. The launcher refuses to launch without it (empty means
    /// "not configured yet").
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
    /// Ask for confirmation before force-killing the game (Kill button).
    pub confirm_kill: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            username: String::new(),
            game_width: 854,
            game_height: 480,
            use_custom_resolution: false,
            java_args: jvm::DEFAULT_JVM_ARGS.to_string(),
            use_custom_java: false,
            java_path: String::new(),
            game_directory: String::new(),
            selected_version: String::new(),
            connect_server_ip: String::new(),
            auto_connect: false,
            save_console_log: true,
            all_logs: false,
            dark_theme: true,
            confirm_kill: true,
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
        assert_eq!(s.java_args, jvm::DEFAULT_JVM_ARGS);
        assert!(!s.use_custom_java);
        assert_eq!(s.game_width, 854);
        assert!(s.dark_theme);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tmp_home("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let s = Settings {
            java_args: "-Xms2g -Xmx8g -XX:+UseZGC".into(),
            username: "Rizer001".into(),
            use_custom_java: true,
            java_path: "C:/java/bin/java.exe".into(),
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
    fn unknown_and_legacy_fields_do_not_break_loading() {
        let dir = tmp_home("extra");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // `ram` was removed from the struct; old configs must still load.
        std::fs::write(
            dir.join("config.json"),
            br#"{"ram": 2048, "someRemovedSetting": true}"#,
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.java_args, jvm::DEFAULT_JVM_ARGS);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
