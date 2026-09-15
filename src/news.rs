//! News feed for the launcher: JSON from a configurable URL with built-in
//! fallback items (port of the Java `NewsManager`; the URL placeholder bug —
//! it pointed at `your-repo` — is now surfaced as a visible error instead of
//! a silent fallback).

use anyhow::Result;
use serde::Deserialize;

use crate::net;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NewsItem {
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub date: String,
    #[serde(default, rename = "type")]
    pub item_type: String,
    #[serde(default, rename = "imageUrl")]
    pub image_url: String,
}

impl NewsItem {
    pub fn new(title: &str, content: &str, date: &str, item_type: &str) -> NewsItem {
        NewsItem {
            title: title.to_string(),
            content: content.to_string(),
            date: date.to_string(),
            item_type: item_type.to_string(),
            image_url: String::new(),
        }
    }
}

/// Default news shown when nothing was fetched yet.
pub fn fallback_news() -> Vec<NewsItem> {
    vec![
        NewsItem::new(
            "Welcome to RustLauncher!",
            "A fast native Minecraft launcher, rewritten in Rust. \
             Select an account, pick a version and press Play.",
            "01.01.2025 12:00",
            "info",
        ),
        NewsItem::new(
            "How to get started",
            "1. Add your nickname in Accounts\n2. Pick a version (install one \
             from the Catalog if needed)\n3. Configure RAM in Settings\n4. Press Play!",
            "01.01.2025 12:00",
            "guide",
        ),
        NewsItem::new(
            "Migrated from PowerLaunch",
            "All launch features of the Java launcher are here: server list \
             sync, skins, diagnostics and numbered session logs.",
            "01.01.2025 12:00",
            "update",
        ),
    ]
}

/// Fetch the news feed. Returns parsed items or an error the caller can show.
pub fn fetch(agent: &ureq::Agent, url: &str) -> Result<Vec<NewsItem>> {
    let body = net::get_string(agent, url)?;
    let items: Vec<NewsItem> =
        serde_json::from_str(&body).map_err(|e| anyhow::anyhow!("bad news format: {e}"))?;
    if items.is_empty() {
        return Err(anyhow::anyhow!("news feed is empty"));
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_feed_with_optional_fields() {
        let body = r#"[
            {"title": "T1", "content": "C1", "date": "d", "type": "info"},
            {"title": "T2"}
        ]"#;
        let items: Vec<NewsItem> = serde_json::from_str(body).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].item_type, "info");
        assert!(items[1].content.is_empty());
    }

    #[test]
    fn fallback_news_is_never_empty() {
        assert!(!fallback_news().is_empty());
        assert!(fallback_news().iter().all(|n| !n.title.is_empty()));
    }
}
