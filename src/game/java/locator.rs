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

    Err(anyhow!(
        "{}",
        tr(lang, "Java not found. Install Java or set a custom path.")
    ))
}

/// The major version of the selected Java, probed once.
pub fn selected_java_major(java_exe: &Path, lang: crate::lang::Language) -> Result<u32> {
    java_major_version(java_exe).context(crate::lang::tr_fmt(
        lang,
        "failed to probe Java version at {0}",
        &[&java_exe.display().to_string()],
    ))
}

// ---------------------------------------------------------------------------
// Installed-runtime inventory (Settings → Java picker).
// ---------------------------------------------------------------------------

/// A Java runtime found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledJava {
    /// Path to the `java` executable (what the launch plan consumes).
    pub path: PathBuf,
    /// Directory that contains `bin/java` (the runtime home).
    pub home: PathBuf,
    /// Major version (`8`, `21`, `26`…).
    pub major: u32,
    /// Full version string from `java -version` (`21.0.5`, `1.8.0_402`).
    pub version: String,
    /// Short vendor hint derived from the install location.
    pub vendor: String,
}

impl InstalledJava {
    /// Combo-box label, e.g. `Temurin 21.0.5`.
    pub fn label(&self) -> String {
        format!("{} {}", self.vendor, self.version)
    }
}

/// The full version token quoted by `java -version`
/// (`openjdk version "21.0.5" …` -> `21.0.5`).
pub fn parse_java_full_version(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

/// Probe a java executable; `None` when the binary does not answer.
pub fn probe(java_exe: &Path) -> Option<(u32, String)> {
    let output = Command::new(java_exe).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr).to_string();
    let text = if text.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        text
    };
    let version = parse_java_full_version(&text)?;
    let major = parse_java_major_version(&text)?;
    Some((major, version))
}

/// Guess a vendor label from the install path (parent folder names like
/// `Eclipse Adoptium`, dir names like `zulu-21`, `corretto`, `liberica`).
pub fn vendor_from_path(home: &Path) -> String {
    let haystack = format!(
        "{} {}",
        home.parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        home.display()
    )
    .to_ascii_lowercase();
    let table: &[(&str, &str)] = &[
        ("temurin", "Temurin"),
        ("eclipse adoptium", "Temurin"),
        ("corretto", "Corretto"),
        ("zulu", "Zulu"),
        ("liberica", "Liberica"),
        ("graalvm", "GraalVM"),
        ("microsoft", "Microsoft"),
        ("jdk-1.8", "Oracle"),
        ("jdk1.", "Oracle"),
        ("oracle", "Oracle"),
        ("jdk-", "Oracle"),
    ];
    for (needle, label) in table {
        if haystack.contains(needle) {
            return label.to_string();
        }
    }
    if haystack.contains("runtimes") {
        return "RustLauncher".to_string();
    }
    "Java".to_string()
}

/// Normalize a path for dedup: absolute, case-insensitive, `/` and `\\`
/// unified. Different sources spell the same install differently
/// (`C:/Program Files/...` vs `C:\\Program Files\\...`).
fn canonical_key(path: &Path) -> Option<String> {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
        .replace('/', "\\")
        .into()
}

fn push_candidate(java_exe: PathBuf, out: &mut Vec<InstalledJava>) {
    // Store a consistent native-looking path (roots are scanned with `/`,
    // PATH entries arrive with `\\`).
    let java_exe = PathBuf::from(java_exe.to_string_lossy().replace('/', "\\"));
    let home = java_exe
        .parent()
        .and_then(|bin| bin.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| java_exe.clone());
    let key = canonical_key(&home);
    if let Some(key) = key {
        if out
            .iter()
            .any(|j| canonical_key(&j.home).is_some_and(|k| k == key))
        {
            return;
        }
    }
    let Some((major, version)) = probe(&java_exe) else {
        return;
    };
    let vendor = vendor_from_path(&home);
    out.push(InstalledJava {
        path: java_exe,
        home,
        major,
        version,
        vendor,
    });
}

/// Extra scan roots: runtimes downloaded by the launcher and `JAVA_HOME`.
fn extra_java_roots(runtimes_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dir) = runtimes_dir {
        roots.push(dir.to_path_buf());
    }
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        if !home.is_empty() {
            roots.push(PathBuf::from(home));
        }
    }
    roots
}

