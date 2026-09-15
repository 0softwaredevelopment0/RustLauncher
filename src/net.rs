//! Shared HTTP client for Mojang services and the news feed.

use std::io::Read;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

/// A lazily created agent used by all HTTP calls.
pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .user_agent("RustLauncher/0.2")
        .build()
}

/// GET a URL and return the body as bytes with helpful error classification.
pub fn get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    let response = agent.get(url).call().map_err(classify)?;
    let status = response.status();
    if status != 200 {
        return Err(anyhow!("HTTP {status} for {url}"));
    }
    let reader = response.into_reader();
    let mut limited = reader.take(512 * 1024 * 1024);
    let mut buf = Vec::new();
    limited
        .read_to_end(&mut buf)
        .with_context(|| format!("failed to read body of {url}"))?;
    Ok(buf)
}

/// GET a URL and return the body as a UTF-8 string.
pub fn get_string(agent: &ureq::Agent, url: &str) -> Result<String> {
    let bytes = get_bytes(agent, url)?;
    String::from_utf8(bytes).map_err(|e| anyhow!("non-UTF-8 body from {url}: {e}"))
}

/// Map ureq transport errors to human-readable causes (port of the Java
/// error classification the launcher printed for news/skins/updater).
pub fn classify(err: ureq::Error) -> anyhow::Error {
    match err {
        ureq::Error::Status(code, response) => {
            let url = response.get_url().to_string();
            anyhow!("HTTP {code} for {url}")
        }
        ureq::Error::Transport(t) => {
            let message = t.to_string();
            match t.kind() {
                ureq::ErrorKind::Dns => anyhow!("DNS resolution failed: {message}"),
                ureq::ErrorKind::ConnectionFailed => anyhow!("connection failed: {message}"),
                _ => anyhow!("network error: {message}"),
            }
        }
    }
}
