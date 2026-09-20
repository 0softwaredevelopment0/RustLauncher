//! Downloading Java runtimes from popular distributions.
//!
//! Editions: Eclipse Adoptium (Temurin), Azul Zulu, Amazon Corretto,
//! Microsoft OpenJDK and Oracle GraalVM. Everything lands in the launcher's
//! `runtimes/` directory and is picked up by `java_locator` on the next scan,
//! so downloaded runtimes appear in the Settings Java picker automatically.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::lang::Language;
use crate::net;

// ---------------------------------------------------------------------------
// Editions
// ---------------------------------------------------------------------------

/// A downloadable Java distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edition {
    /// Stable identifier stored in configs (`temurin`, `zulu`, …).
    pub id: &'static str,
    /// Human-readable label shown in the UI.
    pub label: &'static str,
}

/// The editions the downloader supports, in display order.
pub const EDITIONS: &[Edition] = &[
    Edition {
        id: "temurin",
        label: "Eclipse Adoptium (Temurin)",
    },
    Edition {
        id: "zulu",
        label: "Azul Zulu",
    },
    Edition {
        id: "corretto",
        label: "Amazon Corretto",
    },
    Edition {
        id: "microsoft",
        label: "Microsoft OpenJDK",
    },
    Edition {
        id: "graalvm",
        label: "Oracle GraalVM",
    },
];

/// Look up an edition by id.
pub fn find_edition(id: &str) -> Option<&'static Edition> {
    EDITIONS.iter().find(|e| e.id == id)
}

/// Major versions offered for download (`8`–`26`, matching Minecraft's needs).
pub fn list_majors() -> Vec<u32> {
    (8..=26).rev().collect()
}

/// The OS token used by the vendor APIs (this build targets the running OS).
fn os_token() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Normalized architecture token: `x64` or `aarch64`.
pub fn arch_token() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x64"
    }
}

// ---------------------------------------------------------------------------
// Version listings
// ---------------------------------------------------------------------------

/// One downloadable sub-version of a major version (`21.0.5+13`, or the
/// pseudo-entry `latest` for editions without a version API).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubVersion {
    /// Opaque id passed back to [`resolve_url`].
    pub id: String,
    /// What the UI displays.
    pub label: String,
}

impl SubVersion {
    fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        let label = id.clone();
        Self { id, label }
    }
}

/// Build a `21.0.5+13`-style label from Adoptium `version_data`.
fn temurin_label(v: &Value) -> Option<String> {
    let major = v.get("major")?.as_u64()?;
    let minor = v.get("minor")?.as_u64()?;
    let security = v.get("security")?.as_u64()?;
    let build = v.get("build").and_then(Value::as_u64).unwrap_or(0);
    Some(format!("{major}.{minor}.{security}+{build}"))
}

/// Extract the `21.0.5` version token from a Zulu package name like
/// `zulu21.40.17-ca-jdk21.0.5-win_x64.zip`.
fn zulu_version_from_name(name: &str) -> Option<String> {
    let idx = name.find("jdk")? + 3;
    let rest = &name[idx..];
    let version: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
}

/// Extract a GraalVM version from a release tag like `jdk-21.0.2`.
fn graal_version_from_tag(tag: &str) -> Option<String> {
    let rest = tag.strip_prefix("jdk-")?;
    if rest.is_empty() || !rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(rest.to_string())
}

