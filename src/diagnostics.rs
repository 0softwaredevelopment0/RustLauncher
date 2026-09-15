//! Network diagnostics: DNS, HTTP reachability of Mojang services and direct
//! TCP tests to Minecraft servers (port of the Java `NetworkDiagnostics`).

use std::net::ToSocketAddrs;
use std::time::Duration;

const TEST_URLS: &[&str] = &[
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
    "https://api.mojang.com/users/profiles/minecraft/Test",
];

const TEST_SERVERS: &[&str] = &["mc.hypixel.net:25565", "google.com:80"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// DNS resolve a host (Minecraft-style, first addresses).
pub fn dns_check(host: &str) -> CheckResult {
    let name = format!("DNS {host}");
    match (host, 0u16).to_socket_addrs() {
        Ok(addrs) => {
            let ips: Vec<String> = addrs.take(4).map(|a| a.ip().to_string()).collect();
            if ips.is_empty() {
                CheckResult {
                    name,
                    ok: false,
                    detail: "no addresses returned".into(),
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

/// HTTP GET probe.
pub fn http_check(agent: &ureq::Agent, url: &str) -> CheckResult {
    let name = format!("HTTP {url}");
    match agent.get(url).call() {
        Ok(resp) => {
            let status = resp.status();
            let detail = if resp.header("content-length").is_some() {
                format!("HTTP {status}")
            } else {
                format!("HTTP {status} (streamed)")
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
            detail: crate::net::classify(e).to_string(),
        },
    }
}

/// Direct TCP connect probe with a short timeout.
pub fn tcp_check(address: &str, timeout: Duration) -> CheckResult {
    let name = format!("TCP {address}");
    match address.to_socket_addrs() {
        Err(e) => CheckResult {
            name,
            ok: false,
            detail: format!("DNS failed: {e}"),
        },
        Ok(mut addrs) => {
            let Some(addr) = addrs.next() else {
                return CheckResult {
                    name,
                    ok: false,
                    detail: "DNS returned no addresses".into(),
                };
            };
            match std::net::TcpStream::connect_timeout(&addr, timeout) {
                Ok(_) => CheckResult {
                    name,
                    ok: true,
                    detail: format!("connected to {addr}"),
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

/// Run the full suite in order.
pub fn run_all(agent: &ureq::Agent) -> Vec<CheckResult> {
    let mut results = Vec::new();
    results.push(CheckResult {
        name: "Platform".into(),
        ok: true,
        detail: format!("{} / RustLauncher", std::env::consts::OS),
    });
    for host in [
        "piston-meta.mojang.com",
        "api.mojang.com",
        "resources.download.minecraft.net",
    ] {
        results.push(dns_check(host));
    }
    for url in TEST_URLS {
        results.push(http_check(agent, url));
    }
    for server in TEST_SERVERS {
        results.push(tcp_check(server, Duration::from_secs(3)));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_check_reports_localhost() {
        let r = dns_check("localhost");
        assert!(r.ok, "{}: {}", r.name, r.detail);
    }

    #[test]
    fn tcp_check_reports_bad_host_gracefully() {
        let r = tcp_check("no.such.host.invalid:12345", Duration::from_secs(1));
        assert!(!r.ok);
        assert!(!r.detail.is_empty());
    }
}
