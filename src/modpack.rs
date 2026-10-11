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
use std::io::Read;
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

// ---------------------------------------------------------------------------
// Installation pipeline
// ---------------------------------------------------------------------------

/// What one `install` run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    /// Human-readable pack name from the index.
    pub pack_name: String,
    /// The mod-loader version id installed (the instance's launch target).
    pub version_id: String,
    /// Pack files downloaded this run.
    pub files_downloaded: usize,
    /// Pack files already present with a correct SHA-1 (skipped).
    pub files_skipped: usize,
    /// Files written from the override layers.
    pub overrides_extracted: usize,
}

/// Read `modrinth.index.json` out of a `.mrpack` archive.
pub fn read_index(zip_path: &Path, lang: Language) -> Result<MrpackIndex> {
    let file = std::fs::File::open(zip_path)
        .with_context(|| tr(lang, "failed to open the zip archive"))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| tr(lang, "failed to open the zip archive"))?;
    let mut bytes = Vec::new();
    archive
        .by_name(INDEX_ENTRY)
        .context(tr(lang, "the archive has no modrinth.index.json"))?
        .read_to_end(&mut bytes)
        .context(tr(lang, "the archive has no modrinth.index.json"))?;
    MrpackIndex::parse(&bytes)
}

const INDEX_ENTRY: &str = "modrinth.index.json";
/// Override layers applied to a client instance, in order (later layers
/// overwrite earlier ones; `server-overrides` is never touched).
const OVERRIDE_LAYERS: [&str; 2] = ["overrides", "client-overrides"];

/// Install a `.mrpack` into `game_root`:
/// Minecraft version + pinned loader via the vanilla `updater` machinery,
/// then the indexed pack files, then the override layers. Idempotent —
/// already-present files with a correct SHA-1 are kept.
pub fn install(
    zip_path: &Path,
    game_root: &Path,
    agent: &ureq::Agent,
    lang: Language,
    progress: &mut dyn FnMut(&str),
) -> Result<InstallOutcome> {
    let index = read_index(zip_path, lang)?;
    let mc = index.mc_version()?.to_string();
    let (loader, build) = index.loader_dep()?;
    let build = build.to_string();

    // 1. The vanilla version the pack targets.
    progress(&tr_fmt(lang, "Installing Minecraft {0}", &[&mc]));
    let manifest = updater::fetch_manifest(agent, lang)?;
    let version = updater::find_version(&manifest, &mc).ok_or_else(|| {
        anyhow!(
            "{}",
            tr_fmt(lang, "version {0} is required but not installed", &[&mc])
        )
    })?;
    updater::install_version(agent, game_root, version, lang, progress)?;

    // 2. The pinned loader build on top of it.
    progress(&tr_fmt(
        lang,
        "Installing {0} {1}",
        &[loader.label(), &build],
    ));
    let builds = updater::fetch_loader_builds(agent, loader, &mc, lang)?;
    let chosen = builds
        .iter()
        .find(|b| b.version == build)
        .ok_or_else(|| {
            anyhow!(
                "{}",
                tr_fmt(
                    lang,
                    "loader build {0} for {1} not found",
                    &[&build, &mc]
                )
            )
        })?
        .clone();
    let loader_outcome =
        updater::install_loader(agent, game_root, loader, &mc, &chosen, lang, progress)?;

    // 3. The indexed pack files.
    let (mut downloaded, mut skipped) = (0usize, 0usize);
    let total = index.files.len();
    for (i, pack_file) in index.files.iter().enumerate() {
        if install_pack_file(agent, game_root, pack_file, lang)? {
            downloaded += 1;
        } else {
            skipped += 1;
        }
        progress(&tr_fmt(
            lang,
            "pack files {0}/{1}",
            &[&(i + 1).to_string(), &total.to_string()],
        ));
    }

    // 4. The override layers.
    progress(&tr(lang, "Extracting overrides…"));
    let file = std::fs::File::open(zip_path)
        .with_context(|| tr(lang, "failed to open the zip archive"))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| tr(lang, "failed to open the zip archive"))?;
    let overrides = extract_overlays(&mut archive, game_root, lang)?;

    Ok(InstallOutcome {
        pack_name: index.name,
        version_id: loader_outcome.version_id,
        files_downloaded: downloaded,
        files_skipped: skipped,
        overrides_extracted: overrides,
    })
}

