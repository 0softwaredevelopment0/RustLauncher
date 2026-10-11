//! Maven library resolution shared by the vanilla installer and the
//! mod-loader installer: coordinate-to-path mapping, download URL
//! resolution and the per-library download loop.

use std::path::Path;

use anyhow::{anyhow, Result};

use crate::game::version_json::{LibraryEntry, VersionJson};
use crate::lang::{tr_fmt, Language};

use super::fetch_to_file;

/// Default Maven repository for libraries whose json entry declares no URL.
const DEFAULT_LIBRARY_REPO: &str = "https://libraries.minecraft.net/";

/// Resolve a Maven coordinate to a jar path under `libraries_dir`
/// (`net.fabricmc:fabric-loader:0.19.5` -> `<dir>/net/fabricmc/fabric-loader/
/// 0.19.5/fabric-loader-0.19.5.jar`). Returns `None` for malformed names.
pub(super) fn coordinate_to_path(
    libraries_dir: &Path,
    coordinate: &str,
) -> Option<std::path::PathBuf> {
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

/// `net.fabricmc:fabric-loader:0.19.5` ->
/// `net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar`
pub(super) fn coordinate_rel(coordinate: &str) -> String {
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

/// Resolve the download URL of a version-json library entry: the vanilla
/// `downloads.artifact.url` when present, else the entry's own repository
/// `url`, else the default Mojang libraries repository.
fn library_download_url(entry: &LibraryEntry) -> String {
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
        crate::game::version::list_versions(game_dir)
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
pub(super) fn natives_os() -> &'static str {
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
pub(super) fn download_library_entry(
    agent: &ureq::Agent,
    libs_dir: &Path,
    lib: &LibraryEntry,
    natives_key: &str,
    lang: Language,
    progress: &mut dyn FnMut(&str, Option<f32>),
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
    progress: &mut dyn FnMut(&str, Option<f32>),
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
            progress(
                &tr_fmt(
                    lang,
                    "libraries {0}/{1}",
                    &[&done.to_string(), &total.to_string()],
                ),
                Some(done as f32 / total as f32),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::version_json::{LibraryArtifact, LibraryDownloads};
    use std::collections::BTreeMap;

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
    fn library_download_url_resolution_order() {
        let with_artifact = LibraryEntry {
            name: "com.google.gson:gson:2.10.1".into(),
            url: String::new(),
            sha1: String::new(),
            downloads: Some(LibraryDownloads {
                artifact: Some(LibraryArtifact {
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

        let with_repo = LibraryEntry {
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

        let bare = LibraryEntry {
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
}
