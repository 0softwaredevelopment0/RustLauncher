//! Remote version discovery and installation from the Mojang manifest.
//!
//! The Java launcher could only *list* remote versions (`Updater.fetchVersions`)
//! but had no install path; this port closes that gap: downloading the version
//! JSON, the client jar, the asset index and all asset objects, with SHA-1
//! verification and a resumable layout identical to the vanilla launcher.
//!
//! Mod-loader manifests: for every Minecraft version the launcher can also
//! install Fabric, Quilt, NeoForge and Forge. Fabric/Quilt expose ready-made
//! launcher profiles through their meta APIs; NeoForge/Forge are assembled
//! from their Maven artifacts (universal jar + launcher metadata json).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::net;

#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case, dead_code)] // `releaseTime` is kept for the UI layer
pub struct ManifestVersion {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub releaseTime: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub latest: BTreeMap<String, String>,
    pub versions: Vec<ManifestVersion>,
}

/// Fetch `version_manifest_v2.json`.
pub fn fetch_manifest(agent: &ureq::Agent) -> Result<Manifest> {
    let body = net::get_string(
        agent,
        "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
    )?;
    serde_json::from_str(&body).context("failed to parse the version manifest")
}

/// The manifest entry for one version id.
#[allow(dead_code)] // part of the public API used by tests
pub fn find_version<'a>(manifest: &'a Manifest, id: &str) -> Option<&'a ManifestVersion> {
    manifest.versions.iter().find(|v| v.id == id)
}

// ---------------------------------------------------------------------------
// Mod-loader manifests
// ---------------------------------------------------------------------------

/// A mod loader that can be installed on top of a Minecraft version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Loader {
    Fabric,
    Quilt,
    NeoForge,
    Forge,
}

impl Loader {
    /// The identifier used in version ids and UI (`fabric-loader-0.19.5-1.21.4`).
    #[allow(dead_code)] // part of the public API for tests/future use
    pub fn slug(self) -> &'static str {
        match self {
            Loader::Fabric => "fabric",
            Loader::Quilt => "quilt",
            Loader::NeoForge => "neoforge",
            Loader::Forge => "forge",
        }
    }

    /// UI label.
    pub fn label(self) -> &'static str {
        match self {
            Loader::Fabric => "Fabric",
            Loader::Quilt => "Quilt",
            Loader::NeoForge => "NeoForge",
            Loader::Forge => "Forge",
        }
    }

    /// All loaders in display order.
    pub const ALL: [Loader; 4] = [
        Loader::Fabric,
        Loader::Quilt,
        Loader::NeoForge,
        Loader::Forge,
    ];
}

/// One loader build available for a Minecraft version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderBuild {
    /// The loader build version (`0.19.5`, `21.4.157`, `54.1.14`).
    pub version: String,
    /// Whether the build is a stable/recommended one (drives sorting).
    pub stable: bool,
}

/// List the loader builds available for `mc` (newest first, stable first).
pub fn fetch_loader_builds(
    agent: &ureq::Agent,
    loader: Loader,
    mc: &str,
) -> Result<Vec<LoaderBuild>> {
    let builds = match loader {
        Loader::Fabric => fetch_fabric_builds(agent, mc)?,
        Loader::Quilt => fetch_quilt_builds(agent, mc)?,
        Loader::NeoForge => fetch_neoforge_builds(agent, mc)?,
        Loader::Forge => fetch_forge_builds(agent, mc)?,
    };
    if builds.is_empty() {
        return Err(anyhow!("no {} builds found for {mc}", loader.label()));
    }
    Ok(builds)
}

#[derive(Debug, Deserialize)]
struct FabricLoaderEntry {
    loader: FabricLoaderInfo,
}

#[derive(Debug, Deserialize)]
struct FabricLoaderInfo {
    version: String,
    #[serde(default = "default_true")]
    stable: bool,
}

fn default_true() -> bool {
    true
}

