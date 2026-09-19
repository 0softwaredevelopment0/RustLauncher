//! Locating a suitable Java runtime for the game.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};

/// Parse the major version from `java -version` output
/// (e.g. `openjdk version "26.0.1" 2025-04-15` -> 26,
///  `java version "1.8.0_402"` -> 8).
pub fn parse_java_major_version(output: &str) -> Option<u32> {
    let line = output.lines().next()?;
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    let version = &line[start..end];
    let mut parts = version.split('.');
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        // Legacy "1.8.0" format: the major version is the second component.
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Run `java -version` (reports on stderr) and return the major version.
pub fn java_major_version(java_exe: &Path) -> Option<u32> {
    let output = Command::new(java_exe).arg("-version").output().ok()?;
    if !output.status.success() && output.stderr.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stderr);
    parse_java_major_version(&text).or_else(|| {
        // Some JVMs print -version on stdout.
        parse_java_major_version(&String::from_utf8_lossy(&output.stdout))
    })
}

/// The java executable bundled with (or adjacent to) this launcher, if any:
/// `<exe_dir>/runtime/bin/java[.exe]`, then `<exe_dir>/jre/bin/...`.
pub fn bundled_java() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for sub in ["runtime", "jre"] {
        for name in java_binary_names() {
            let candidate = dir.join(sub).join("bin").join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn java_binary_names() -> Vec<&'static str> {
    if cfg!(target_os = "windows") {
        vec!["java.exe", "javaw.exe"]
    } else {
        vec!["java"]
    }
}

/// Find `java` on PATH (skipping `where`/`which` shell-outs).
pub fn java_on_path() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in java_binary_names() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Common JRE/JDK installation roots (Windows-oriented, harmless elsewhere).
fn common_java_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("C:/Program Files/Java"),
        PathBuf::from("C:/Program Files (x86)/Java"),
        PathBuf::from("C:/Program Files/Eclipse Adoptium"),
        PathBuf::from("C:/Program Files/Amazon Corretto"),
        PathBuf::from("C:/Program Files/Zulu"),
        PathBuf::from("C:/Program Files/Microsoft"),
    ];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        roots.push(local.join("Programs/Eclipse Adoptium"));
        roots.push(local.join("Programs/Java"));
    }
    roots
}

/// Find any installed Java with major version <= `max_version`, preferring the
/// highest compatible one. Checks PATH first, then common install directories.
pub fn find_compatible_java(max_version: u32) -> Option<PathBuf> {
    let mut best: Option<(u32, PathBuf)> = None;

    let mut consider = |path: PathBuf| {
        if !path.is_file() {
            return;
        }
        if let Some(major) = java_major_version(&path) {
            if major <= max_version && best.as_ref().is_none_or(|(m, _)| major > *m) {
                best = Some((major, path));
            }
        }
    };

    if let Some(path_java) = java_on_path() {
        consider(path_java);
    }
    for root in common_java_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            for bin_dir in [dir.join("bin"), dir.join("jre/bin")] {
                for name in java_binary_names() {
                    consider(bin_dir.join(name));
                }
            }
        }
    }
    best.map(|(_, path)| path)
}

/// Pick the Java executable used for launching: explicit setting, bundled
/// runtime, or the best match for the game's required version.
pub fn select_java(
    configured: Option<&str>,
    required_major: Option<u32>,
    lang: crate::lang::Language,
) -> Result<PathBuf> {
    use crate::lang::{tr, tr_fmt};
    // 1. Explicitly configured path must exist.
    if let Some(path) = configured {
        if !path.trim().is_empty() {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Ok(path);
            }
            return Err(anyhow!(
                "{}",
                tr_fmt(
                    lang,
                    "configured Java path does not exist: {0}",
                    &[&path.display().to_string()]
                )
            ));
        }
    }

    // 2. Bundled runtime next to the launcher binary.
    if let Some(bundled) = bundled_java() {
        return Ok(bundled);
    }

    // 3. Best Java on the system for the required major version.
    if let Some(required) = required_major {
        if let Some(compatible) = find_compatible_java(required) {
            return Ok(compatible);
        }
    }

    // 4. Anything on PATH.
    if let Some(path_java) = java_on_path() {
        return Ok(path_java);
    }

    Err(anyhow!("{}", tr(lang, "Java not found. Install Java or set a custom path.")))
}

/// The major version of the selected Java, probed once.
pub fn selected_java_major(java_exe: &Path, lang: crate::lang::Language) -> Result<u32> {
    java_major_version(java_exe).context(crate::lang::tr_fmt(
        lang,
        "failed to probe Java version at {0}",
        &[&java_exe.display().to_string()],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_java_version_output() {
        let out = "openjdk version \"26.0.1\" 2025-04-15\nOpenJDK Runtime Environment";
        assert_eq!(parse_java_major_version(out), Some(26));
    }

    #[test]
    fn parses_legacy_1_8_format() {
        let out = "java version \"1.8.0_402\"\nJava(TM) SE Runtime Environment";
        assert_eq!(parse_java_major_version(out), Some(8));
    }

    #[test]
    fn parses_graal_and_other_prefixes() {
        assert_eq!(
            parse_java_major_version("openjdk version \"21.0.2\" 2024-01-16"),
            Some(21)
        );
        assert_eq!(parse_java_major_version("no version here"), None);
        assert_eq!(parse_java_major_version(""), None);
    }

    #[test]
    fn select_java_fails_when_configured_path_missing() {
        let err = select_java(
            Some("Z:/definitely/not/real/java.exe"),
            None,
            crate::lang::Language::English,
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }
}