/// Inventory every installed Java on this machine: the well-known Windows
/// install roots, the launcher's own `runtimes/` downloads, `JAVA_HOME` and
/// every `PATH` entry — including installations that are not on `PATH`.
/// Each candidate is verified with a `java -version` probe.
pub fn detect_installed_javas(runtimes_dir: Option<&Path>) -> Vec<InstalledJava> {
    let mut out: Vec<InstalledJava> = Vec::new();

    // Roots whose direct children are runtime homes.
    let mut home_roots = common_java_roots();
    home_roots.extend(extra_java_roots(runtimes_dir));
    for root in &home_roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            for bin_dir in [dir.join("bin"), dir.join("jre/bin")] {
                for name in java_binary_names() {
                    push_candidate(bin_dir.join(name), &mut out);
                }
            }
            // Nested one level deeper (e.g. runtimes/<name>/<jdk-home>/bin).
            if let Ok(nested) = std::fs::read_dir(&dir) {
                for nested_entry in nested.flatten() {
                    let nested_dir = nested_entry.path();
                    if !nested_dir.is_dir() {
                        continue;
                    }
                    for name in java_binary_names() {
                        push_candidate(nested_dir.join("bin").join(name), &mut out);
                    }
                }
            }
        }
    }

    // Anything on PATH (may point outside the roots above).
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        for name in java_binary_names() {
            push_candidate(dir.join(name), &mut out);
        }
    }

    // Drop nested runtimes: a `<jdk>/jre` inside an already-found `<jdk>`
    // (in either scan order) is the same installation — keep the outer home.
    let keys: Vec<Option<String>> = out.iter().map(|j| canonical_key(&j.home)).collect();
    let mut keep: Vec<bool> = keys.iter().map(|_| true).collect();
    for (i, ki) in keys.iter().enumerate() {
        let Some(ki) = ki else { continue };
        for (j, kj) in keys.iter().enumerate() {
            if i == j || !keep[j] {
                continue;
            }
            let Some(kj) = kj else { continue };
            if kj.starts_with(ki.as_str())
                && kj.len() > ki.len()
                && kj[ki.len()..].starts_with('\\')
            {
                keep[j] = false; // j is nested inside i
            }
        }
    }
    let out: Vec<InstalledJava> = out
        .into_iter()
        .zip(keep)
        .filter_map(|(j, k)| k.then_some(j))
        .collect();

    // Newest first; ties fall back to path for a stable order.
    let mut out = out;
    out.sort_by(|a, b| b.major.cmp(&a.major).then(a.path.cmp(&b.path)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // machine-specific: run with `cargo test -- --ignored --nocapture` locally
    fn detect_prints_installed_javas() {
        for j in detect_installed_javas(None) {
            println!(
                "{:>2} {:<10} {} -> {}",
                j.major,
                j.vendor,
                j.version,
                j.path.display()
            );
        }
    }

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

    #[test]
    fn parses_full_version_token() {
        let out = "openjdk version \"21.0.5\" 2024-10-15\nOpenJDK Runtime Environment";
        assert_eq!(parse_java_full_version(out).as_deref(), Some("21.0.5"));
        let out = "java version \"1.8.0_402\"\nJava(TM) SE Runtime Environment";
        assert_eq!(parse_java_full_version(out).as_deref(), Some("1.8.0_402"));
        assert_eq!(parse_java_full_version("garbage"), None);
    }

    #[test]
    fn vendor_labels_from_paths() {
        let f = |p: &str| vendor_from_path(Path::new(p));
        assert_eq!(
            f("C:/Program Files/Eclipse Adoptium/jdk-21.0.5+11-hotspot"),
            "Temurin"
        );
        assert_eq!(f("C:/Program Files/Zulu/zulu-21"), "Zulu");
        assert_eq!(
            f("C:/Program Files/Amazon Corretto/jdk21.0.5_11"),
            "Corretto"
        );
        assert_eq!(f("C:/Program Files/Microsoft/jdk-21.0.5+11"), "Microsoft");
        assert_eq!(f("C:/Program Files/Java/jdk-17"), "Oracle");
        assert_eq!(f("C:/Program Files/Java/jdk1.8.0_402"), "Oracle");
        assert_eq!(f("C:/home/runtimes/Temurin-21.0.5+11"), "Temurin");
        assert_eq!(f("C:/somewhere/plain-jdk"), "Java");
    }

    #[test]
    fn labels_combine_vendor_and_version() {
        let j = InstalledJava {
            path: PathBuf::from("C:/x/bin/java.exe"),
            home: PathBuf::from("C:/x"),
            major: 21,
            version: "21.0.5".into(),
            vendor: "Temurin".into(),
        };
        assert_eq!(j.label(), "Temurin 21.0.5");
    }
}
