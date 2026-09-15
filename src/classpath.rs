//! Classpath assembly: collect library jars, deduplicate Maven artifacts,
//! resolve modloader bootstrap libraries, and keep LWJGL conflicts out.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::version_json::{maven_coordinate_to_path, VersionJson};

/// Collect all `.jar` files under a directory, recursively, sorted.
pub fn collect_jars(dir: &Path) -> Vec<PathBuf> {
    let mut jars = Vec::new();
    collect_jars_into(dir, &mut jars);
    jars.sort();
    jars
}

fn collect_jars_into(dir: &Path, jars: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jars_into(&path, jars);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("jar"))
        {
            jars.push(path);
        }
    }
}

/// Maven layout parts of a library path relative to the libraries dir:
/// `<group dirs...>/<artifact>/<version>/<artifact>-<version>.jar`.
fn maven_parts(libraries_dir: &Path, jar: &Path) -> Option<(String, String, String)> {
    let rel = jar.strip_prefix(libraries_dir).ok()?;
    let mut parts: Vec<String> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .map(str::to_string)
        .collect();
    if parts.len() < 4 {
        return None;
    }
    let _file = parts.pop()?;
    let version = parts.pop()?;
    let artifact = parts.pop()?;
    let group = parts.join(".");
    Some((group, artifact, version))
}

/// Compare dot-separated version strings numerically; non-numeric components
/// compare as 0 (matching the Java launcher's behaviour).
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (x, y) => {
                let parse = |s: Option<&str>| -> i64 {
                    s.and_then(|s| s.split('-').next())
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0)
                };
                let (n1, n2) = (parse(x), parse(y));
                if n1 != n2 {
                    return n1.cmp(&n2);
                }
            }
        }
    }
}

/// Keep only the newest version of each `group:artifact`, preserving all
/// entries (classifiers like natives) of the kept version.
pub fn deduplicate_artifacts(jars: &mut Vec<PathBuf>, libraries_dir: &Path) {
    let mut by_artifact: BTreeMap<(String, String), Vec<(String, PathBuf)>> = BTreeMap::new();
    for jar in jars.iter() {
        if let Some((group, artifact, version)) = maven_parts(libraries_dir, jar) {
            by_artifact
                .entry((group, artifact))
                .or_default()
                .push((version, jar.clone()));
        }
    }

    let mut keep: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for ((_group, artifact), mut versions) in by_artifact {
        if versions.len() == 1 {
            keep.insert(versions.remove(0).1);
            continue;
        }
        versions.sort_by(|a, b| compare_versions(&b.0, &a.0));
        let newest = versions[0].0.clone();
        // Gson >= 2.14 removed setStrictness that modern clients need: fall
        // back to the newest version that still has it.
        let target = if artifact == "gson" && is_gson_too_new(&newest) {
            versions
                .iter()
                .find(|(v, _)| !is_gson_too_new(v))
                .map(|(v, _)| v.clone())
                .unwrap_or(newest)
        } else {
            newest
        };
        for (version, path) in versions {
            if version == target {
                keep.insert(path);
            }
        }
    }
    jars.retain(|jar| !is_under(jar, libraries_dir) || keep.contains(jar));
}

fn is_under(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir)
}

fn is_gson_too_new(version: &str) -> bool {
    let mut parts = version.split('.');
    let major: i64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor: i64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    major == 2 && minor >= 14
}

/// Remove LWJGL jars that are not the newest `org.lwjgl*` version present,
/// to avoid VerifyError from mixed LWJGL 2/3 classes.
pub fn remove_conflicting_lwjgl(jars: &mut Vec<PathBuf>, libraries_dir: &Path) {
    let mut lwjgl_versions: Vec<(String, PathBuf)> = Vec::new();
    for jar in jars.iter() {
        if let Some((group, _, version)) = maven_parts(libraries_dir, jar) {
            if group.starts_with("org.lwjgl") {
                lwjgl_versions.push((version, jar.clone()));
            }
        }
    }
    if lwjgl_versions.len() <= 1 {
        return;
    }
    let newest = lwjgl_versions
        .iter()
        .map(|(v, _)| v.clone())
        .max_by(|a, b| compare_versions(a, b))
        .unwrap();
    jars.retain(|jar| {
        let is_old_lwjgl = lwjgl_versions
            .iter()
            .any(|(v, path)| *path == *jar && *v != newest);
        !is_old_lwjgl
    });
}

/// Bootstrap libraries for modloader versions: resolve `version.json`'s
/// library coordinates to existing jars under the libraries directory.
pub fn bootstrap_libraries(version_json: &VersionJson, libraries_dir: &Path) -> Vec<PathBuf> {
    version_json
        .libraries
        .iter()
        .filter_map(|lib| maven_coordinate_to_path(libraries_dir, &lib.name))
        .collect()
}

