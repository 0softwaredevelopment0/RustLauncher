//! Building and running the Minecraft launch command.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::auth::Account;
use crate::classpath::build_classpath;
use crate::java_locator;
use crate::lang::{tr, tr_fmt, Language};
use crate::version_json::VersionJson;

/// Module-access flags for modded launchers on modern JVMs.
///
/// Each `--add-opens` must be a complete `module/package=ALL-UNNAMED`
/// specifier: the JVM parses the flag and its value together, so pushing
/// them as two separate argv entries (`--add-opens`, `java.base/java.net`)
/// fails boot with `Unable to parse --add-opens <module>=<value>`.
pub fn module_access_flags(java_major: u32) -> Vec<String> {
    if java_major < 9 {
        return Vec::new();
    }
    let mut flags = Vec::new();
    for module in [
        "java.base/java.net",
        "java.base/java.lang",
        "java.base/java.lang.reflect",
        "java.base/java.util",
        "java.base/java.io",
        "java.base/java.nio",
        "java.base/sun.nio.ch",
        "java.base/java.security",
    ] {
        flags.push(format!("--add-opens={module}=ALL-UNNAMED"));
    }
    if java_major >= 22 {
        flags.push("--enable-native-access=ALL-UNNAMED".to_string());
    }
    if java_major >= 26 {
        flags.push("--enable-final-field-mutation=ALL-UNNAMED".to_string());
    }
    flags
}

/// Drop JVM args that can break the game or the machine: proxy/DNS hijacks,
/// TLS downgrades, agent injection, JDWP. Heap flags (`-Xms`/`-Xmx`) are NOT
/// filtered — they are the legitimate way RAM is configured now.
pub fn filter_custom_java_args(custom: &str) -> Vec<String> {
    const BLOCKED_PREFIXES: &[&str] = &[
        "-dproxyhost",
        "-dproxyport",
        "-dhttp.proxyhost",
        "-dhttp.proxyport",
        "-dhttps.proxyhost",
        "-dhttps.proxyport",
        "-dhttp.nonproxyhosts",
        "-dsocksproxyhost",
        "-dsocksproxyport",
        "-dsocksproxypasswd",
        "-djava.net.preferipv4stack",
        "-djava.security.manager",
        "-djava.security.policy",
        "-dsun.net.spi.nameservice.nameservers",
        "-dcom.sun.net.ssl.checkrevocation",
        "-djavax.net.ssl.truststore",
        "-djavax.net.ssl.truststorepassword",
        "-djavax.net.ssl.keystore",
        "-dhttps.protocols",
        "-djdk.tls.client.protocols",
        "-xbootclasspath",
        "-agentlib:",
        "-javaagent:",
        "-xrunjdwp:",
    ];

    custom
        .split_whitespace()
        .map(str::trim)
        .filter(|arg| !arg.is_empty())
        .filter(|arg| {
            let lower = arg.to_ascii_lowercase();
            let blocked = BLOCKED_PREFIXES.iter().any(|p| lower.starts_with(p));
            if blocked {
                eprintln!("[RustLauncher] Dropped potentially unsafe arg: {arg}");
            }
            !blocked
        })
        .map(str::to_string)
        .collect()
}

/// Parse `host[:port]` into `(host, port)` with the default 25565.
pub fn split_server_address(addr: &str) -> (String, String) {
    match addr.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
            (host.to_string(), port.to_string())
        }
        _ => (addr.to_string(), "25565".to_string()),
    }
}

/// Extract native libraries (`.dll/.so/.dylib`) from `*natives*.jar` files
/// under the libraries directory into the natives directory.
pub fn extract_natives(
    libraries_dir: &Path,
    natives_dir: &Path,
    lang: Language,
) -> Result<usize> {
    let mut count = 0;
    for jar in crate::classpath::collect_jars(libraries_dir) {
        let Some(name) = jar.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.contains("natives") {
            continue;
        }
        let file = std::fs::File::open(&jar)
            .with_context(|| tr_fmt(lang, "failed to open {0}", &[&jar.display().to_string()]))?;
        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| tr_fmt(lang, "failed to read {0}", &[&jar.display().to_string()]))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let entry_name = entry.name().to_string();
            if !entry_name.ends_with(".dll")
                && !entry_name.ends_with(".so")
                && !entry_name.ends_with(".dylib")
            {
                continue;
            }
            if entry.is_dir() {
                continue;
            }
            let out_path = natives_dir.join(&entry_name);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
            count += 1;
        }
    }
    Ok(count)
}

