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
//!
//! Module layout:
//! - [`loaders`] — the [`Loader`] enum and per-loader build discovery;
//! - [`vanilla`] — installing a vanilla version (client jar + assets);
//! - [`loader_install`] — installing a mod loader on top of a version;
//! - [`libraries`] — Maven library resolution shared by both installers.

mod libraries;
mod loader_install;
mod loaders;
mod vanilla;

pub use libraries::ensure_version_libraries;
pub use loader_install::{install_loader, loader_version_id};
pub use loaders::{Loader, LoaderBuild, fetch_loader_builds};
pub use vanilla::install_version;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::lang::{tr, tr_fmt, Language};
use crate::util::net;

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
    progress: &mut dyn FnMut(&str, Option<f32>),
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
    progress(&dest.file_name().unwrap_or_default().to_string_lossy(), None);
    Ok(true)
}

/// Everything needed to install one version.
pub struct InstallOutcome {
    pub version_id: String,
    pub downloaded_files: usize,
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
}
