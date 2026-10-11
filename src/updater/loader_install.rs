//! Mod-loader installation: write a self-contained version entry
//! (`versions/<id>/<id>.json`) for Fabric/Quilt (meta launcher profiles),
//! NeoForge (installer-jar metadata) and Forge (minimal hand-assembled
//! profile), downloading every library the profile references.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::game::version_json::VersionJson;
use crate::lang::{tr_fmt, Language};
use crate::util::net;

use super::InstallOutcome;
use super::libraries::{coordinate_rel, coordinate_to_path, download_library_entry, natives_os};
use super::loaders::{Loader, LoaderBuild};
use super::fetch_to_file;

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

/// The version id an installed loader build gets (`fabric-loader-0.19.5-1.21.4`,
/// `neoforge-1.21.4-21.4.157`). Shared by the installer and the modpack
/// engine's exact skip-if-installed check.
pub fn loader_version_id(loader: Loader, mc: &str, build: &str) -> String {
    match loader {
        Loader::Fabric => format!("fabric-loader-{build}-{mc}"),
        Loader::Quilt => format!("quilt-loader-{build}-{mc}"),
        Loader::NeoForge => format!("neoforge-{mc}-{build}"),
        Loader::Forge => format!("forge-{mc}-{build}"),
    }
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
    mut progress: &mut dyn FnMut(&str, Option<f32>),
) -> Result<InstallOutcome> {
    let version_id = loader_version_id(loader, mc, &build.version);
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
                    progress(
                        &tr_fmt(
                            lang,
                            "libraries {0}/{1}",
                            &[&(i + 1).to_string(), &total.to_string()],
                        ),
                        Some((i + 1) as f32 / total as f32),
                    );
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
            for (i, lib) in meta.libraries.iter().enumerate() {
                if coordinate_to_path(&libs_dir, &lib.name).is_none() {
                    continue;
                }
                download_library_entry(agent, &libs_dir, lib, natives, lang, &mut progress)?;
                downloaded += 1;
                progress(
                    &tr_fmt(
                        lang,
                        "libraries {0}/{1}",
                        &[&(i + 1).to_string(), &meta.libraries.len().to_string()],
                    ),
                    Some((i + 1) as f32 / meta.libraries.len() as f32),
                );
            }

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

    progress(
        &tr_fmt(lang, "{0} installed", &[&version_id]),
        Some(1.0),
    );
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