/// Build the final classpath for a version.
///
/// - Fabric-like versions (main class contains `KnotClient`): version jar +
///   bootstrap libraries from version.json only — Fabric's own classloader
///   loads the rest, and duplicates on `-cp` break signed-jar loading.
/// - Vanilla / Forge: all library jars + the version jar + referenced jar.
pub fn build_classpath(
    game_dir: &Path,
    _version_name: &str,
    version_json: &VersionJson,
    version_jar: &Path,
) -> Result<String> {
    let libraries_dir = game_dir.join("libraries");
    let separator: &str = if cfg!(target_os = "windows") {
        ";"
    } else {
        ":"
    };
    let mut entries: Vec<PathBuf> = Vec::new();

    let is_fabric = version_json.main_class().contains("KnotClient");
    if is_fabric {
        entries.push(version_jar.to_path_buf());
        entries.extend(bootstrap_libraries(version_json, &libraries_dir));
    } else {
        entries.extend(collect_jars(&libraries_dir));
        if let Some(referenced) = version_json.jar_reference() {
            let ref_jar = game_dir
                .join("versions")
                .join(referenced)
                .join(format!("{referenced}.jar"));
            if ref_jar.is_file() {
                entries.push(ref_jar);
            }
        }
        entries.push(version_jar.to_path_buf());
        deduplicate_artifacts(&mut entries, &libraries_dir);
        if version_json.uses_lwjgl3() {
            remove_conflicting_lwjgl(&mut entries, &libraries_dir);
        }
    }

    if entries.is_empty() {
        return Err(anyhow::anyhow!("classpath is empty — no libraries found"));
    }

    let classpath = entries
        .iter()
        .map(|p| {
            p.to_str()
                .with_context(|| format!("non-UTF8 path: {}", p.display()))
        })
        .collect::<Result<Vec<&str>>>()?
        .join(separator);

    Ok(classpath)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"PK").unwrap();
    }

    /// Unique temp root per test to avoid parallel-test interference.
    fn tmp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rl-cp-{}-{tag}", std::process::id()))
    }

    fn write_libs(root: &Path) -> PathBuf {
        let libs = root.join("libraries");
        // gson 2.10.1 and 2.11 (both "too new" candidates), asm 9 and 6,
        // lwjgl 3.3.1 and 2.9.1.
        touch(&libs.join("com/google/gson/gson/2.10.1/gson-2.10.1.jar"));
        touch(&libs.join("com/google/gson/gson/2.11.0/gson-2.11.0.jar"));
        touch(&libs.join("org/ow2/asm/asm/9.6/asm-9.6.jar"));
        touch(&libs.join("org/ow2/asm/asm/6.2/asm-6.2.jar"));
        touch(&libs.join("org/lwjgl/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar"));
        touch(&libs.join("org/lwjgl/lwjgl/lwjgl/2.9.1/lwjgl-2.9.1.jar"));
        libs
    }

    #[test]
    fn collects_jars_recursively_and_sorted() {
        let tmp = tmp_root("collect");
        let _ = fs::remove_dir_all(&tmp);
        let libs = write_libs(&tmp);

        let jars = collect_jars(&libs);
        assert_eq!(jars.len(), 6);
        let mut paths = jars.clone();
        paths.sort();
        assert_eq!(jars, paths, "collect_jars must return sorted paths");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn dedup_keeps_newest_artifact_version() {
        let tmp = tmp_root("dedup");
        let _ = fs::remove_dir_all(&tmp);
        let libs = write_libs(&tmp);

        let mut jars = collect_jars(&libs);
        deduplicate_artifacts(&mut jars, &libs);

        let names: Vec<String> = jars
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"gson-2.11.0.jar".to_string()));
        assert!(!names.contains(&"gson-2.10.1.jar".to_string()));
        assert!(names.contains(&"asm-9.6.jar".to_string()));
        assert!(!names.contains(&"asm-6.2.jar".to_string()));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn dedup_gson_skips_too_new() {
        let tmp = tmp_root("gson");
        let _ = fs::remove_dir_all(&tmp);
        let libs = tmp.join("libraries");
        touch(&libs.join("com/google/gson/gson/2.10.1/gson-2.10.1.jar"));
        touch(&libs.join("com/google/gson/gson/2.14.0/gson-2.14.0.jar"));

        let mut jars = collect_jars(&libs);
        deduplicate_artifacts(&mut jars, &libs);

        let names: Vec<String> = jars
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        // 2.14 removed setStrictness; the launcher must fall back to 2.10.1.
        assert!(names.contains(&"gson-2.10.1.jar".to_string()));
        assert!(!names.contains(&"gson-2.14.0.jar".to_string()));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn lwjgl_conflicts_removed_keeping_newest() {
        let tmp = tmp_root("lwjgl");
        let _ = fs::remove_dir_all(&tmp);
        let libs = write_libs(&tmp);

        let mut jars = collect_jars(&libs);
        remove_conflicting_lwjgl(&mut jars, &libs);

        let names: Vec<String> = jars
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"lwjgl-3.3.1.jar".to_string()));
        assert!(!names.contains(&"lwjgl-2.9.1.jar".to_string()));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn version_compare_handles_numeric_and_legacy() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("2.10.1", "2.9.4"), Greater);
        assert_eq!(compare_versions("1.7.10", "1.7.2"), Greater);
        assert_eq!(compare_versions("3.3.1", "3.3.1"), Equal);
        assert_eq!(compare_versions("2.8.9", "2.10.0"), Less);
    }
}