/// Fully resolved launch plan (also used for dry-run output).
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub java: PathBuf,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
}

/// Assemble the full launch command for a version.
#[allow(clippy::too_many_arguments)]
pub fn build_launch_plan(
    game_dir: &Path,
    version_name: &str,
    version_json: &VersionJson,
    version_jar: &Path,
    account: &Account,
    java_args: &str,
    custom_java_path: Option<&str>,
    server: Option<&str>,
    resolution: Option<(u32, u32)>,
    lang: Language,
) -> Result<LaunchPlan> {
    let parsed_args: Vec<String> = java_args
        .split_whitespace()
        .map(str::trim)
        .filter(|arg| !arg.is_empty())
        .map(str::to_string)
        .collect();
    // The game cannot start without heap flags; presets guarantee them,
    // manual edits are checked here.
    crate::jvm::validate_jvm_args(&parsed_args, lang).map_err(|e| anyhow::anyhow!(e))?;

    let required_major = version_json.required_java_major().or(Some(21)).or(None);
    let required_major = required_major.unwrap_or(21);

    let java = java_locator::select_java(custom_java_path, Some(required_major), lang)?;
    let java_major = java_locator::selected_java_major(&java, lang)?;

    let natives_dir = game_dir.join("natives");
    let assets_dir = game_dir.join("assets");
    std::fs::create_dir_all(&natives_dir).context(tr(lang, "failed to create natives dir"))?;
    std::fs::create_dir_all(&assets_dir).context(tr(lang, "failed to create assets dir"))?;

    let libraries_dir = game_dir.join("libraries");
    if libraries_dir.is_dir() {
        let extracted = extract_natives(&libraries_dir, &natives_dir, lang)
            .context(tr(lang, "failed to extract native libraries"))?;
        if extracted > 0 {
            println!(
                "[RustLauncher] {}",
                tr_fmt(lang, "Extracted {0} native libraries", &[&extracted.to_string()])
            );
        }
    }

    let classpath = build_classpath(game_dir, version_name, version_json, version_jar)?;

    let mut args: Vec<String> = Vec::new();

    // User JVM flags first: heap flags (-Xms/-Xmx) must come before the
    // main class and are typically overridden only by later duplicates.
    args.extend(filter_custom_java_args(java_args));

    // Short DNS TTL — do NOT force IPv4 (it breaks IPv6-only networks).
    args.push("-Dsun.net.inetaddr.ttl=0".to_string());

    // Module access needed by modloaders on modern JVMs.
    args.extend(module_access_flags(java_major));

    args.push(format!("-Djava.library.path={}", natives_dir.display()));
    args.push("-cp".to_string());
    args.push(classpath);
    args.push(version_json.main_class().to_string());

    // Game args (order matters for some versions).
    args.push("--username".into());
    args.push(account.username.clone());
    args.push("--uuid".into());
    args.push(account.uuid.replace('-', ""));
    args.push("--accessToken".into());
    // Online accounts pass their real session token; offline accounts use
    // their UUID (the vanilla offline-mode convention).
    args.push(if account.access_token.is_empty() {
        account.uuid.clone()
    } else {
        account.access_token.clone()
    });
    args.push("--version".into());
    args.push(version_name.to_string());
    args.push("--gameDir".into());
    args.push(game_dir.to_string_lossy().to_string());
    args.push("--assetsDir".into());
    args.push(assets_dir.to_string_lossy().to_string());
    args.push("--assetIndex".into());
    args.push(version_json.asset_index_id(version_name).to_string());
    args.push("--userProperties".into());
    args.push("{}".to_string());
    args.push("--userType".into());
    args.push("legacy".to_string());
    args.push("--clientVersion".into());
    args.push(version_name.to_string());
    args.push("--xuid".into());
    args.push(String::new());
    args.push("--clientId".into());
    args.push(String::new());

    if let Some((width, height)) = resolution {
        args.push("--width".into());
        args.push(width.to_string());
        args.push("--height".into());
        args.push(height.to_string());
    }

    if let Some(server) = server.filter(|s| !s.trim().is_empty()) {
        let (host, port) = split_server_address(server.trim());
        args.push("--server".into());
        args.push(host);
        args.push("--port".into());
        args.push(port);
    }

    Ok(LaunchPlan {
        java,
        args,
        working_dir: game_dir.to_path_buf(),
    })
}

