//! Modrinth modpack (.mrpack) installation.
//!
//! A `.mrpack` is a ZIP with `modrinth.index.json` at its root: the index
//! declares the game and loader versions plus every file to download, and
//! the archive carries `overrides/` / `client-overrides/` folders that are
//! extracted into the instance directory on top of the downloads.
//!
//! The engine reuses the vanilla `updater` machinery for the Minecraft
//! version and the mod loader, then materializes the pack files and the
//! override layers inside the instance's game root. Every install step is
//! idempotent: files that already exist with a correct SHA-1 are kept, so
//! an interrupted install can simply be retried.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::lang::{tr, tr_fmt, Language};
use crate::updater;

/// The parsed `modrinth.index.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct MrpackIndex {
    #[serde(rename = "formatVersion", default)]
    pub format_version: u32,
    #[serde(default)]
    pub game: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub files: Vec<MrpackFile>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

/// One downloadable file of the pack.
#[derive(Debug, Clone, Deserialize)]
pub struct MrpackFile {
    /// Destination path relative to the instance directory.
    pub path: String,
    #[serde(default)]
    pub hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub env: Option<MrpackEnv>,
    #[serde(default)]
    pub downloads: Vec<String>,
    #[serde(default, rename = "fileSize")]
    pub file_size: u64,
}

impl MrpackFile {
    /// Client-launcher rule: the file is installed unless the index marks
    /// it `unsupported` on the client side. A missing `env` installs.
    pub fn client_installable(&self) -> bool {
        self.env
            .as_ref()
            .and_then(|e| e.client.as_deref())
            .map(|side| side != "unsupported")
            .unwrap_or(true)
    }

    /// The SHA-1 declared for this file (empty when absent).
    pub fn sha1(&self) -> &str {
        self.hashes.get("sha1").map(String::as_str).unwrap_or("")
    }
}

/// Per-environment availability of a pack file.
#[derive(Debug, Clone, Deserialize)]
pub struct MrpackEnv {
    #[serde(default)]
    pub client: Option<String>,
    /// Parsed for format completeness; the client launcher never consults
    /// the server side (server packs are out of scope).
    #[allow(dead_code)]
    #[serde(default)]
    pub server: Option<String>,
}

impl MrpackIndex {
    /// Parse and validate an index from raw JSON bytes.
    pub fn parse(bytes: &[u8]) -> Result<MrpackIndex> {
        let index: MrpackIndex = serde_json::from_slice(bytes).with_context(|| {
            tr(
                Language::English,
                "failed to parse modrinth.index.json: parse error",
            )
        })?;
        index.validate()?;
        Ok(index)
    }

    /// Reject indexes this launcher cannot install (wrong format, wrong
    /// game, missing/ambiguous loader dependencies).
    pub fn validate(&self) -> Result<()> {
        if self.format_version != 1 {
            return Err(anyhow!(
                "{}",
                tr_fmt(
                    Language::English,
                    "unsupported modpack format version {0}",
                    &[&self.format_version.to_string()]
                )
            ));
        }
        if self.game != "minecraft" {
            return Err(anyhow!(
                "{}",
                tr_fmt(
                    Language::English,
                    "unsupported modpack game '{0}'",
                    &[&self.game]
                )
            ));
        }
        self.mc_version()?;
        let loaders = self
            .dependencies
            .keys()
            .filter(|k| loader_key(k).is_some())
            .count();
        if loaders != 1 {
            return Err(anyhow!(
                "{}",
                tr_fmt(
                    Language::English,
                    "the modpack must declare exactly one loader (found {0})",
                    &[&loaders.to_string()]
                )
            ));
        }
        Ok(())
    }

    /// The Minecraft version the pack is built for.
    pub fn mc_version(&self) -> Result<&str> {
        self.dependencies
            .get("minecraft")
            .map(String::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                anyhow!(
                    "{}",
                    tr(
                        Language::English,
                        "the modpack does not declare a Minecraft version",
                    )
                )
            })
    }

    /// The single loader dependency as `(loader, pinned build)`.
    pub fn loader_dep(&self) -> Result<(updater::Loader, &str)> {
        let (key, build) = self
            .dependencies
            .iter()
            .find(|(k, _)| loader_key(k).is_some())
            .ok_or_else(|| {
                anyhow!(
                    "{}",
                    tr(
                        Language::English,
                        "the modpack must declare exactly one loader (found 0)",
                    )
                )
            })?;
        let loader = loader_key(key).expect("filtered above");
        Ok((loader, build.as_str()))
    }
}

/// Map a `dependencies` key to a supported loader; unknown keys (the spec
/// may add IDs) return `None`.
fn loader_key(key: &str) -> Option<updater::Loader> {
    match key {
        "fabric-loader" => Some(updater::Loader::Fabric),
        "quilt-loader" => Some(updater::Loader::Quilt),
        "forge" => Some(updater::Loader::Forge),
        "neoforge" => Some(updater::Loader::NeoForge),
        _ => None,
    }
}