/// Sub-versions available for `major` of an edition. Hits the vendor's API
/// (Temurin/Zulu/GraalVM); Corretto and Microsoft ship a single `latest`.
pub fn list_sub_versions(
    agent: &ureq::Agent,
    lang: Language,
    edition_id: &str,
    major: u32,
) -> Result<Vec<SubVersion>> {
    let os = os_token();
    let arch = arch_token();
    match edition_id {
        "temurin" => {
            let url = format!(
                "https://api.adoptium.net/v3/assets/feature_releases/{major}/ga\
                 ?architecture={arch}&image_type=jdk&os={os}&page=0&page_size=25&sort_order=DESC"
            );
            let body = net::get_bytes(agent, &url, lang)?;
            let releases: Vec<Value> = serde_json::from_slice(&body)
                .with_context(|| format!("unexpected Adoptium API response for {url}"))?;
            let mut out = Vec::new();
            for release in &releases {
                let has_binary = release
                    .get("binaries")
                    .and_then(Value::as_array)
                    .is_some_and(|b| !b.is_empty());
                if !has_binary {
                    continue;
                }
                if let Some(label) = release.get("version_data").and_then(temurin_label) {
                    out.push(SubVersion::new(label));
                }
            }
            if out.is_empty() {
                return Err(anyhow!(
                    "Adoptium does not publish JDK {major} for {os}/{arch}"
                ));
            }
            Ok(out)
        }
        "zulu" => {
            let azul_arch = if arch == "aarch64" { "aarch64" } else { "x64" };
            let url = format!(
                "https://api.azul.com/metadata/v1/zulu/packages/?java_version={major}\
                 &os={os}&arch={azul_arch}&hw_bitness=64&bundle_type=jdk&archive_type=zip\
                 &release_status=ga&availability_types=CA&page_size=100"
            );
            let body = net::get_bytes(agent, &url, lang)?;
            let packages: Vec<Value> = serde_json::from_slice(&body)
                .with_context(|| format!("unexpected Azul API response for {url}"))?;
            let mut out: Vec<SubVersion> = Vec::new();
            for package in &packages {
                let name = package
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| package.get("url").and_then(Value::as_str))
                    .unwrap_or_default();
                if let Some(version) = zulu_version_from_name(name) {
                    if !out.iter().any(|s| s.id == version) {
                        out.push(SubVersion::new(version));
                    }
                }
            }
            if out.is_empty() {
                return Err(anyhow!(
                    "Azul does not publish JDK {major} for {os}/{azul_arch}"
                ));
            }
            Ok(out)
        }
        "graalvm" => {
            let body = net::get_bytes(
                agent,
                "https://api.github.com/repos/oracle/graalvm/releases?per_page=100",
                lang,
            )?;
            let releases: Vec<Value> =
                serde_json::from_slice(&body).context("unexpected GraalVM releases response")?;
            let mut out = Vec::new();
            for release in &releases {
                let tag = release
                    .get("tag_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(version) = graal_version_from_tag(tag) {
                    let prefix = format!("{major}.");
                    if version.starts_with(&prefix)
                        && !out.iter().any(|s: &SubVersion| s.id == version)
                    {
                        out.push(SubVersion::new(version));
                    }
                }
            }
            if out.is_empty() {
                return Err(anyhow!(
                    "GraalVM does not publish a JDK {major} release on GitHub"
                ));
            }
            Ok(out)
        }
        "corretto" | "microsoft" => Ok(vec![SubVersion::new("latest")]),
        other => Err(anyhow!("unknown Java edition: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Download URL resolution
// ---------------------------------------------------------------------------

/// The stable "latest per major" download URL for Corretto.
fn corretto_latest_url(major: u32, os: &str, arch: &str) -> Result<String> {
    if os != "windows" || arch != "x64" {
        return Err(anyhow!(
            "Corretto auto-downloads are only wired for Windows x64; use a custom Java path"
        ));
    }
    Ok(format!(
        "https://corretto.aws/downloads/latest/amazon-corretto-{major}-windows-x64-jdk.zip"
    ))
}

/// The stable "latest per major" download URL for Microsoft OpenJDK.
fn microsoft_latest_url(major: u32, os: &str, arch: &str) -> Result<String> {
    if os != "windows" {
        return Err(anyhow!(
            "Microsoft OpenJDK auto-downloads are only wired for Windows; use a custom Java path"
        ));
    }
    Ok(format!(
        "https://aka.ms/download-jdk/microsoft-jdk-{major}-windows-{arch}.zip"
    ))
}

/// Find the windows package link inside an Adoptium release entry.
fn temurin_download_url(release: &Value) -> Option<String> {
    release
        .get("binaries")?
        .as_array()?
        .iter()
        .find(|b| {
            b.get("image_type").and_then(Value::as_str) == Some("jdk")
                && b.get("os").and_then(Value::as_str) == Some(os_token())
                && b.get("architecture").and_then(Value::as_str) == Some(arch_token())
        })?
        .pointer("/package/link")?
        .as_str()
        .map(str::to_string)
}

/// Find the Zulu package URL matching a `21.0.5` version token.
fn zulu_download_url(packages: &[Value], version: &str) -> Option<String> {
    for package in packages {
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| package.get("url").and_then(Value::as_str))
            .unwrap_or_default();
        if zulu_version_from_name(name).as_deref() == Some(version) {
            return package
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
    }
    None
}

/// Find the GraalVM windows asset for a release tag.
fn graal_download_url(releases: &[Value], version: &str) -> Option<String> {
    for release in releases {
        let tag = release
            .get("tag_name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if graal_version_from_tag(tag).as_deref() != Some(version) {
            continue;
        }
        for asset in release
            .get("assets")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let name = asset.get("name").and_then(Value::as_str).unwrap_or("");
            let arch = arch_token();
            let wanted = format!("windows-{arch}_bin.zip");
            if name.ends_with(&wanted) {
                return asset
                    .get("browser_download_url")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }
    }
    None
}

/// Resolve an (edition, major, sub-version) to a direct download URL.
pub fn resolve_url(
    agent: &ureq::Agent,
    lang: Language,
    edition_id: &str,
    major: u32,
    sub: &str,
) -> Result<String> {
    let os = os_token();
    let arch = arch_token();
    match edition_id {
        "temurin" => {
            let url = format!(
                "https://api.adoptium.net/v3/assets/feature_releases/{major}/ga\
                 ?architecture={arch}&image_type=jdk&os={os}&page=0&page_size=25&sort_order=DESC"
            );
            let body = net::get_bytes(agent, &url, lang)?;
            let releases: Vec<Value> = serde_json::from_slice(&body)
                .with_context(|| format!("unexpected Adoptium API response for {url}"))?;
            let found = releases.iter().find(|r| {
                r.get("version_data")
                    .and_then(temurin_label)
                    .is_some_and(|l| l == sub || sub == "latest")
            });
            found.and_then(temurin_download_url).ok_or_else(|| {
                anyhow!("Adoptium release {sub} for JDK {major} has no {os}/{arch} package")
            })
        }
        "zulu" => {
            let azul_arch = if arch == "aarch64" { "aarch64" } else { "x64" };
            let url = format!(
                "https://api.azul.com/metadata/v1/zulu/packages/?java_version={major}\
                 &os={os}&arch={azul_arch}&hw_bitness=64&bundle_type=jdk&archive_type=zip\
                 &release_status=ga&availability_types=CA&page_size=100"
            );
            let body = net::get_bytes(agent, &url, lang)?;
            let packages: Vec<Value> = serde_json::from_slice(&body)
                .with_context(|| format!("unexpected Azul API response for {url}"))?;
            zulu_download_url(&packages, sub)
                .ok_or_else(|| anyhow!("Azul JDK {major} {sub} for {os}/{azul_arch} not found"))
        }
        "graalvm" => {
            let body = net::get_bytes(
                agent,
                "https://api.github.com/repos/oracle/graalvm/releases?per_page=100",
                lang,
            )?;
            let releases: Vec<Value> =
                serde_json::from_slice(&body).context("unexpected GraalVM releases response")?;
            graal_download_url(&releases, sub)
                .ok_or_else(|| anyhow!("GraalVM JDK {major} {sub} has no {os}/{arch} asset"))
        }
        "corretto" => corretto_latest_url(major, os, arch),
        "microsoft" => microsoft_latest_url(major, os, arch),
        other => Err(anyhow!("unknown Java edition: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

/// Make a filesystem-safe directory name from a sub-version id.
fn sanitize_dir_name(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The destination directory for a downloaded runtime:
/// `<runtimes>/<edition>-<sub>`.
pub fn runtime_dest(runtimes_dir: &Path, edition_id: &str, sub: &str) -> PathBuf {
    runtimes_dir.join(format!("{edition_id}-{}", sanitize_dir_name(sub)))
}

/// Extract a zip archive into `dest`, returning the paths written.
fn extract_zip(bytes: &[u8], dest: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .context("downloaded file is not a valid zip archive")?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("bad zip entry #{index}"))?;
        let Some(name) = entry
            .mangled_name()
            .file_name()
            .map(std::path::PathBuf::from)
        else {
            continue;
        };
        // Rebuild the path level by level (mangled_name already strips
        // traversal), so nested archives land in the right place.
        let target = dest.join(entry.mangled_name());
        if entry.is_dir() {
            std::fs::create_dir_all(&target)
                .with_context(|| format!("failed to create {}", target.display()))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)
            .with_context(|| format!("failed to create {}", target.display()))?;
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("failed to extract {}", name.display()))?;
    }
    Ok(())
}

/// Find `bin/java[.exe]` anywhere under `dir` (JDK zips nest a top folder).
fn find_java_exe(dir: &Path) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["java.exe", "javaw.exe"]
    } else {
        &["java"]
    };
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_lowercase();
        let in_bin = path
            .parent()
            .is_some_and(|p| p.file_name().is_some_and(|b| b == "bin"));
        if in_bin && names.contains(&file_name.as_ref()) {
            return Some(path);
        }
    }
    subdirs.iter().find_map(|d| find_java_exe(d))
}

/// Download and unpack a runtime, returning the path to its `java` executable.
/// The result is idempotent: an already-installed (edition, sub-version) is
/// returned as-is instead of downloading again.
pub fn install_runtime(
    agent: &ureq::Agent,
    lang: Language,
    edition_id: &str,
    major: u32,
    sub: &str,
    runtimes_dir: &Path,
) -> Result<PathBuf> {
    let dest = runtime_dest(runtimes_dir, edition_id, sub);
    if let Some(java) = find_java_exe(&dest) {
        return Ok(java);
    }

    let url = resolve_url(agent, lang, edition_id, major, sub)?;
    let bytes = net::get_bytes(agent, &url, lang)?;
    if bytes.len() < 1024 {
        return Err(anyhow!(
            "download from {url} is suspiciously small ({} bytes)",
            bytes.len()
        ));
    }
    std::fs::create_dir_all(&dest)?;
    extract_zip(&bytes, &dest)?;

    find_java_exe(&dest).ok_or_else(|| {
        anyhow!(
            "no bin/java found in the {} {} archive",
            find_edition(edition_id)
                .map(|e| e.label)
                .unwrap_or(edition_id),
            sub
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editions_are_unique_and_labeled() {
        let mut ids: Vec<&str> = EDITIONS.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), EDITIONS.len());
        assert!(EDITIONS.iter().all(|e| !e.label.is_empty()));
        assert!(find_edition("temurin").is_some());
        assert!(find_edition("nope").is_none());
    }

    #[test]
    fn majors_span_8_to_26_newest_first() {
        let majors = list_majors();
        assert_eq!(majors.len(), 19);
        assert_eq!(majors[0], 26);
        assert_eq!(*majors.last().unwrap(), 8);
    }

    #[test]
    fn temurin_labels_match_version_data() {
        let v: Value = serde_json::from_str(
            r#"{"major": 21, "minor": 0, "security": 5, "patch": null, "build": 11}"#,
        )
        .unwrap();
        assert_eq!(temurin_label(&v).as_deref(), Some("21.0.5+11"));

        let legacy: Value = serde_json::from_str(
            r#"{"major": 8, "minor": 0, "security": 412, "patch": null, "build": 8}"#,
        )
        .unwrap();
        assert_eq!(temurin_label(&legacy).as_deref(), Some("8.0.412+8"));

        assert_eq!(temurin_label(&Value::Null), None);
    }

    #[test]
    fn zulu_names_yield_version_tokens() {
        assert_eq!(
            zulu_version_from_name("zulu21.40.17-ca-jdk21.0.5-win_x64.zip").as_deref(),
            Some("21.0.5")
        );
        assert_eq!(
            zulu_version_from_name("zulu17.54.21-ca-fx-jdk17.0.11-win_x64.zip").as_deref(),
            Some("17.0.11")
        );
        assert_eq!(
            zulu_version_from_name("zulu8.78.0.19-ca-jdk8.0.412-win_x64.zip").as_deref(),
            Some("8.0.412")
        );
        assert_eq!(zulu_version_from_name("readme.txt"), None);
    }

    #[test]
    fn graal_tags_yield_version_tokens() {
        assert_eq!(
            graal_version_from_tag("jdk-21.0.2").as_deref(),
            Some("21.0.2")
        );
        assert_eq!(graal_version_from_tag("jdk-25").as_deref(), Some("25"));
        assert_eq!(graal_version_from_tag("vm-21.0.2"), None);
        assert_eq!(graal_version_from_tag("jdk-"), None);
    }

    #[test]
    fn latest_urls_follow_vendor_patterns() {
        assert_eq!(
            corretto_latest_url(21, "windows", "x64").unwrap(),
            "https://corretto.aws/downloads/latest/amazon-corretto-21-windows-x64-jdk.zip"
        );
        assert_eq!(
            microsoft_latest_url(21, "windows", "x64").unwrap(),
            "https://aka.ms/download-jdk/microsoft-jdk-21-windows-x64.zip"
        );
        assert!(corretto_latest_url(21, "linux", "x64").is_err());
        assert!(microsoft_latest_url(21, "macos", "x64").is_err());
    }

    #[test]
    fn temurin_download_url_picks_the_windows_package() {
        let release: Value = serde_json::from_str(
            r#"{
                "version_data": {"major": 21, "minor": 0, "security": 5, "build": 11},
                "binaries": [
                    {"os": "linux", "architecture": "x64", "image_type": "jdk",
                     "package": {"link": "https://example.com/linux.zip"}},
                    {"os": "windows", "architecture": "x64", "image_type": "jdk",
                     "package": {"link": "https://example.com/win.zip"}},
                    {"os": "windows", "architecture": "x86", "image_type": "jdk",
                     "package": {"link": "https://example.com/win32.zip"}}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(
            temurin_download_url(&release).as_deref(),
            Some("https://example.com/win.zip")
        );
    }

    #[test]
    fn zulu_download_url_matches_the_version() {
        let packages: Vec<Value> = serde_json::from_str(
            r#"[
                {"name": "zulu21.40.17-ca-jdk21.0.5-win_x64.zip",
                 "url": "https://cdn.azul.com/zulu/bin/zulu21.40.17-ca-jdk21.0.5-win_x64.zip"},
                {"name": "zulu21.40.15-ca-jdk21.0.4-win_x64.zip",
                 "url": "https://cdn.azul.com/zulu/bin/zulu21.40.15-ca-jdk21.0.4-win_x64.zip"}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            zulu_download_url(&packages, "21.0.4").as_deref(),
            Some("https://cdn.azul.com/zulu/bin/zulu21.40.15-ca-jdk21.0.4-win_x64.zip")
        );
        assert_eq!(zulu_download_url(&packages, "9.9.9"), None);
    }

    #[test]
    fn graal_download_url_matches_os_and_arch() {
        let releases: Vec<Value> = serde_json::from_str(
            r#"[
                {"tag_name": "jdk-21.0.2",
                 "assets": [
                    {"name": "graalvm-jdk-21_linux-x64_bin.zip",
                     "browser_download_url": "https://github.com/x/graalvm-jdk-21_linux-x64_bin.zip"},
                    {"name": "graalvm-jdk-21_windows-x64_bin.zip",
                     "browser_download_url": "https://github.com/x/graalvm-jdk-21_windows-x64_bin.zip"}
                 ]}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            graal_download_url(&releases, "21.0.2").as_deref(),
            Some("https://github.com/x/graalvm-jdk-21_windows-x64_bin.zip")
        );
        assert_eq!(graal_download_url(&releases, "20.0.0"), None);
    }

    #[test]
    fn dest_names_are_filesystem_safe() {
        assert_eq!(
            runtime_dest(Path::new("runtimes"), "temurin", "21.0.5+11")
                .file_name()
                .unwrap(),
            "temurin-21.0.5+11"
        );
        assert_eq!(
            runtime_dest(Path::new("runtimes"), "zulu", "a/b:c")
                .file_name()
                .unwrap(),
            "zulu-a_b_c"
        );
    }

    #[test]
    fn zip_extraction_lands_in_dest() {
        // Build a tiny zip in-memory with the `zip` crate itself.
        let mut buf = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let options: zip::write::SimpleFileOptions = Default::default();
            writer
                .start_file("jdk-21.0.5/bin/release-marker", options)
                .unwrap();
            std::io::Write::write_all(&mut writer, b"ok").unwrap();
            writer.finish().unwrap();
        }
        let bytes = buf.into_inner();

        let dir =
            std::env::temp_dir().join(format!("rustlauncher-zip-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        extract_zip(&bytes, &dir).unwrap();
        let marker = dir.join("jdk-21.0.5/bin/release-marker");
        assert!(marker.is_file());
        assert_eq!(std::fs::read(&marker).unwrap(), b"ok");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