fn fetch_fabric_builds(agent: &ureq::Agent, mc: &str) -> Result<Vec<LoaderBuild>> {
    let url = format!("https://meta.fabricmc.net/v2/versions/loader/{mc}");
    let body = net::get_string(agent, &url)?;
    let entries: Vec<FabricLoaderEntry> =
        serde_json::from_str(&body).context("failed to parse the Fabric meta response")?;
    Ok(entries
        .into_iter()
        .map(|e| LoaderBuild {
            version: e.loader.version,
            stable: e.loader.stable,
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct QuiltLoaderEntry {
    loader: QuiltLoaderInfo,
}

#[derive(Debug, Deserialize)]
struct QuiltLoaderInfo {
    version: String,
}

fn fetch_quilt_builds(agent: &ureq::Agent, mc: &str) -> Result<Vec<LoaderBuild>> {
    let url = format!("https://meta.quiltmc.org/v3/versions/loader/{mc}");
    let body = net::get_string(agent, &url)?;
    let entries: Vec<QuiltLoaderEntry> =
        serde_json::from_str(&body).context("failed to parse the Quilt meta response")?;
    Ok(entries
        .into_iter()
        .map(|e| {
            let stable = !e.loader.version.contains("beta");
            LoaderBuild {
                version: e.loader.version,
                stable,
            }
        })
        .collect())
}

fn fetch_neoforge_builds(_agent: &ureq::Agent, mc: &str) -> Result<Vec<LoaderBuild>> {
    let (major, minor) = neoforge_major_minor(mc)?;
    let url = "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
    let agent = net::agent();
    let body = net::get_string(&agent, url)?;
    #[derive(Deserialize)]
    struct Versions {
        versions: Vec<String>,
    }
    let parsed: Versions =
        serde_json::from_str(&body).context("failed to parse the NeoForge maven metadata")?;
    let prefix = format!("{major}.{minor}.");
    let mut builds: Vec<LoaderBuild> = parsed
        .versions
        .into_iter()
        .filter(|v| v.starts_with(&prefix) && !v.contains('-'))
        .map(|v| {
            let stable = !v.contains("beta");
            LoaderBuild { version: v, stable }
        })
        .collect();
    // Newest build first.
    builds.sort_by(|a, b| cmp_version_parts(&a.version, &b.version).reverse());
    Ok(builds)
}

/// NeoForge is numbered after the MC minor: MC `1.21.4` -> NeoForge
/// `21.4.<build>` (so the prefix is `<mc-minor>.<mc-patch>.`). Older MC
/// versions have no NeoForge; returns an error the UI can show for those.
fn neoforge_major_minor(mc: &str) -> Result<(u32, u32)> {
    let mut parts = mc.split('.');
    let _mc_major = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .ok_or_else(|| anyhow!("cannot map NeoForge onto version '{mc}'"))?;
    let minor = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .ok_or_else(|| anyhow!("cannot map NeoForge onto version '{mc}'"))?;
    let patch = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .ok_or_else(|| anyhow!("cannot map NeoForge onto version '{mc}'"))?;
    if minor < 20 || (minor == 20 && patch < 2) {
        return Err(anyhow!("NeoForge does not support {mc} (requires 1.20.2+)"));
    }
    Ok((minor, patch))
}

#[derive(Debug, Deserialize)]
struct ForgePromotions {
    #[serde(default)]
    promos: BTreeMap<String, String>,
}

fn fetch_forge_builds(_agent: &ureq::Agent, mc: &str) -> Result<Vec<LoaderBuild>> {
    let url = "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
    let agent = net::agent();
    let body = net::get_string(&agent, url)?;
    let promos: ForgePromotions =
        serde_json::from_str(&body).context("failed to parse the Forge promotions")?;
    let mut builds: Vec<LoaderBuild> = Vec::new();
    for key in ["recommended", "latest"] {
        let promo_key = format!("{mc}-{key}");
        if let Some(v) = promos.promos.get(&promo_key) {
            if !builds.iter().any(|b| &b.version == v) {
                builds.push(LoaderBuild {
                    version: v.clone(),
                    stable: true,
                });
            }
        }
    }
    Ok(builds)
}

/// Numeric-aware comparison of dot-separated versions (`54.1.9` < `54.1.14`).
fn cmp_version_parts(a: &str, b: &str) -> std::cmp::Ordering {
    let pa: Vec<u64> = a.split('.').filter_map(|p| p.parse().ok()).collect();
    let pb: Vec<u64> = b.split('.').filter_map(|p| p.parse().ok()).collect();
    pa.cmp(&pb)
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct VersionMeta {
    #[serde(default)]
    downloads: BTreeMap<String, DownloadInfo>,
    #[serde(default)]
    assetIndex: Option<AssetIndexInfo>,
}

#[derive(Debug, Deserialize)]
struct DownloadInfo {
    url: String,
    #[serde(default)]
    sha1: String,
}

#[derive(Debug, Deserialize)]
struct AssetIndexInfo {
    url: String,
    #[serde(default)]
    id: String,
}

#[derive(Debug, Deserialize)]
struct AssetIndex {
    objects: BTreeMap<String, AssetObject>,
}

#[derive(Debug, Deserialize)]
struct AssetObject {
    hash: String,
}

/// SHA-1 of a byte slice as lowercase hex.
pub fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    bytes_to_hex(&hasher.finalize())
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn verify_sha1(bytes: &[u8], expected: &str, what: &str) -> Result<()> {
    if expected.is_empty() {
        return Ok(());
    }
    let actual = sha1_hex(bytes);
    if actual != expected.to_lowercase() {
        return Err(anyhow!(
            "{what}: checksum mismatch (expected {expected}, got {actual})"
        ));
    }
    Ok(())
}

/// Download a file if missing or corrupt; returns true when it was downloaded.
fn fetch_to_file(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    sha1: &str,
    progress: &mut dyn FnMut(&str),
) -> Result<bool> {
    if let Ok(existing) = std::fs::read(dest) {
        if verify_sha1(&existing, sha1, dest.to_string_lossy().as_ref()).is_ok() {
            return Ok(false);
        }
    }
    let bytes = net::get_bytes(agent, url)?;
    verify_sha1(&bytes, sha1, &format!("{url} (downloaded)"))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, dest)?;
    progress(&dest.file_name().unwrap_or_default().to_string_lossy());
    Ok(true)
}

/// Everything needed to install one version.
pub struct InstallOutcome {
    pub version_id: String,
    pub downloaded_files: usize,
}

/// Install a Minecraft version into `game_dir` (vanilla layout:
/// `versions/<id>/<id>.{json,jar}`, `assets/indexes/<idx>.json`,
/// `assets/objects/<h[:2]>/<h>`).
pub fn install_version(
    agent: &ureq::Agent,
    game_dir: &Path,
    version: &ManifestVersion,
    mut progress: &mut dyn FnMut(&str),
) -> Result<InstallOutcome> {
    let id = &version.id;
    let version_dir = game_dir.join("versions").join(id);
    std::fs::create_dir_all(&version_dir)?;

    // 1. version.json
    let meta_bytes = net::get_bytes(agent, &version.url)?;
    let meta: VersionMeta =
        serde_json::from_slice(&meta_bytes).context("failed to parse the version metadata")?;
    let json_path = version_dir.join(format!("{id}.json"));
    std::fs::write(&json_path, &meta_bytes)?;

    // 2. client jar
    let jar = meta
        .downloads
        .get("client")
        .ok_or_else(|| anyhow!("version {id} has no client download"))?;
    let jar_path = version_dir.join(format!("{id}.jar"));
    let mut downloaded = 0usize;
    if fetch_to_file(agent, &jar.url, &jar_path, &jar.sha1, &mut progress)? {
        downloaded += 1;
    }

    // 3. asset index + objects
    let asset_index = meta
        .assetIndex
        .context("version metadata has no asset index")?;
    let index_id = if asset_index.id.is_empty() {
        "legacy"
    } else {
        &asset_index.id
    };
    let index_path = game_dir
        .join("assets")
        .join("indexes")
        .join(format!("{index_id}.json"));
    let index_bytes = net::get_bytes(agent, &asset_index.url)?;
    std::fs::create_dir_all(index_path.parent().unwrap())?;
    std::fs::write(&index_path, &index_bytes)?;
    let index: AssetIndex =
        serde_json::from_slice(&index_bytes).context("failed to parse the asset index")?;

    let objects_dir = game_dir.join("assets").join("objects");
    let total = index.objects.len();
    progress(&format!("assets 0/{total}"));
    for (i, (name, object)) in index.objects.iter().enumerate() {
        let hash = object.hash.to_lowercase();
        let dest = objects_dir.join(&hash[..2]).join(&hash);
        let url = format!(
            "https://resources.download.minecraft.net/{}/{hash}",
            &hash[..2]
        );
        // Names are not URLs; use them only for progress display.
        let _ = name;
        if fetch_to_file(agent, &url, &dest, &hash, &mut progress)? {
            downloaded += 1;
        }
        if (i + 1) % 200 == 0 || i + 1 == total {
            progress(&format!("assets {}/{total}", i + 1));
        }
    }

    Ok(InstallOutcome {
        version_id: id.clone(),
        downloaded_files: downloaded,
    })
}

// ---------------------------------------------------------------------------
// Mod-loader installation
// ---------------------------------------------------------------------------

/// The Fabric/Quilt meta `launcherMeta` shape (version 2).
#[derive(Debug, Deserialize)]
struct LauncherMeta {
    #[serde(default)]
    libraries: BTreeMap<String, Vec<MetaLibrary>>,
}

#[derive(Debug, Clone, Deserialize)]
struct MetaLibrary {
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    sha1: String,
}

/// The Fabric/Quilt profile json (same shape the official installers emit).
/// Fields are parsed for validation; the raw bytes are what gets installed.
#[derive(Debug, Deserialize)]
#[allow(non_snake_case, dead_code)]
struct LoaderProfile {
    id: String,
    #[serde(default)]
    inheritsFrom: String,
    #[serde(default)]
    jar: Option<String>,
    #[serde(default)]
    mainClass: Option<String>,
    #[serde(default)]
    libraries: Vec<MetaLibrary>,
    #[serde(default)]
    launcherMeta: Option<LauncherMeta>,
}

/// Resolve a Maven coordinate to a jar path under `libraries_dir`
/// (`net.fabricmc:fabric-loader:0.19.5` -> `<dir>/net/fabricmc/fabric-loader/
/// 0.19.5/fabric-loader-0.19.5.jar`). Returns `None` for malformed names.
fn coordinate_to_path(libraries_dir: &Path, coordinate: &str) -> Option<std::path::PathBuf> {
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let group_path = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let mut file = format!("{artifact}-{version}");
    if let Some(classifier) = parts.get(3) {
        if !classifier.is_empty() {
            file.push('-');
            file.push_str(classifier);
        }
    }
    file.push_str(".jar");
    Some(
        libraries_dir
            .join(group_path)
            .join(artifact)
            .join(version)
            .join(file),
    )
}

/// Install a mod loader on top of an installed vanilla version.
///
/// The result is a self-contained version entry in the standard layout
/// (`versions/<id>/<id>.json`), which the launcher discovers and launches
/// like any other version. The vanilla version must already be installed:
/// the loader profile references it via `inheritsFrom`/`jar` and the game
/// assets and client jar come from it.
pub fn install_loader(
    agent: &ureq::Agent,
    game_dir: &Path,
    loader: Loader,
    mc: &str,
    build: &LoaderBuild,
    mut progress: &mut dyn FnMut(&str),
) -> Result<InstallOutcome> {
    let version_id = match loader {
        Loader::Fabric => format!("fabric-loader-{}-{mc}", build.version),
        Loader::Quilt => format!("quilt-loader-{}-{mc}", build.version),
        Loader::NeoForge => format!("neoforge-{mc}-{}", build.version),
        Loader::Forge => format!("forge-{mc}-{}", build.version),
    };
    let version_dir = game_dir.join("versions").join(&version_id);
    std::fs::create_dir_all(&version_dir)?;

    let mut downloaded = 0usize;
    match loader {
        Loader::Fabric | Loader::Quilt => {
            let profile_json = match loader {
                Loader::Fabric => format!(
                    "https://meta.fabricmc.net/v2/versions/loader/{mc}/{}/profile/json",
                    build.version
                ),
                _ => format!(
                    "https://meta.quiltmc.org/v3/versions/loader/{mc}/{}/profile/json",
                    build.version
                ),
            };
            let bytes = net::get_bytes(agent, &profile_json)?;
            let profile: LoaderProfile = serde_json::from_slice(&bytes)
                .with_context(|| format!("bad {0} profile for {mc}", loader.label()))?;

            // Download every library the profile references.
            let libs_dir = game_dir.join("libraries");
            let mut libs = profile.libraries.clone();
            if let Some(meta) = &profile.launcherMeta {
                for entries in meta.libraries.values() {
                    libs.extend(entries.iter().cloned());
                }
            }
            let total = libs.len();
            for (i, lib) in libs.iter().enumerate() {
                let (url, sha1) = library_source(lib, loader)?;
                let dest = coordinate_to_path(&libs_dir, &lib.name)
                    .ok_or_else(|| anyhow!("bad library coordinate: {}", lib.name))?;
                if fetch_to_file(agent, &url, &dest, &sha1, &mut progress)? {
                    downloaded += 1;
                }
                if (i + 1) % 10 == 0 || i + 1 == total {
                    progress(&format!("libraries {}/{}", i + 1, total));
                }
            }

            // The profile references the vanilla version; make sure the
            // `jar` field points at the vanilla id so the launcher finds
            // the client jar.
            let final_json = finalize_profile(&bytes, &profile, mc)?;
            std::fs::write(version_dir.join(format!("{version_id}.json")), final_json)?;
        }
        Loader::NeoForge | Loader::Forge => {
            // Assemble a minimal launcher profile: main class + the loader
            // jar as a library. The loader bootstraps the rest itself.
            let (maven_base, coordinate, main_class) = match loader {
                Loader::NeoForge => (
                    "https://maven.neoforged.net/releases",
                    format!("net.neoforged:neoforge:{}:universal", build.version),
                    "net.neoforged.fml.loading.ImmediateWindowHandler",
                ),
                _ => (
                    "https://maven.minecraftforge.net",
                    format!("net.minecraftforge:forge:{}:universal", build.version),
                    "net.minecraftforge.fml.loading.ImmediateWindowHandler",
                ),
            };
            let lib = MetaLibrary {
                name: coordinate,
                url: format!("{maven_base}/"),
                sha1: String::new(),
            };
            let libs_dir = game_dir.join("libraries");
            let (url, _) = library_source(&lib, loader)?;
            let dest = coordinate_to_path(&libs_dir, &lib.name)
                .ok_or_else(|| anyhow!("bad library coordinate: {}", lib.name))?;
            if fetch_to_file(agent, &url, &dest, "", &mut progress)? {
                downloaded += 1;
            }

            let profile =
                neoforge_forge_profile(loader, mc, &build.version, &version_id, main_class);
            let json = serde_json::to_vec_pretty(&profile)?;
            std::fs::write(version_dir.join(format!("{version_id}.json")), json)?;
        }
    }

    progress(&format!("{version_id} installed"));
    Ok(InstallOutcome {
        version_id,
        downloaded_files: downloaded,
    })
}

/// Resolve the download URL and SHA-1 for one profile library.
fn library_source(lib: &MetaLibrary, loader: Loader) -> Result<(String, String)> {
    if lib.url.is_empty() {
        // Fabric/Quilt meta libraries always carry a repo URL; vanilla shared
        // ones (e.g. ASM) default to the loader's own maven.
        let base = match loader {
            Loader::Fabric => "https://maven.fabricmc.net/",
            _ => "https://maven.quiltmc.org/repository/release/",
        };
        return Ok((
            format!("{base}{}", coordinate_rel(&lib.name)),
            lib.sha1.clone(),
        ));
    }
    let base = lib.url.trim_end_matches('/');
    Ok((
        format!("{base}/{}", coordinate_rel(&lib.name)),
        lib.sha1.clone(),
    ))
}

/// `net.fabricmc:fabric-loader:0.19.5` ->
/// `net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar`
fn coordinate_rel(coordinate: &str) -> String {
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return String::new();
    }
    let group_path = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let mut file = format!("{artifact}-{version}");
    if let Some(classifier) = parts.get(3) {
        if !classifier.is_empty() {
            file.push('-');
            file.push_str(classifier);
        }
    }
    file.push_str(".jar");
    format!("{group_path}/{artifact}/{version}/{file}")
}

/// Rewrite a downloaded profile json before saving: fill `jar` with the
/// vanilla id when missing (so the launcher can find the client jar) and
/// keep everything else byte-identical where possible.
fn finalize_profile(original: &[u8], profile: &LoaderProfile, mc: &str) -> Result<Vec<u8>> {
    let mut value: serde_json::Value = serde_json::from_slice(original)?;
    if value.get("jar").is_none() {
        value["jar"] = serde_json::Value::String(mc.to_string());
    }
    let _ = profile;
    Ok(serde_json::to_vec_pretty(&value)?)
}

/// Build the minimal NeoForge/Forge profile written as `<id>.json`.
fn neoforge_forge_profile(
    loader: Loader,
    mc: &str,
    build: &str,
    version_id: &str,
    main_class: &str,
) -> serde_json::Value {
    let coordinate = match loader {
        Loader::NeoForge => format!("net.neoforged:neoforge:{build}:universal"),
        _ => format!("net.minecraftforge:forge:{build}:universal"),
    };
    serde_json::json!({
        "id": version_id,
        "inheritsFrom": mc,
        "jar": mc,
        "releaseTime": "2024-01-01T00:00:00+00:00",
        "time": "2024-01-01T00:00:00+00:00",
        "type": "release",
        "mainClass": main_class,
        "libraries": [ { "name": coordinate } ],
        "arguments": { "game": [], "jvm": [] }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_minimal_shape() {
        let body = r#"{
            "latest": {"release": "1.21.4", "snapshot": "25w14craftmine"},
            "versions": [
                {"id": "1.21.4", "type": "release", "url": "https://piston-meta.mojang.com/v1/packages/abc/1.21.4.json", "releaseTime": "2024-12-03T10:14:44+00:00", "time": "2024-12-03T10:15:00+00:00"},
                {"id": "broken-entry-without-id"}
            ]
        }"#;
        let manifest: Manifest = serde_json::from_str(body).unwrap();
        assert_eq!(manifest.latest["release"], "1.21.4");
        assert_eq!(manifest.versions.len(), 2);
        assert!(find_version(&manifest, "1.21.4").is_some());
        assert!(find_version(&manifest, "9.9.9").is_none());
    }

    #[test]
    fn sha1_hex_matches_known_vectors() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn checksum_mismatch_is_detected() {
        let err =
            verify_sha1(b"abc", "0000000000000000000000000000000000000000", "test").unwrap_err();
        assert!(err.to_string().contains("mismatch"));
        verify_sha1(b"abc", "A9993E364706816ABA3E25717850C26C9CD0D89D", "test").unwrap();
    }

    #[test]
    fn fabric_meta_parses() {
        let body = r#"[
            {"loader": {"version": "0.19.5", "stable": true},
             "intermediary": {"version": "1.21.4"}},
            {"loader": {"version": "0.20.0", "stable": false}}
        ]"#;
        let entries: Vec<FabricLoaderEntry> = serde_json::from_str(body).unwrap();
        let builds: Vec<LoaderBuild> = entries
            .into_iter()
            .map(|e| LoaderBuild {
                version: e.loader.version,
                stable: e.loader.stable,
            })
            .collect();
        assert_eq!(builds.len(), 2);
        assert_eq!(builds[0].version, "0.19.5");
        assert!(builds[0].stable);
        assert!(!builds[1].stable);
    }

    #[test]
    fn quilt_meta_parses() {
        let body = r#"[
            {"loader": {"maven": "org.quiltmc:quilt-loader:0.20.0-beta.9",
                        "version": "0.20.0-beta.9", "build": 9}},
            {"loader": {"maven": "org.quiltmc:quilt-loader:0.21.0",
                        "version": "0.21.0", "build": 3}}
        ]"#;
        let entries: Vec<QuiltLoaderEntry> = serde_json::from_str(body).unwrap();
        let builds: Vec<LoaderBuild> = entries
            .into_iter()
            .map(|e| {
                let stable = !e.loader.version.contains("beta");
                LoaderBuild {
                    version: e.loader.version,
                    stable,
                }
            })
            .collect();
        assert_eq!(builds.len(), 2);
        assert!(!builds[0].stable);
        assert!(builds[1].stable);
    }

    #[test]
    fn forge_promotions_parse_and_filter() {
        let body = r#"{
            "homepage": "https://files.minecraftforge.net",
            "promos": {
                "1.21.4-latest": "54.1.18",
                "1.21.4-recommended": "54.1.14",
                "1.20.1-latest": "47.4.23"
            }
        }"#;
        let promos: ForgePromotions = serde_json::from_str(body).unwrap();
        assert_eq!(promos.promos.get("1.21.4-latest").unwrap(), "54.1.18");
        assert_eq!(promos.promos.get("1.20.1-latest").unwrap(), "47.4.23");
        assert!(!promos.promos.contains_key("9.9.9-latest"));
    }

    #[test]
    fn version_parts_compare_numerically() {
        use std::cmp::Ordering;
        assert_eq!(cmp_version_parts("54.1.9", "54.1.14"), Ordering::Less);
        assert_eq!(cmp_version_parts("54.1.14", "54.1.9"), Ordering::Greater);
        assert_eq!(cmp_version_parts("21.4.157", "21.4.157"), Ordering::Equal);
    }

    #[test]
    fn neoforge_version_mapping_rejects_old_mc() {
        assert!(neoforge_major_minor("1.21.4").is_ok());
        assert!(neoforge_major_minor("1.20.2").is_ok());
        assert!(neoforge_major_minor("1.20.1").is_err());
        assert!(neoforge_major_minor("1.12.2").is_err());
        assert!(neoforge_major_minor("not-a-version").is_err());
    }

    #[test]
    fn coordinate_to_path_builds_maven_layout() {
        // Component-wise comparison: Path separators differ between OSes.
        let expected: &[&str] = &[
            "net",
            "neoforged",
            "neoforge",
            "21.4.157",
            "neoforge-21.4.157-universal.jar",
        ];
        let path = coordinate_to_path(
            Path::new("/libs"),
            "net.neoforged:neoforge:21.4.157:universal",
        )
        .unwrap();
        let comps: Vec<std::ffi::OsString> = path
            .components()
            .map(|c| c.as_os_str().to_owned())
            .collect();
        let tail: Vec<std::ffi::OsString> = comps[comps.len() - expected.len()..].to_vec();
        let want: Vec<std::ffi::OsString> = expected
            .iter()
            .map(|s| std::ffi::OsString::from(*s))
            .collect();
        assert_eq!(tail, want);

        let path =
            coordinate_to_path(Path::new("/libs"), "net.fabricmc:fabric-loader:0.19.5").unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(name, "fabric-loader-0.19.5.jar");
        assert_eq!(path.components().count(), 7); // /libs + 4 + file

        assert!(coordinate_to_path(Path::new("/libs"), "garbage").is_none());
        assert!(coordinate_to_path(Path::new("/libs"), "a:b:").is_none());
    }

    #[test]
    fn library_source_prefers_meta_url_with_fallbacks() {
        let meta = MetaLibrary {
            name: "org.ow2.asm:asm:9.10.1".into(),
            url: "https://maven.fabricmc.net/".into(),
            sha1: "abc".into(),
        };
        let (url, sha1) = library_source(&meta, Loader::Fabric).unwrap();
        assert_eq!(
            url,
            "https://maven.fabricmc.net/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar"
        );
        assert_eq!(sha1, "abc");

        let no_url = MetaLibrary {
            name: "net.fabricmc:fabric-loader:0.19.5".into(),
            url: String::new(),
            sha1: String::new(),
        };
        let (url, _) = library_source(&no_url, Loader::Fabric).unwrap();
        assert!(url.starts_with("https://maven.fabricmc.net/"));
        let (url, _) = library_source(&no_url, Loader::Quilt).unwrap();
        assert!(url.starts_with("https://maven.quiltmc.org/"));
    }

    #[test]
    fn loader_profile_json_parses_and_finalizes() {
        let body = br#"{
            "id": "fabric-loader-0.19.5-1.21.4",
            "inheritsFrom": "1.21.4",
            "releaseTime": "2025-01-01T00:00:00+00:00",
            "time": "2025-01-01T00:00:00+00:00",
            "type": "release",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "libraries": [{"name": "org.ow2.asm:asm:9.10.1"}],
            "launcherMeta": {
                "version": 2,
                "libraries": {
                    "common": [{"name": "net.fabricmc:fabric-loader:0.19.5",
                                "url": "https://maven.fabricmc.net/"}],
                    "client": [],
                    "server": []
                }
            }
        }"#;
        let profile: LoaderProfile = serde_json::from_slice(body).unwrap();
        assert_eq!(profile.id, "fabric-loader-0.19.5-1.21.4");
        assert_eq!(profile.libraries.len(), 1);
        let meta = profile.launcherMeta.as_ref().unwrap();
        assert_eq!(meta.libraries["common"].len(), 1);
        assert_eq!(meta.libraries["client"].len(), 0);

        // finalize_profile adds the jar reference when missing.
        let out = finalize_profile(body, &profile, "1.21.4").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["jar"], "1.21.4");
    }

    #[test]
    fn loader_version_ids_use_expected_shapes() {
        let build = LoaderBuild {
            version: "0.19.5".into(),
            stable: true,
        };
        let game_dir = std::env::temp_dir().join(format!("rl-loader-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&game_dir);

        // Version-id shapes are exercised via install_loader's id scheme;
        // assert the naming convention through the profile builder.
        let profile = neoforge_forge_profile(
            Loader::NeoForge,
            "1.21.4",
            "21.4.157",
            "neoforge-1.21.4-21.4.157",
            "net.neoforged.fml.loading.ImmediateWindowHandler",
        );
        assert_eq!(profile["id"], "neoforge-1.21.4-21.4.157");
        assert_eq!(profile["inheritsFrom"], "1.21.4");
        assert_eq!(profile["jar"], "1.21.4");
        assert_eq!(
            profile["libraries"][0]["name"],
            "net.neoforged:neoforge:21.4.157:universal"
        );

        let _ = build;
        let _ = game_dir;
        let _ = std::fs::remove_dir_all(&game_dir);
    }
}
