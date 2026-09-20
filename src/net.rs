//! Shared HTTP client for Mojang services and the news feed.

use std::io::Read;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use crate::lang::{tr_fmt, Language};

/// A lazily created agent used by all HTTP calls.
pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .user_agent("RustLauncher/0.2")
        .build()
}

/// GET a URL and return the body as bytes with helpful error classification.
pub fn get_bytes(agent: &ureq::Agent, url: &str, lang: Language) -> Result<Vec<u8>> {
    let response = agent.get(url).call().map_err(|e| classify(e, lang))?;
    let status = response.status();
    if status != 200 {
        return Err(anyhow!(
            "{}",
            tr_fmt(lang, "HTTP {0} for {1}", &[&status.to_string(), url])
        ));
    }
    let reader = response.into_reader();
    let mut limited = reader.take(512 * 1024 * 1024);
    let mut buf = Vec::new();
    limited
        .read_to_end(&mut buf)
        .with_context(|| tr_fmt(lang, "failed to read body of {0}", &[url]))?;
    Ok(buf)
}

/// GET a URL and return the body as a UTF-8 string.
pub fn get_string(agent: &ureq::Agent, url: &str, lang: Language) -> Result<String> {
    let bytes = get_bytes(agent, url, lang)?;
    String::from_utf8(bytes).map_err(|e| {
        anyhow!(
            "{}",
            tr_fmt(lang, "non-UTF-8 body from {0}: {1}", &[url, &e.to_string()])
        )
    })
}

/// Map ureq transport errors to human-readable causes (port of the Java
/// error classification the launcher printed for news/skins/updater).
pub fn classify(err: ureq::Error, lang: Language) -> anyhow::Error {
    match err {
        ureq::Error::Status(code, response) => {
            let url = response.get_url().to_string();
            anyhow!(
                "{}",
                tr_fmt(lang, "HTTP {0} for {1}", &[&code.to_string(), &url])
            )
        }
        ureq::Error::Transport(t) => {
            let message = t.to_string();
            match t.kind() {
                ureq::ErrorKind::Dns => anyhow!(
                    "{}",
                    tr_fmt(lang, "DNS resolution failed: {0}", &[&message])
                ),
                ureq::ErrorKind::ConnectionFailed => {
                    anyhow!("{}", tr_fmt(lang, "connection failed: {0}", &[&message]))
                }
                _ => anyhow!("{}", tr_fmt(lang, "network error: {0}", &[&message])),
            }
        }
    }
}
