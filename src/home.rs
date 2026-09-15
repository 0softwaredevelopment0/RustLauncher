//! The launcher home — the single place where settings, accounts, servers,
//! profiles, skins and logs live.
//!
//! Resolution order (port of the Java `LauncherHomeProvider`):
//! 1. `RUSTLAUNCHER_HOME` environment variable (portable distribution).
//! 2. The OS data directory: `%APPDATA%` on Windows,
//!    `~/Library/Application Support` on macOS, `~/.local/share` elsewhere,
//!    plus the `RustLauncher` folder name.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Absolute path to the launcher home (the directory itself is not created).
pub fn launcher_home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RUSTLAUNCHER_HOME") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let base = dirs::data_dir().context("could not resolve the OS data directory")?;
    Ok(base.join("RustLauncher"))
}

/// Create the launcher home directory (and parents) if missing.
pub fn ensure(home: &Path) -> Result<()> {
    std::fs::create_dir_all(home).with_context(|| format!("failed to create {}", home.display()))
}

pub fn skins_dir(home: &Path) -> PathBuf {
    home.join("skins")
}

pub fn profiles_dir(home: &Path) -> PathBuf {
    home.join("profiles")
}

pub fn logs_dir(home: &Path) -> PathBuf {
    home.join("logs")
}

pub fn config_file(home: &Path) -> PathBuf {
    home.join("config.json")
}

pub fn accounts_file(home: &Path) -> PathBuf {
    home.join("accounts.json")
}

pub fn servers_file(home: &Path) -> PathBuf {
    home.join("servers.json")
}
