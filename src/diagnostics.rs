//! Diagnostics tab: local system checks (platform, game directory, Java
//! runtimes) plus network probes — DNS, HTTP reachability of Mojang/Modrinth/
//! news services and direct TCP tests to Minecraft servers (port of the Java
//! `NetworkDiagnostics`).

use std::net::ToSocketAddrs;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::lang::{tr, tr_fmt, Language};

const TEST_URLS: &[&str] = &[
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json",
    "https://api.modrinth.com/v2/projects?limit=1",
];

const TEST_SERVERS: &[&str] = &["mc.hypixel.net:25565", "google.com:80"];

const DNS_HOSTS: &[&str] = &[
    "piston-meta.mojang.com",
    "api.mojang.com",
    "resources.download.minecraft.net",
    "libraries.minecraft.net",
    "sessionserver.mojang.com",
    "textures.minecraft.net",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// Launcher state the local checks need; filled from settings before the
/// background job starts.
#[derive(Debug, Clone, Default)]
pub struct DiagInput {
    /// Configured game root directory (empty = not configured).
    pub game_directory: String,
    /// News feed site base URL (empty = no feed check).
    pub news_url: String,
    /// Explicitly selected Java executable, if a concrete runtime is picked.
    pub java_path: Option<String>,
    /// Launcher home directory; its `runtimes/` folder is scanned too.
    pub home_dir: Option<std::path::PathBuf>,
}

fn millis(elapsed: Instant) -> String {
    format!("{} ms", elapsed.elapsed().as_millis())
}

/// Platform summary: OS, architecture, logical cores, launcher version.
pub fn platform_check(lang: Language) -> CheckResult {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    CheckResult {
        name: tr(lang, "Platform").into(),
        ok: true,
        detail: format!(
            "{} / {} / {} cores / RustLauncher v{}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            cores,
            env!("CARGO_PKG_VERSION")
        ),
    }
}

/// The configured game directory must exist and be writable.
pub fn game_dir_check(dir: &str, lang: Language) -> CheckResult {
    let name = tr(lang, "Game directory").into();
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return CheckResult {
            name,
            ok: false,
            detail: tr(lang, "not configured").into(),
        };
    }
    let path = std::path::PathBuf::from(trimmed);
    if !path.is_dir() {
        return CheckResult {
            name,
            ok: false,
            detail: tr_fmt(
                lang,
                "directory does not exist: {0}",
                &[&path.display().to_string()],
            ),
        };
    }
    let probe_file = path.join(".__rustlauncher_write_test");
    match std::fs::write(&probe_file, b"ok") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe_file);
            CheckResult {
                name,
                ok: true,
                detail: path.display().to_string(),
            }
        }
        Err(e) => CheckResult {
            name,
            ok: false,
            detail: tr_fmt(lang, "not writable: {0}", &[&e.to_string()]),
        },
    }
}

/// How many Java runtimes are installed on this machine (system roots plus
/// the launcher's own `runtimes/` folder).
pub fn runtimes_check(home_dir: Option<&Path>, lang: Language) -> CheckResult {
    let name = tr(lang, "Detected Java runtimes").into();
    let runtimes = home_dir.map(|h| h.join("runtimes"));
    let found = crate::java_locator::detect_installed_javas(runtimes.as_deref());
    if found.is_empty() {
        if crate::java_locator::bundled_java().is_some() {
            CheckResult {
                name,
                ok: true,
                detail: tr(lang, "none found; bundled runtime available").into(),
            }
        } else {
            CheckResult {
                name,
                ok: false,
                detail: tr(lang, "none found").into(),
            }
        }
    } else {
        let mut majors: Vec<u32> = found.iter().map(|j| j.major).collect();
        majors.sort_unstable();
        majors.dedup();
        let list = majors
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        CheckResult {
            name,
            ok: true,
            detail: tr_fmt(
                lang,
                "{0} found (Java {1})",
                &[&found.len().to_string(), &list],
            ),
        }
    }
}

/// The Java executable games would launch with right now, verified by
/// actually running `java -version`.
pub fn selected_java_check(configured: Option<&str>, lang: Language) -> CheckResult {
    let name = tr(lang, "Selected Java").into();
    match crate::java_locator::select_java(configured, None, lang) {
        Err(e) => CheckResult {
            name,
            ok: false,
            detail: e.to_string(),
        },
        Ok(path) => match crate::java_locator::probe(&path) {
            Some((major, version)) => CheckResult {
                name,
                ok: true,
                detail: format!("{} (Java {major}, {version})", path.display()),
            },
            None => CheckResult {
                name,
                ok: false,
                detail: tr_fmt(
                    lang,
                    "java -version failed for {0}",
                    &[&path.display().to_string()],
                ),
            },
        },
    }
}

