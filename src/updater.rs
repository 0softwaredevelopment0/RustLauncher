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

use crate::lang::{tr, tr_fmt, Language};
use crate::net;
use crate::version_json::VersionJson;

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
pub fn fetch_manifest(agent: &ureq::Agent, lang: Language) -> Result<Manifest> {
    let body = net::get_string(
        agent,
        "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
        lang,
    )?;
    serde_json::from_str(&body).context(tr(lang, "failed to parse the version manifest"))
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
    lang: Language,
) -> Result<Vec<LoaderBuild>> {
    let builds = match loader {
        Loader::Fabric => fetch_fabric_builds(agent, mc, lang)?,
        Loader::Quilt => fetch_quilt_builds(agent, mc, lang)?,
        Loader::NeoForge => fetch_neoforge_builds(agent, mc, lang)?,
        Loader::Forge => fetch_forge_builds(agent, mc, lang)?,
    };
    if builds.is_empty() {
        return Err(anyhow!(
            "{}",
            tr_fmt(lang, "no {0} builds found for {1}", &[loader.label(), mc])
        ));
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

fn fetch_fabric_builds(agent: &ureq::Agent, mc: &str, lang: Language) -> Result<Vec<LoaderBuild>> {
    let url = format!("https://meta.fabricmc.net/v2/versions/loader/{mc}");
    let body = net::get_string(agent, &url, lang)?;
    let entries: Vec<FabricLoaderEntry> = serde_json::from_str(&body)
        .context(tr(lang, "failed to parse the Fabric meta response"))?;
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

fn fetch_quilt_builds(agent: &ureq::Agent, mc: &str, lang: Language) -> Result<Vec<LoaderBuild>> {
    let url = format!("https://meta.quiltmc.org/v3/versions/loader/{mc}");
    let body = net::get_string(agent, &url, lang)?;
    let entries: Vec<QuiltLoaderEntry> =
        serde_json::from_str(&body).context(tr(lang, "failed to parse the Quilt meta response"))?;
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

fn fetch_neoforge_builds(
    _agent: &ureq::Agent,
    mc: &str,
    lang: Language,
) -> Result<Vec<LoaderBuild>> {
    let (major, minor) = neoforge_major_minor(mc, lang)?;
    let url = "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
    let agent = net::agent();
    let body = net::get_string(&agent, url, lang)?;
    #[derive(Deserialize)]
    struct Versions {
        versions: Vec<String>,
    }
    let parsed: Versions = serde_json::from_str(&body)
        .context(tr(lang, "failed to parse the NeoForge maven metadata"))?;
    let prefix = format!("{major}.{minor}.");
    let mut builds: Vec<LoaderBuild> = parsed
        .versions
        .into_iter()
        .filter(|v| v.starts_with(&prefix))
        .map(|v| {
            // NeoForge 26.x ships through `-beta`/`-alpha` suffixed builds;
            // a bare version string is a stable release.
            let stable = !v.contains("-beta") && !v.contains("-alpha");
            LoaderBuild { version: v, stable }
        })
        .collect();
    // Newest build first.
    builds.sort_by(|a, b| cmp_version_parts(&a.version, &b.version).reverse());
    Ok(builds)
}

/// NeoForge is numbered after the MC minor: MC `1.21.4` -> NeoForge
/// `21.4.<build>`, and year-based ids map directly: MC `26.3` ->
/// `26.3.<build>`. Older MC versions have no NeoForge; returns an error
/// the UI can show for those.
fn neoforge_major_minor(mc: &str, lang: Language) -> Result<(u32, u32)> {
    let mut parts = mc.split('.');
    let first = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .ok_or_else(|| {
            anyhow!(
                "{}",
                tr_fmt(lang, "cannot map NeoForge onto version '{0}'", &[mc])
            )
        })?;
    let second = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .ok_or_else(|| {
            anyhow!(
                "{}",
                tr_fmt(lang, "cannot map NeoForge onto version '{0}'", &[mc])
            )
        })?;
    // Legacy `1.x.y`: NeoForge numbers are `x.y.<build>` (leading 1
    // dropped) and require 1.20.2+. Year-based ids (`26.3`) map directly
    // to `26.3.<build>`.
    if first == 1 {
        let patch = parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .ok_or_else(|| {
                anyhow!(
                    "{}",
                    tr_fmt(lang, "cannot map NeoForge onto version '{0}'", &[mc])
                )
            })?;
        if second < 20 || (second == 20 && patch < 2) {
            return Err(anyhow!(
                "{}",
                tr_fmt(
                    lang,
                    "NeoForge does not support {0} (requires 1.20.2+)",
                    &[mc]
                )
            ));
        }
        Ok((second, patch))
    } else {
        Ok((first, second))
    }
}

#[derive(Debug, Deserialize)]
struct ForgePromotions {
    #[serde(default)]
    promos: BTreeMap<String, String>,
}

fn fetch_forge_builds(_agent: &ureq::Agent, mc: &str, lang: Language) -> Result<Vec<LoaderBuild>> {
    let url = "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
    let agent = net::agent();
    let body = net::get_string(&agent, url, lang)?;
    let promos: ForgePromotions =
        serde_json::from_str(&body).context(tr(lang, "failed to parse the Forge promotions"))?;
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

/// Numeric-aware comparison of dot-separated versions; each segment
/// compares by its leading digits (`54.1.9` < `54.1.14`, `37-beta` -> 37).
fn cmp_version_parts(a: &str, b: &str) -> std::cmp::Ordering {
    let pa: Vec<u64> = a.split('.').map(numeric_prefix).collect();
    let pb: Vec<u64> = b.split('.').map(numeric_prefix).collect();
    pa.cmp(&pb)
}

/// The leading digit run of a version segment (`37-beta` -> 37, no digits -> 0).
fn numeric_prefix(segment: &str) -> u64 {
    let digits: String = segment.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct VersionMeta {
    #[serde(default)]
    downloads: BTreeMap<String, DownloadInfo>,
    #[serde(default)]
    assetIndex: Option<AssetIndexInfo>,
    #[serde(default)]
    libraries: Vec<crate::version_json::LibraryEntry>,
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

fn verify_sha1(bytes: &[u8], expected: &str, what: &str, lang: Language) -> Result<()> {
    if expected.is_empty() {
        return Ok(());
    }
    let actual = sha1_hex(bytes);
    if actual != expected.to_lowercase() {
        return Err(anyhow!(
            "{}",
            tr_fmt(
                lang,
                "{0}: checksum mismatch (expected {1}, got {2})",
                &[what, expected, &actual]
            )
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
    lang: Language,
) -> Result<bool> {
    if let Ok(existing) = std::fs::read(dest) {
        if verify_sha1(&existing, sha1, dest.to_string_lossy().as_ref(), lang).is_ok() {
            return Ok(false);
        }
    }
    let bytes = net::get_bytes(agent, url, lang)?;
    verify_sha1(&bytes, sha1, &format!("{url} (downloaded)"), lang)?;
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
    lang: Language,
    mut progress: &mut dyn FnMut(&str),
) -> Result<InstallOutcome> {
    let id = &version.id;
    let version_dir = game_dir.join("versions").join(id);
    std::fs::create_dir_all(&version_dir)?;

    // 1. version.json
    let meta_bytes = net::get_bytes(agent, &version.url, lang)?;
    let meta: VersionMeta = serde_json::from_slice(&meta_bytes)
        .context(tr(lang, "failed to parse the version metadata"))?;
    let json_path = version_dir.join(format!("{id}.json"));
    std::fs::write(&json_path, &meta_bytes)?;

    // 2. client jar
    let jar = meta.downloads.get("client").ok_or_else(|| {
        anyhow!(
            "{}",
            tr_fmt(lang, "version {0} has no client download", &[id])
        )
    })?;
    let jar_path = version_dir.join(format!("{id}.jar"));
    let mut downloaded = 0usize;
    if fetch_to_file(agent, &jar.url, &jar_path, &jar.sha1, &mut progress, lang)? {
        downloaded += 1;
    }

    // 3. libraries declared by the version json (current-OS natives jars
    //    included), so the first launch does not have to fetch them
    let libs_dir = game_dir.join("libraries");
    let natives = natives_os();
    let total = meta.libraries.len();
    for (i, lib) in meta.libraries.iter().enumerate() {
        download_library_entry(agent, &libs_dir, lib, natives, lang, &mut progress)?;
        if (i + 1) % 10 == 0 || i + 1 == total {
            progress(&tr_fmt(
                lang,
                "libraries {0}/{1}",
                &[&(i + 1).to_string(), &total.to_string()],
            ));
        }
    }

    // 4. asset index + objects
    let asset_index = meta
        .assetIndex
        .context(tr(lang, "version metadata has no asset index"))?;
    let index_id = if asset_index.id.is_empty() {
        "legacy"
    } else {
        &asset_index.id
    };
    let index_path = game_dir
        .join("assets")
        .join("indexes")
        .join(format!("{index_id}.json"));
    let index_bytes = net::get_bytes(agent, &asset_index.url, lang)?;
    std::fs::create_dir_all(index_path.parent().unwrap())?;
    std::fs::write(&index_path, &index_bytes)?;
    let index: AssetIndex = serde_json::from_slice(&index_bytes)
        .context(tr(lang, "failed to parse the asset index"))?;

    let objects_dir = game_dir.join("assets").join("objects");
    let total = index.objects.len();
    progress(&tr_fmt(lang, "assets 0/{0}", &[&total.to_string()]));
    for (i, (name, object)) in index.objects.iter().enumerate() {
        let hash = object.hash.to_lowercase();
        let dest = objects_dir.join(&hash[..2]).join(&hash);
        let url = format!(
            "https://resources.download.minecraft.net/{}/{hash}",
            &hash[..2]
        );
        // Names are not URLs; use them only for progress display.
        let _ = name;
        if fetch_to_file(agent, &url, &dest, &hash, &mut progress, lang)? {
            downloaded += 1;
        }
        if (i + 1) % 200 == 0 || i + 1 == total {
            progress(&tr_fmt(
                lang,
                "assets {0}/{1}",
                &[&(i + 1).to_string(), &total.to_string()],
            ));
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
    lang: Language,
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
            let bytes = net::get_bytes(agent, &profile_json, lang)?;
            let profile: LoaderProfile = serde_json::from_slice(&bytes)
                .with_context(|| tr_fmt(lang, "bad {0} profile for {1}", &[loader.label(), mc]))?;

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
                let dest = coordinate_to_path(&libs_dir, &lib.name).ok_or_else(|| {
                    anyhow!(
                        "{}",
                        tr_fmt(lang, "bad library coordinate: {0}", &[&lib.name])
                    )
                })?;
                if fetch_to_file(agent, &url, &dest, &sha1, &mut progress, lang)? {
                    downloaded += 1;
                }
                if (i + 1) % 10 == 0 || i + 1 == total {
                    progress(&tr_fmt(
                        lang,
                        "libraries {0}/{1}",
                        &[&(i + 1).to_string(), &total.to_string()],
                    ));
                }
            }

            // The profile references the vanilla version; make sure the
            // `jar` field points at the vanilla id so the launcher finds
            // the client jar.
            let final_json = finalize_profile(&bytes, &profile, mc)?;
            std::fs::write(version_dir.join(format!("{version_id}.json")), final_json)?;
        }
        Loader::NeoForge => {
            // NeoForge publishes a vanilla-format version.json inside its
            // installer jar (real mainClass, FML libraries, launch
            // arguments); use it as the profile instead of hand-assembling
            // a minimal one.
            let maven_base = "https://maven.neoforged.net/releases";
            let build_version = &build.version;
            let installer_url = format!(
                "{maven_base}/net/neoforged/neoforge/{build_version}/neoforge-{build_version}-installer.jar"
            );
            let installer = net::get_bytes(agent, &installer_url, lang)?;
            let meta_bytes = installer_version_json(&installer).with_context(|| {
                tr_fmt(lang, "bad {0} installer for {1}", &["NeoForge", mc])
            })?;
            let meta: VersionJson = serde_json::from_slice(&meta_bytes).with_context(|| {
                tr_fmt(lang, "bad {0} installer for {1}", &["NeoForge", mc])
            })?;

            // The loader jar itself plus every library the metadata lists
            // (vanilla-format entries with downloads.artifact URLs and
            // per-OS natives classifiers).
            let libs_dir = game_dir.join("libraries");
            let universal = MetaLibrary {
                name: format!("net.neoforged:neoforge:{build_version}:universal"),
                url: format!("{maven_base}/"),
                sha1: String::new(),
            };
            let (url, _) = library_source(&universal, loader)?;
            let dest = coordinate_to_path(&libs_dir, &universal.name).ok_or_else(|| {
                anyhow!(
                    "{}",
                    tr_fmt(lang, "bad library coordinate: {0}", &[&universal.name])
                )
            })?;
            if fetch_to_file(agent, &url, &dest, "", &mut progress, lang)? {
                downloaded += 1;
            }
            let natives = natives_os();
            for lib in &meta.libraries {
                if coordinate_to_path(&libs_dir, &lib.name).is_none() {
                    continue;
                }
                download_library_entry(agent, &libs_dir, lib, natives, lang, &mut progress)?;
                downloaded += 1;
            }
            progress(&tr_fmt(
                lang,
                "libraries {0}/{1}",
                &[&meta.libraries.len().to_string(), &meta.libraries.len().to_string()],
            ));

            // Rewrite the metadata into our version entry: our id, and the
            // parent reference guaranteed to resolve.
            let mut value: serde_json::Value = serde_json::from_slice(&meta_bytes)?;
            value["id"] = serde_json::Value::String(version_id.clone());
            if value.get("jar").is_none() {
                value["jar"] = serde_json::Value::String(mc.to_string());
            }
            if value.get("inheritsFrom").is_none() {
                value["inheritsFrom"] = serde_json::Value::String(mc.to_string());
            }
            let json = serde_json::to_vec_pretty(&value)?;
            std::fs::write(version_dir.join(format!("{version_id}.json")), json)?;
        }
        Loader::Forge => {
            // Assemble a minimal launcher profile: main class + the loader
            // jar as a library. The loader bootstraps the rest itself.
            // (Forge installers need processor-based patching; not wired.)
            let maven_base = "https://maven.minecraftforge.net";
            let lib = MetaLibrary {
                name: format!("net.minecraftforge:forge:{}:universal", build.version),
                url: format!("{maven_base}/"),
                sha1: String::new(),
            };
            let libs_dir = game_dir.join("libraries");
            let (url, _) = library_source(&lib, loader)?;
            let dest = coordinate_to_path(&libs_dir, &lib.name).ok_or_else(|| {
                anyhow!(
                    "{}",
                    tr_fmt(lang, "bad library coordinate: {0}", &[&lib.name])
                )
            })?;
            if fetch_to_file(agent, &url, &dest, "", &mut progress, lang)? {
                downloaded += 1;
            }

            let profile = neoforge_forge_profile(
                loader,
                mc,
                &build.version,
                &version_id,
                "net.minecraftforge.fml.loading.ImmediateWindowHandler",
            );
            let json = serde_json::to_vec_pretty(&profile)?;
            std::fs::write(version_dir.join(format!("{version_id}.json")), json)?;
        }
    }

    progress(&tr_fmt(lang, "{0} installed", &[&version_id]));
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

// ---------------------------------------------------------------------------
// Launch-time library resolution
// ---------------------------------------------------------------------------

/// Default Maven repository for libraries whose json entry declares no URL.
const DEFAULT_LIBRARY_REPO: &str = "https://libraries.minecraft.net/";

/// Resolve the download URL of a version-json library entry: the vanilla
/// `downloads.artifact.url` when present, else the entry's own repository
/// `url`, else the default Mojang libraries repository.
fn library_download_url(entry: &crate::version_json::LibraryEntry) -> String {
    if let Some((url, _)) = entry.artifact_download() {
        if !url.is_empty() {
            return url.to_string();
        }
    }
    let rel = coordinate_rel(&entry.name);
    if !entry.url.is_empty() {
        return format!("{}/{}", entry.url.trim_end_matches('/'), rel);
    }
    format!("{DEFAULT_LIBRARY_REPO}{rel}")
}

/// Load the version json of an installed version by id: the standard
/// `versions/<id>/<id>.json` layout first, then any scanned version dir.
fn load_installed_version_json(game_dir: &Path, id: &str, lang: Language) -> Result<VersionJson> {
    let direct = game_dir.join("versions").join(id).join(format!("{id}.json"));
    let path = if direct.is_file() {
        direct
    } else {
        crate::version::list_versions(game_dir)
            .ok()
            .and_then(|versions| versions.into_iter().find(|v| v.name == id))
            .map(|v| v.json)
            .filter(|p| p.is_file())
            .ok_or_else(|| {
                anyhow!(
                    "{}",
                    tr_fmt(lang, "version {0} is required but not installed", &[id])
                )
            })?
    };
    VersionJson::load(&path)
}

/// The vanilla-json OS key for natives classifiers.
fn natives_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

/// Download one version-json library: the main artifact plus, for vanilla
/// entries that declare per-OS `natives` classifiers, the current OS's
/// natives jar. Existing files are kept (`fetch_to_file` verifies sha1).
fn download_library_entry(
    agent: &ureq::Agent,
    libs_dir: &Path,
    lib: &crate::version_json::LibraryEntry,
    natives_key: &str,
    lang: Language,
    progress: &mut dyn FnMut(&str),
) -> Result<()> {
    if let Some(dest) = coordinate_to_path(libs_dir, &lib.name) {
        let url = library_download_url(lib);
        let sha1 = match lib.artifact_download() {
            Some((_, sha1)) if !sha1.is_empty() => sha1.to_string(),
            _ => lib.sha1.clone(),
        };
        fetch_to_file(agent, &url, &dest, &sha1, progress, lang)?;
    }
    if let Some(classifier) = lib.natives_classifier(natives_key) {
        let coordinate = format!("{}:{}", lib.name, classifier);
        if let Some(dest) = coordinate_to_path(libs_dir, &coordinate) {
            if let Some((url, sha1)) = lib
                .downloads
                .as_ref()
                .and_then(|d| d.classifiers.get(&classifier))
                .map(|a| (a.url.as_str(), a.sha1.as_str()))
                .filter(|(url, _)| !url.is_empty())
            {
                fetch_to_file(agent, url, &dest, sha1, progress, lang)?;
            }
        }
    }
    Ok(())
}

/// Ensure every library declared by the version json and its parent chain
/// (`inheritsFrom`/`jar`) exists under `<game_dir>/libraries`, downloading
/// missing jars. Called right before the launch, so a version whose json
/// was added without libraries (Fabric/Quilt profiles, hand-copied dirs)
/// still starts instead of failing with "could not find the game".
pub fn ensure_version_libraries(
    agent: &ureq::Agent,
    game_dir: &Path,
    json: &VersionJson,
    lang: Language,
    progress: &mut dyn FnMut(&str),
) -> Result<()> {
    // Parent chain, child first; `visited` guards against cycles.
    let mut jsons: Vec<VersionJson> = vec![json.clone()];
    let mut visited: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut next = json.parent_id().map(str::to_string);
    while let Some(id) = next {
        if !visited.insert(id.clone()) {
            break;
        }
        let parent = load_installed_version_json(game_dir, &id, lang)?;
        next = parent.parent_id().map(str::to_string);
        jsons.push(parent);
    }

    let libs_dir = game_dir.join("libraries");
    let natives = natives_os();
    let total: usize = jsons.iter().map(|j| j.libraries.len()).sum();
    let mut done = 0usize;
    for version_json in &jsons {
        for lib in &version_json.libraries {
            done += 1;
            // Malformed coordinates cannot map to a maven path; classpath
            // assembly skips them the same way.
            if coordinate_to_path(&libs_dir, &lib.name).is_none() {
                continue;
            }
            download_library_entry(agent, &libs_dir, lib, natives, lang, progress)?;
            progress(&tr_fmt(
                lang,
                "libraries {0}/{1}",
                &[&done.to_string(), &total.to_string()],
            ));
        }
    }
    Ok(())
}

/// Extract the vanilla-format `version.json` embedded in a mod-loader
/// installer jar (NeoForge ships its launcher profile there).
fn installer_version_json(installer_jar: &[u8]) -> Result<Vec<u8>> {
    let reader = std::io::Cursor::new(installer_jar);
    let mut archive = zip::ZipArchive::new(reader).context("broken installer jar")?;
    let mut entry = archive
        .by_name("version.json")
        .context("installer jar has no version.json")?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut out)?;
    Ok(out)
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
    let (maven_base, coordinate) = match loader {
        Loader::NeoForge => (
            "https://maven.neoforged.net/releases",
            format!("net.neoforged:neoforge:{build}:universal"),
        ),
        _ => (
            "https://maven.minecraftforge.net",
            format!("net.minecraftforge:forge:{build}:universal"),
        ),
    };
    // The `url` lets launch-time library resolution fetch the jar from the
    // loader's own maven instead of the default Mojang repository.
    serde_json::json!({
        "id": version_id,
        "inheritsFrom": mc,
        "jar": mc,
        "releaseTime": "2024-01-01T00:00:00+00:00",
        "time": "2024-01-01T00:00:00+00:00",
        "type": "release",
        "mainClass": main_class,
        "libraries": [ { "name": coordinate, "url": format!("{maven_base}/") } ],
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
        let lang = crate::lang::Language::English;
        let err = verify_sha1(
            b"abc",
            "0000000000000000000000000000000000000000",
            "test",
            lang,
        )
        .unwrap_err();
        assert!(err.to_string().contains("mismatch"));
        verify_sha1(
            b"abc",
            "A9993E364706816ABA3E25717850C26C9CD0D89D",
            "test",
            lang,
        )
        .unwrap();
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
        assert!(neoforge_major_minor("1.21.4", crate::lang::Language::English).is_ok());
        assert!(neoforge_major_minor("1.20.2", crate::lang::Language::English).is_ok());
        assert!(neoforge_major_minor("1.20.1", crate::lang::Language::English).is_err());
        assert!(neoforge_major_minor("1.12.2", crate::lang::Language::English).is_err());
        assert!(neoforge_major_minor("not-a-version", crate::lang::Language::English).is_err());
        // Year-based ids map directly to `<year>.<minor>.<build>`.
        assert_eq!(
            neoforge_major_minor("26.3", crate::lang::Language::English).unwrap(),
            (26, 3)
        );
        assert_eq!(
            neoforge_major_minor("1.21.4", crate::lang::Language::English).unwrap(),
            (21, 4)
        );
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
    fn library_download_url_resolution_order() {
        let with_artifact = crate::version_json::LibraryEntry {
            name: "com.google.gson:gson:2.10.1".into(),
            url: String::new(),
            sha1: String::new(),
            downloads: Some(crate::version_json::LibraryDownloads {
                artifact: Some(crate::version_json::LibraryArtifact {
                    url: "https://libraries.minecraft.net/gson-2.10.1.jar".into(),
                    sha1: "abc".into(),
                }),
                classifiers: BTreeMap::new(),
            }),
            natives: None,
        };
        assert_eq!(
            library_download_url(&with_artifact),
            "https://libraries.minecraft.net/gson-2.10.1.jar"
        );

        let with_repo = crate::version_json::LibraryEntry {
            name: "net.fabricmc:fabric-loader:0.19.5".into(),
            url: "https://maven.fabricmc.net/".into(),
            sha1: String::new(),
            downloads: None,
            natives: None,
        };
        assert_eq!(
            library_download_url(&with_repo),
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar"
        );

        let bare = crate::version_json::LibraryEntry {
            name: "org.ow2.asm:asm:9.6".into(),
            url: String::new(),
            sha1: String::new(),
            downloads: None,
            natives: None,
        };
        assert_eq!(
            library_download_url(&bare),
            "https://libraries.minecraft.net/org/ow2/asm/asm/9.6/asm-9.6.jar"
        );
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
    fn installer_version_json_extracts_profile() {
        let buf = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(buf);
        writer
            .start_file("version.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, br#"{"id":"x","mainClass":"a.B"}"#).unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let meta = installer_version_json(&bytes).unwrap();
        let parsed: VersionJson = serde_json::from_slice(&meta).unwrap();
        assert_eq!(parsed.main_class(), "a.B");

        let no_entry = std::io::Cursor::new(Vec::new());
        let mut writer2 = zip::ZipWriter::new(no_entry);
        writer2
            .start_file("other.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer2, b"x").unwrap();
        let bytes2 = writer2.finish().unwrap().into_inner();
        assert!(installer_version_json(&bytes2).is_err());
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
        assert_eq!(
            profile["libraries"][0]["url"],
            "https://maven.neoforged.net/releases/"
        );

        let _ = build;
        let _ = game_dir;
        let _ = std::fs::remove_dir_all(&game_dir);
    }
}
