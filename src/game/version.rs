//! Game directory and version discovery.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// A Minecraft version installed on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub name: String,
    pub dir: PathBuf,
    pub jar: PathBuf,
    pub json: PathBuf,
}

/// Default game root: `%APPDATA%\.rustlauncher` on Windows,
/// `~/Library/Application Support/.rustlauncher` on macOS, `~/.rustlauncher` elsewhere.
pub fn default_game_dir() -> Result<PathBuf> {
    if cfg!(target_os = "windows") {
        let appdata =
            std::env::var("APPDATA").context("APPDATA environment variable is not set")?;
        Ok(Path::new(&appdata).join(".rustlauncher"))
    } else if cfg!(target_os = "macos") {
        let home = dirs::home_dir().context("could not resolve the home directory")?;
        Ok(home.join("Library/Application Support/.rustlauncher"))
    } else {
        let home = dirs::home_dir().context("could not resolve the home directory")?;
        Ok(home.join(".rustlauncher"))
    }
}

/// Resolve the effective game directory: explicit override, else default.
pub fn resolve_game_dir(override_dir: Option<&str>) -> Result<PathBuf> {
    match override_dir {
        Some(dir) if !dir.trim().is_empty() => Ok(PathBuf::from(dir)),
        _ => default_game_dir(),
    }
}

/// Directories that may contain version folders, in scan order:
/// root itself, `<root>/versions`, `<root>/.minecraft/versions`.
pub fn version_dir_candidates(root: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![
        root.to_path_buf(),
        root.join("versions"),
        root.join(".minecraft").join("versions"),
    ];
    // If the user pointed directly at .../versions, the parent is also a candidate.
    if root
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("versions"))
    {
        if let Some(parent) = root.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    candidates.dedup();
    candidates
}

/// A folder "looks like" a version dir when it contains `<name>.json`.
fn looks_like_version_dir(dir: &Path) -> bool {
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    dir.join(format!("{name}.json")).is_file()
}

/// Scan the game directory for installed versions, sorted by name.
pub fn list_versions(root: &Path) -> Result<Vec<Version>> {
    let mut versions: Vec<Version> = Vec::new();

    for dir in version_dir_candidates(root) {
        if !dir.is_dir() {
            continue;
        }
        let entries =
            std::fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("failed to read {}", dir.display()))?;
            let path = entry.path();
            if !path.is_dir() || !looks_like_version_dir(&path) {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .context("non-UTF8 version folder name")?
                .to_string();
            let jar = path.join(format!("{name}.jar"));
            let json = path.join(format!("{name}.json"));
            if versions.iter().any(|v| v.name == name) {
                continue;
            }
            versions.push(Version {
                name,
                dir: path,
                jar,
                json,
            });
        }
    }

    versions.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(versions)
}

/// Find one installed version by name.
pub fn find_version(root: &Path, name: &str) -> Result<Version> {
    let versions = list_versions(root)?;
    versions
        .into_iter()
        .find(|v| v.name == name)
        .ok_or_else(|| {
            anyhow!(
                "version '{name}' not found in {} (run `rustlauncher versions` to list)",
                root.display()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Unique temp root per test to avoid parallel-test interference.
    fn tmp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rl-ver-{}-{tag}", std::process::id()))
    }

    fn write_version(root: &Path, name: &str) {
        let dir = root.join("versions").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.json")), "{}").unwrap();
        fs::write(dir.join(format!("{name}.jar")), b"PK").unwrap();
    }

    #[test]
    fn lists_versions_from_versions_subdir() {
        let tmp = tmp_root("case1");
        let root = tmp.join("case1");
        let _ = fs::remove_dir_all(&root);
        write_version(&root, "1.7.10");
        write_version(&root, "1.21.4");
        // A random folder without <name>.json must be ignored.
        fs::create_dir_all(root.join("versions").join("not-a-version")).unwrap();

        let versions = list_versions(&root).unwrap();
        let names: Vec<&str> = versions.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, vec!["1.21.4", "1.7.10"]);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn scans_root_itself_when_versions_are_directly_inside() {
        let tmp = tmp_root("case2");
        let root = tmp.join("case2");
        let _ = fs::remove_dir_all(&root);
        write_version(&root, "fabric-1.20.1");

        let versions = list_versions(&root).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].name, "fabric-1.20.1");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn scans_dotminecraft_subdir() {
        let tmp = tmp_root("case3");
        let root = tmp.join("case3");
        let _ = fs::remove_dir_all(&root);
        write_version(&root.join(".minecraft"), "1.12.2");

        let versions = list_versions(&root).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].name, "1.12.2");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_version_reports_missing_with_context() {
        let tmp = tmp_root("case4");
        let root = tmp.join("case4");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let err = find_version(&root, "nope").unwrap_err();
        assert!(err.to_string().contains("nope"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
