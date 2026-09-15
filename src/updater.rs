//! Remote version discovery and installation from the Mojang manifest.
//!
//! The Java launcher could only *list* remote versions (`Updater.fetchVersions`)
//! but had no install path; this port closes that gap: downloading the version
//! JSON, the client jar, the asset index and all asset objects, with SHA-1
//! verification and a resumable layout identical to the vanilla launcher.

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
}