/// DNS resolve a host (Minecraft-style, first addresses).
pub fn dns_check(host: &str, lang: Language) -> CheckResult {
    let name = format!("DNS {host}");
    match (host, 0u16).to_socket_addrs() {
        Ok(addrs) => {
            let ips: Vec<String> = addrs.take(4).map(|a| a.ip().to_string()).collect();
            if ips.is_empty() {
                CheckResult {
                    name,
                    ok: false,
                    detail: tr(lang, "no addresses returned").into(),
                }
            } else {
                CheckResult {
                    name,
                    ok: true,
                    detail: ips.join(", "),
                }
            }
        }
        Err(e) => CheckResult {
            name,
            ok: false,
            detail: e.to_string(),
        },
    }
}

/// HTTP GET probe with a latency measurement.
pub fn http_check(agent: &ureq::Agent, url: &str, lang: Language) -> CheckResult {
    let name = format!("HTTP {url}");
    let started = Instant::now();
    match agent.get(url).call() {
        Ok(resp) => {
            let status = resp.status();
            let latency = millis(started);
            let detail = if resp.header("content-length").is_some() {
                tr_fmt(lang, "HTTP {0} ({1})", &[&status.to_string(), &latency])
            } else {
                tr_fmt(
                    lang,
                    "HTTP {0} streamed ({1})",
                    &[&status.to_string(), &latency],
                )
            };
            CheckResult {
                name,
                ok: status == 200,
                detail,
            }
        }
        Err(e) => CheckResult {
            name,
            ok: false,
            detail: crate::net::classify(e, lang).to_string(),
        },
    }
}

/// Direct TCP connect probe with a short timeout and latency measurement.
pub fn tcp_check(address: &str, timeout: Duration, lang: Language) -> CheckResult {
    let name = format!("TCP {address}");
    match address.to_socket_addrs() {
        Err(e) => CheckResult {
            name,
            ok: false,
            detail: tr_fmt(lang, "DNS failed: {0}", &[&e.to_string()]),
        },
        Ok(mut addrs) => {
            let Some(addr) = addrs.next() else {
                return CheckResult {
                    name,
                    ok: false,
                    detail: tr(lang, "DNS returned no addresses").into(),
                };
            };
            let started = Instant::now();
            match std::net::TcpStream::connect_timeout(&addr, timeout) {
                Ok(_) => CheckResult {
                    name,
                    ok: true,
                    detail: tr_fmt(
                        lang,
                        "connected to {0} ({1})",
                        &[&addr.to_string(), &millis(started)],
                    ),
                },
                Err(e) => CheckResult {
                    name,
                    ok: false,
                    detail: e.to_string(),
                },
            }
        }
    }
}

/// A single diagnostic check that can be run independently: its display
/// name (shown as a pending row until it finishes) and the task itself.
pub struct DiagTask {
    pub name: String,
    pub run: Box<dyn FnOnce() -> CheckResult + Send>,
}

/// Build the ordered list of checks without running them; the caller spawns
/// each task in the background so results stream in one by one.
pub fn suite(agent: ureq::Agent, lang: Language, input: DiagInput) -> Vec<DiagTask> {
    let mut tasks: Vec<DiagTask> = Vec::new();
    tasks.push(DiagTask {
        name: tr(lang, "Platform").into(),
        run: Box::new(move || platform_check(lang)),
    });
    let dir = input.game_directory.clone();
    tasks.push(DiagTask {
        name: tr(lang, "Game directory").into(),
        run: Box::new(move || game_dir_check(&dir, lang)),
    });
    let home = input.home_dir.clone();
    tasks.push(DiagTask {
        name: tr(lang, "Detected Java runtimes").into(),
        run: Box::new(move || runtimes_check(home.as_deref(), lang)),
    });
    let java = input.java_path.clone();
    tasks.push(DiagTask {
        name: tr(lang, "Selected Java").into(),
        run: Box::new(move || selected_java_check(java.as_deref(), lang)),
    });
    for host in DNS_HOSTS {
        tasks.push(DiagTask {
            name: format!("DNS {host}"),
            run: Box::new(move || dns_check(host, lang)),
        });
    }
    for url in TEST_URLS {
        let agent = agent.clone();
        tasks.push(DiagTask {
            name: format!("HTTP {url}"),
            run: Box::new(move || http_check(&agent, url, lang)),
        });
    }
    let news = input.news_url.trim().trim_end_matches('/').to_string();
    if !news.is_empty() {
        let url = format!("{news}/api/news");
        tasks.push(DiagTask {
            name: format!("HTTP {url}"),
            run: Box::new(move || http_check(&agent, &url, lang)),
        });
    }
    for server in TEST_SERVERS {
        tasks.push(DiagTask {
            name: format!("TCP {server}"),
            run: Box::new(move || tcp_check(server, Duration::from_secs(3), lang)),
        });
    }
    tasks
}

