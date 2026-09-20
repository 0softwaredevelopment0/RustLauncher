//! News feed for the launcher: JSON from a configurable URL with built-in
//! fallback items (port of the Java `NewsManager`).

use anyhow::Result;
use serde::Deserialize;

use crate::lang::{tr, tr_fmt, Language};
use crate::net;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Author {
    pub name: String,
    #[serde(default)]
    pub image: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct NewsItem {
    #[serde(default)]
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default, rename = "createdAt")]
    pub created_at: String,
    #[serde(default, rename = "syncedToDiscord")]
    pub synced_to_discord: bool,
    #[serde(default)]
    pub author: Option<Author>,
}

#[allow(dead_code)]
impl NewsItem {
    pub fn new(title: &str, content: &str, date: &str) -> NewsItem {
        NewsItem {
            id: String::new(),
            title: title.to_string(),
            content: content.to_string(),
            slug: String::new(),
            created_at: date.to_string(),
            synced_to_discord: false,
            author: None,
        }
    }

    pub fn author_name(&self, lang: Language) -> String {
        self.author
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| tr(lang, "Unknown").to_string())
    }

    pub fn author_avatar(&self) -> Option<&str> {
        self.author.as_ref().and_then(|a| {
            if a.image.is_empty() {
                None
            } else {
                Some(a.image.as_str())
            }
        })
    }

    /// Format the ISO 8601 createdAt into a short date string.
    pub fn formatted_date(&self) -> String {
        // "2026-08-23T11:28:35.957Z" -> "23.08.2026"
        let s = &self.created_at;
        if s.len() >= 10 {
            let y = &s[..4];
            let m = &s[5..7];
            let d = &s[8..10];
            format!("{d}.{m}.{y}")
        } else {
            s.clone()
        }
    }
}

/// Default news shown when nothing was fetched yet.
pub fn fallback_news(lang: Language) -> Vec<NewsItem> {
    vec![
        NewsItem::new(
            tr(lang, "Welcome to RustLauncher!"),
            tr(
                lang,
                "A fast native Minecraft launcher, rewritten in Rust. \
                 Select an account, pick a version and press Play.",
            ),
            "01.01.2025",
        ),
        NewsItem::new(
            tr(lang, "How to get started"),
            tr(
                lang,
                "1. Add your nickname in Accounts\n2. Pick a version (install one \
                 from the Catalog if needed)\n3. Configure RAM in Settings\n4. Press Play!",
            ),
            "01.01.2025",
        ),
    ]
}

/// Fetch the news feed. Returns parsed items or an error the caller can show.
pub fn fetch(agent: &ureq::Agent, url: &str, lang: Language) -> Result<Vec<NewsItem>> {
    let body = net::get_string(agent, url, lang)?;
    let items: Vec<NewsItem> = serde_json::from_str(&body).map_err(|e| {
        anyhow::anyhow!(
            "{}",
            tr_fmt(lang, "bad news format: {0}", &[&e.to_string()])
        )
    })?;
    if items.is_empty() {
        return Err(anyhow::anyhow!("{}", tr(lang, "news feed is empty")));
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_feed() {
        let body = r#"[
            {
                "id": "abc",
                "title": "T1",
                "content": "C1",
                "slug": "t1",
                "createdAt": "2026-08-23T11:28:35.957Z",
                "syncedToDiscord": true,
                "author": {"name": "rizer001", "image": "https://example.com/avatar.png"}
            },
            {"title": "T2"}
        ]"#;
        let items: Vec<NewsItem> = serde_json::from_str(body).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].author_name(crate::lang::Language::English),
            "rizer001"
        );
        assert!(items[0].author_avatar().is_some());
        assert!(items[1].author.is_none());
        assert_eq!(
            items[1].author_name(crate::lang::Language::English),
            "Unknown"
        );
    }

    #[test]
    fn formatted_date_parses_iso() {
        let item = NewsItem {
            created_at: "2026-08-23T11:28:35.957Z".into(),
            ..NewsItem::new("t", "c", "")
        };
        assert_eq!(item.formatted_date(), "23.08.2026");
    }

    #[test]
    fn fallback_news_is_never_empty() {
        let lang = crate::lang::Language::English;
        assert!(!fallback_news(lang).is_empty());
        assert!(fallback_news(lang).iter().all(|n| !n.title.is_empty()));
    }
}