/// A line handler that is callable from multiple reader threads.
pub type LineHandler = std::sync::Arc<std::sync::Mutex<dyn FnMut(&str) + Send>>;

/// Wrap a closure into a shareable line handler.
pub fn line_handler(mut f: impl FnMut(&str) + Send + 'static) -> LineHandler {
    std::sync::Arc::new(std::sync::Mutex::new(move |line: &str| f(line)))
}

/// Run the launch plan, streaming child output through `on_line`.
/// Returns the child's exit code. The child can never block on a full pipe
/// because both streams are drained concurrently.
pub fn run_plan(plan: LaunchPlan, on_line: LineHandler) -> Result<i32> {
    use std::io::Write;
    let _ = std::io::stdout().flush();

    println!(
        "[RustLauncher] Launching: {} ({})",
        plan.java.display(),
        plan.working_dir.display()
    );

    let mut child = Command::new(&plan.java)
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {}", plan.java.display()))?;

    let stdout = child.stdout.take().context("no stdout")?;
    let stderr = child.stderr.take().context("no stderr")?;

    fn drain(
        stream: impl std::io::Read + Send + 'static,
        print: LineHandler,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if let Ok(mut f) = print.lock() {
                    f(&line);
                }
            }
        })
    }

    let t_out = drain(stdout, on_line.clone());
    let t_err = drain(stderr, on_line);

    let _ = t_out.join();
    let _ = t_err.join();

    let status = child.wait()?;
    Ok(status.code().unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_flags_are_complete_specifiers() {
        let flags = module_access_flags(25);
        // Every --add-opens must carry its target in the SAME argv entry:
        // the JVM parses flag+value together, `--add-opens module/pkg`
        // without `=ALL-UNNAMED` fails boot layer initialization.
        let opens: Vec<&String> = flags
            .iter()
            .filter(|f| f.starts_with("--add-opens"))
            .collect();
        assert!(!opens.is_empty());
        for f in opens {
            let rest = f.trim_start_matches("--add-opens=");
            assert!(
                rest.ends_with("=ALL-UNNAMED"),
                "incomplete --add-opens specifier: {f}"
            );
        }
        assert!(flags.contains(&"--enable-native-access=ALL-UNNAMED".to_string()));
        assert!(!flags.iter().any(|f| f.contains("final-field-mutation")));
    }

    #[test]
    fn old_java_gets_no_module_flags_new_java_gets_final_field_mutation() {
        assert!(module_access_flags(8).is_empty());
        let modern = module_access_flags(26);
        assert!(modern
            .iter()
            .any(|f| f.contains("--enable-final-field-mutation=ALL-UNNAMED")));
    }

    #[test]
    fn filters_unsafe_args_but_keeps_heap_flags() {
        let kept = filter_custom_java_args(
            "-XX:+UseG1GC -Xmx8G -DproxyHost=evil -javaagent:x.jar -Dfoo=bar",
        );
        assert_eq!(
            kept,
            vec![
                "-XX:+UseG1GC".to_string(),
                "-Xmx8G".to_string(),
                "-Dfoo=bar".to_string()
            ]
        );
    }

    #[test]
    fn filters_are_case_insensitive() {
        // Blocklist matching ignores case; note that a bare `-Djava.agent=...`
        // property is inert in Java, while the real flags `-javaagent:` and
        // proxy properties are what must be dropped in any casing.
        let kept = filter_custom_java_args("-JAVAAGENT:x.jar -DPROXYHOST=evil");
        assert!(
            kept.is_empty(),
            "agent and proxy args must be dropped regardless of case"
        );
    }

    #[test]
    fn empty_custom_args_produce_nothing() {
        assert!(filter_custom_java_args("").is_empty());
        assert!(filter_custom_java_args("   ").is_empty());
    }

    #[test]
    fn splits_server_addresses() {
        assert_eq!(
            split_server_address("mc.example.com:25566"),
            ("mc.example.com".into(), "25566".into())
        );
        assert_eq!(
            split_server_address("mc.example.com"),
            ("mc.example.com".into(), "25565".into())
        );
        assert_eq!(
            split_server_address("host:"),
            ("host:".into(), "25565".into())
        );
    }
}