/// Plain-text report for the clipboard (the Copy report button).
pub fn report_text(results: &[CheckResult]) -> String {
    let passed = results.iter().filter(|r| r.ok).count();
    let mut out = format!(
        "RustLauncher diagnostics: {passed}/{} passed\n",
        results.len()
    );
    for r in results {
        out.push_str(&format!(
            "[{}] {} — {}\n",
            if r.ok { " OK " } else { "FAIL" },
            r.name,
            r.detail
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Language;

    #[test]
    fn dns_check_reports_localhost() {
        let r = dns_check("localhost", Language::English);
        assert!(r.ok, "{}: {}", r.name, r.detail);
    }

    #[test]
    fn dns_check_reports_bad_host_gracefully() {
        let r = dns_check("no.such.host.invalid", Language::English);
        assert!(!r.ok);
        assert!(!r.detail.is_empty());
    }

    #[test]
    fn tcp_check_reports_bad_host_gracefully() {
        let r = tcp_check(
            "no.such.host.invalid:12345",
            Duration::from_secs(1),
            Language::English,
        );
        assert!(!r.ok);
        assert!(!r.detail.is_empty());
    }

    #[test]
    fn tcp_check_reports_latency_for_local_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let r = tcp_check(&addr.to_string(), Duration::from_secs(1), Language::English);
        assert!(r.ok, "{}", r.detail);
        assert!(r.detail.contains("ms"), "{}", r.detail);
    }

    #[test]
    fn game_dir_check_flags_unconfigured_and_missing() {
        let r = game_dir_check("", Language::English);
        assert!(!r.ok);
        let r = game_dir_check("   ", Language::English);
        assert!(!r.ok);
        let r = game_dir_check("Z:/definitely/missing/dir", Language::English);
        assert!(!r.ok, "{}", r.detail);
        assert!(!r.detail.is_empty());
    }

    #[test]
    fn game_dir_check_accepts_writable_dir() {
        let dir = std::env::temp_dir().join(format!(
            "rl-diag-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .elapsed()
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let r = game_dir_check(dir.to_str().unwrap(), Language::English);
        assert!(r.ok, "{}", r.detail);
        // No leftover probe file.
        assert!(!dir.join(".__rustlauncher_write_test").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn selected_java_check_reports_bogus_path() {
        let r = selected_java_check(Some("Z:/no/such/java.exe"), Language::English);
        assert!(!r.ok);
        assert!(!r.detail.is_empty());
    }

    #[test]
    fn platform_check_reports_os_and_version() {
        let r = platform_check(Language::English);
        assert!(r.ok);
        assert!(r.detail.contains(std::env::consts::OS), "{}", r.detail);
        assert!(r.detail.contains("RustLauncher"), "{}", r.detail);
    }

    #[test]
    fn report_text_summarizes_passes_and_failures() {
        let results = vec![
            CheckResult {
                name: "A".into(),
                ok: true,
                detail: "fine".into(),
            },
            CheckResult {
                name: "B".into(),
                ok: false,
                detail: "boom".into(),
            },
        ];
        let text = report_text(&results);
        assert!(text.contains("1/2 passed"), "{text}");
        assert!(text.contains("[ OK ] A"), "{text}");
        assert!(text.contains("[FAIL] B"), "{text}");
    }

    #[test]
    #[ignore = "scans the real machine; run manually to see the user-visible line"]
    fn runtimes_check_prints_summary() {
        let r = runtimes_check(None, Language::English);
        println!("{}: ok={} — {}", r.name, r.ok, r.detail);
    }
}