/// Download one indexed pack file (unless a copy with the correct SHA-1 is
/// already in place). Returns true when it was downloaded.
fn install_pack_file(
    agent: &ureq::Agent,
    game_root: &Path,
    pack_file: &MrpackFile,
    lang: Language,
) -> Result<bool> {
    if !pack_file.client_installable() {
        return Ok(false);
    }
    let rel = instance_rel_path(&pack_file.path).ok_or_else(|| {
        anyhow!(
            "{}",
            tr_fmt(
                lang,
                "unsafe path in the modpack: {0}",
                &[&pack_file.path]
            )
        )
    })?;
    let dest = game_root.join(rel);
    let expected = pack_file.sha1();
    if let Ok(existing) = std::fs::read(&dest) {
        if expected.is_empty() || updater::sha1_hex(&existing) == expected.to_lowercase() {
            return Ok(false); // already there and intact
        }
    }

    // Try the mirrors in order; keep the last error for the final message.
    let mut bytes = None;
    let mut last_err = None;
    for url in &pack_file.downloads {
        match crate::net::get_bytes(agent, url, lang) {
            Ok(b) => {
                bytes = Some(b);
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    let bytes = bytes.ok_or_else(|| {
        last_err.unwrap_or_else(|| {
            anyhow!("{}", tr(lang, "the file has no download URL"))
        })
    })?;
    if !expected.is_empty() {
        let actual = updater::sha1_hex(&bytes);
        if actual != expected.to_lowercase() {
            return Err(anyhow!(
                "{}",
                tr_fmt(
                    lang,
                    "{0}: checksum mismatch (expected {1}, got {2})",
                    &[&pack_file.path, expected, &actual]
                )
            ));
        }
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("mrpack-tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(true)
}

/// Extract the client override layers (`overrides/`, then
/// `client-overrides/`) into the instance root. Later layers overwrite
/// earlier ones; every entry path is validated against escapes.
fn extract_overlays<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    game_root: &Path,
    lang: Language,
) -> Result<usize> {
    let mut count = 0usize;
    for layer in OVERRIDE_LAYERS {
        let prefix = format!("{layer}/");
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .with_context(|| tr(lang, "failed to open the zip archive"))?;
            let Some(rel_raw) = entry.name().strip_prefix(&prefix) else {
                continue;
            };
            if rel_raw.is_empty() {
                continue; // the layer directory entry itself
            }
            let rel = instance_rel_path(rel_raw).ok_or_else(|| {
                anyhow!(
                    "{}",
                    tr_fmt(lang, "unsafe path in the modpack: {0}", &[rel_raw])
                )
            })?;
            let out_path = game_root.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out_path)?;
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = std::fs::File::create(&out_path)?;
                std::io::copy(&mut entry, &mut out)?;
                count += 1;
            }
        }
    }
    Ok(count)
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

    // ── Installation pipeline (offline: synthetic archives, no network) ──

    use std::io::Write as _;
    use zip::write::SimpleFileOptions;

    /// Build a zip archive with the given `(name, content)` entries.
    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for (name, content) in entries {
            writer.start_file(*name, SimpleFileOptions::default()).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap();
    }

    /// A fresh unique temp directory (cleaned by the caller).
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("rustlauncher-modpack-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn overrides_layering_and_server_exclusion() {
        let tmp = temp_dir("overlays");
        let zip_path = tmp.join("pack.mrpack");
        write_zip(
            &zip_path,
            &[
                (INDEX_ENTRY, br#"{"formatVersion":1,"game":"minecraft","name":"x","dependencies":{"minecraft":"1.21.4","fabric-loader":"0.16.9"}}"#),
                ("overrides/a.txt", b"base"),
                ("overrides/sub/c.txt", b"c"),
                ("client-overrides/a.txt", b"client"),
                ("client-overrides/b.txt", b"b"),
                ("server-overrides/a.txt", b"server"),
            ],
        );
        let game_root = tmp.join("instance");
        std::fs::create_dir_all(&game_root).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let count = extract_overlays(&mut archive, &game_root, LANG).unwrap();

        assert_eq!(count, 4); // base a, sub/c, client a, client b
        assert_eq!(std::fs::read(game_root.join("a.txt")).unwrap(), b"client");
        assert_eq!(std::fs::read(game_root.join("b.txt")).unwrap(), b"b");
        assert_eq!(std::fs::read(game_root.join("sub").join("c.txt")).unwrap(), b"c");
        assert!(!game_root.join("server-overrides").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn unsafe_override_entry_is_rejected() {
        let tmp = temp_dir("unsafe-overlay");
        let zip_path = tmp.join("pack.mrpack");
        write_zip(
            &zip_path,
            &[
                (INDEX_ENTRY, br#"{"formatVersion":1,"game":"minecraft","name":"x","dependencies":{"minecraft":"1.21.4","fabric-loader":"0.16.9"}}"#),
                ("overrides/../../evil.txt", b"evil"),
            ],
        );
        let game_root = tmp.join("instance");
        std::fs::create_dir_all(&game_root).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let err = extract_overlays(&mut archive, &game_root, LANG).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err:#}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_index_from_real_archive() {
        let tmp = temp_dir("read-index");
        let zip_path = tmp.join("pack.mrpack");
        write_zip(
            &zip_path,
            &[
                (INDEX_ENTRY, GOOD.as_bytes()),
                ("overrides/keep.txt", b"k"),
            ],
        );
        let index = read_index(&zip_path, LANG).unwrap();
        assert_eq!(index.name, "Test Pack");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_index_reports_a_missing_index() {
        let tmp = temp_dir("no-index");
        let zip_path = tmp.join("pack.mrpack");
        write_zip(&zip_path, &[("other.txt", b"x")]);
        let err = read_index(&zip_path, LANG).unwrap_err();
        assert!(err.to_string().contains("modrinth.index.json"), "{err:#}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn existing_file_with_correct_sha1_is_skipped() {
        let tmp = temp_dir("sha1-skip");
        let game_root = tmp.join("instance");
        std::fs::create_dir_all(game_root.join("mods")).unwrap();
        let jar_path = game_root.join("mods").join("a.jar");
        std::fs::write(&jar_path, b"already installed").unwrap();
        let sha1 = updater::sha1_hex(b"already installed");

        let pack_file = MrpackFile {
            path: "mods/a.jar".into(),
            hashes: BTreeMap::from([("sha1".into(), sha1)]),
            env: None,
            downloads: vec![], // would fail with "no download URL" if not skipped
            file_size: 16,
        };
        let agent = crate::net::agent();
        let downloaded = install_pack_file(&agent, &game_root, &pack_file, LANG).unwrap();
        assert!(!downloaded);
        assert_eq!(std::fs::read(&jar_path).unwrap(), b"already installed");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn unsafe_pack_file_path_is_rejected() {
        let tmp = temp_dir("unsafe-file");
        let game_root = tmp.join("instance");
        std::fs::create_dir_all(&game_root).unwrap();
        let pack_file = MrpackFile {
            path: "../escape.jar".into(),
            hashes: BTreeMap::new(),
            env: None,
            downloads: vec!["https://cdn.modrinth.com/escape.jar".into()],
            file_size: 0,
        };
        let agent = crate::net::agent();
        let err = install_pack_file(&agent, &game_root, &pack_file, LANG).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err:#}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn client_unsupported_files_are_not_installed() {
        let tmp = temp_dir("server-only");
        let game_root = tmp.join("instance");
        std::fs::create_dir_all(&game_root).unwrap();
        let pack_file = MrpackFile {
            path: "mods/server-only.jar".into(),
            hashes: BTreeMap::new(),
            env: Some(MrpackEnv {
                client: Some("unsupported".into()),
                server: Some("required".into()),
            }),
            downloads: vec![], // must never be consulted
            file_size: 0,
        };
        let agent = crate::net::agent();
        let downloaded = install_pack_file(&agent, &game_root, &pack_file, LANG).unwrap();
        assert!(!downloaded);
        assert!(!game_root.join("mods").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