/// Map a raw relative path from the index (or from an override archive) to
/// a safe path inside the instance directory. Returns `None` for anything
/// that escapes it: `..` components, drive prefixes, absolute `/`/`\`
/// starts (the .mrpack spec requires exactly this validation).
pub fn instance_rel_path(raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // Drive names: `C:/...`, `C:\...`, bare `C:`.
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        return None;
    }
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANG: Language = Language::English;

    fn parse(json: &str) -> Result<MrpackIndex> {
        MrpackIndex::parse(json.as_bytes())
    }

    const GOOD: &str = r#"{
        "formatVersion": 1,
        "game": "minecraft",
        "versionId": "1.0.0",
        "name": "Test Pack",
        "summary": "a pack for tests",
        "files": [
            {
                "path": "mods/a.jar",
                "hashes": {"sha1": "aa", "sha512": "bb"},
                "env": {"client": "required", "server": "unsupported"},
                "downloads": ["https://cdn.modrinth.com/a.jar"],
                "fileSize": 10
            },
            {
                "path": "mods/server-only.jar",
                "hashes": {"sha1": "cc"},
                "env": {"client": "unsupported", "server": "required"},
                "downloads": ["https://cdn.modrinth.com/b.jar"]
            },
            {
                "path": "config/x.cfg",
                "downloads": ["https://cdn.modrinth.com/x.cfg"]
            }
        ],
        "dependencies": {
            "minecraft": "1.21.4",
            "fabric-loader": "0.16.9",
            "some-future-thing": "9.9"
        }
    }"#;

    #[test]
    fn happy_path_index_parses() {
        let index = parse(GOOD).unwrap();
        assert_eq!(index.name, "Test Pack");
        assert_eq!(index.summary, "a pack for tests");
        assert_eq!(index.files.len(), 3);
        assert_eq!(index.files[0].file_size, 10);
        assert_eq!(index.mc_version().unwrap(), "1.21.4");
        let (loader, build) = index.loader_dep().unwrap();
        assert_eq!(loader, updater::Loader::Fabric);
        assert_eq!(build, "0.16.9");
    }

    #[test]
    fn unknown_dependency_keys_are_tolerated() {
        // `some-future-thing` must not break parsing (the spec may add IDs).
        assert!(parse(GOOD).is_ok());
    }

    #[test]
    fn wrong_game_is_rejected() {
        let err = parse(GOOD.replace("\"minecraft\"", "\"minetest\"").as_str()).unwrap_err();
        assert!(err.to_string().contains("minetest"), "{err:#}");
    }

    #[test]
    fn wrong_format_version_is_rejected() {
        let err = parse(GOOD.replace("\"formatVersion\": 1", "\"formatVersion\": 2").as_str())
            .unwrap_err();
        assert!(err.to_string().contains("format version"), "{err:#}");
    }

    #[test]
    fn missing_minecraft_dependency_is_rejected() {
        let body = r#"{
            "formatVersion": 1, "game": "minecraft", "name": "x",
            "dependencies": {"fabric-loader": "0.16.9"}
        }"#;
        let err = parse(body).unwrap_err();
        assert!(err.to_string().contains("Minecraft version"), "{err:#}");
    }

    #[test]
    fn two_loaders_are_rejected() {
        let body = r#"{
            "formatVersion": 1, "game": "minecraft", "name": "x",
            "dependencies": {"minecraft": "1.21.4", "fabric-loader": "0.16.9", "forge": "47.0.0"}
        }"#;
        let err = parse(body).unwrap_err();
        assert!(err.to_string().contains("exactly one loader"), "{err:#}");
    }

    #[test]
    fn no_known_loader_is_rejected() {
        let body = r#"{
            "formatVersion": 1, "game": "minecraft", "name": "x",
            "dependencies": {"minecraft": "1.21.4"}
        }"#;
        let err = parse(body).unwrap_err();
        assert!(err.to_string().contains("exactly one loader"), "{err:#}");
    }

    #[test]
    fn every_known_loader_key_maps() {
        for (key, expected) in [
            ("fabric-loader", updater::Loader::Fabric),
            ("quilt-loader", updater::Loader::Quilt),
            ("forge", updater::Loader::Forge),
            ("neoforge", updater::Loader::NeoForge),
        ] {
            let body = format!(
                r#"{{"formatVersion": 1, "game": "minecraft", "name": "x",
                    "dependencies": {{"minecraft": "1.21.4", "{key}": "1.0"}}}}"#
            );
            let index = parse(&body).unwrap();
            let (loader, build) = index.loader_dep().unwrap();
            assert_eq!(loader, expected, "key {key}");
            assert_eq!(build, "1.0");
        }
    }

    #[test]
    fn env_filtering_rules() {
        let index = parse(GOOD).unwrap();
        assert!(index.files[0].client_installable()); // required
        assert!(!index.files[1].client_installable()); // unsupported
        assert!(index.files[2].client_installable()); // no env at all
        assert_eq!(index.files[0].sha1(), "aa");
    }

    #[test]
    fn rel_paths_inside_the_instance_are_accepted() {
        let p = instance_rel_path("mods/a.jar").unwrap();
        assert_eq!(p, PathBuf::from("mods").join("a.jar"));
        assert_eq!(
            instance_rel_path("config//nested\\x.txt").unwrap(),
            PathBuf::from("config").join("nested").join("x.txt")
        );
        assert!(instance_rel_path("./options.txt").is_some());
    }

    #[test]
    fn escaping_paths_are_rejected() {
        assert!(instance_rel_path("../evil").is_none());
        assert!(instance_rel_path("mods/../../evil").is_none());
        assert!(instance_rel_path("C:/evil").is_none());
        assert!(instance_rel_path("C:\\evil").is_none());
        assert!(instance_rel_path("/evil").is_none());
        assert!(instance_rel_path("\\evil").is_none());
        assert!(instance_rel_path("").is_none());
        assert!(instance_rel_path("   ").is_none());
        assert!(instance_rel_path(".").is_none());
    }

    #[test]
    fn parse_errors_mention_the_index() {
        let err = MrpackIndex::parse(b"not json").unwrap_err();
        let _ = LANG;
        assert!(err.to_string().contains("modrinth.index.json"), "{err:#}");
    }
}
