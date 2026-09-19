//! UI translations. Every user-visible GUI string goes through [`tr`]
//! with its English text as the key; missing translations fall back to
//! English, so a partially-translated language never breaks the UI.
//!
//! Templates with dynamic parts use `{0}`, `{1}`, … placeholders filled by
//! [`tr_fmt`], so each language can order the parts freely:
//! `tr_fmt(lang, "Stop {0}?", &[&name])`.

use serde::{Deserialize, Serialize};

pub mod de;
pub mod es;
pub mod fr;
pub mod it;
pub mod ja;
pub mod ko;
pub mod pl;
pub mod pt;
pub mod ru;
pub mod tr_;
pub mod uk;
pub mod zh;

/// GUI language. English is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Language {
    #[default]
    English,
    Russian,
    German,
    French,
    Spanish,
    Italian,
    Portuguese,
    Polish,
    Ukrainian,
    Turkish,
    Chinese,
    Japanese,
    Korean,
}

impl Language {
    pub const ALL: [Language; 13] = [
        Language::English,
        Language::Russian,
        Language::German,
        Language::French,
        Language::Spanish,
        Language::Italian,
        Language::Portuguese,
        Language::Polish,
        Language::Ukrainian,
        Language::Turkish,
        Language::Chinese,
        Language::Japanese,
        Language::Korean,
    ];

    /// Native name shown in the language picker.
    pub fn label(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Russian => "Русский",
            Language::German => "Deutsch",
            Language::French => "Français",
            Language::Spanish => "Español",
            Language::Italian => "Italiano",
            Language::Portuguese => "Português",
            Language::Polish => "Polski",
            Language::Ukrainian => "Українська",
            Language::Turkish => "Türkçe",
            Language::Chinese => "中文",
            Language::Japanese => "日本語",
            Language::Korean => "한국어",
        }
    }
}

/// Translate an English UI string. Unknown keys (or languages without a
/// translation yet) return the English text unchanged.
pub fn tr(lang: Language, key: &str) -> &str {
    match lang {
        Language::English => key,
        Language::Russian => ru::get(key).unwrap_or(key),
        Language::German => de::get(key).unwrap_or(key),
        Language::French => fr::get(key).unwrap_or(key),
        Language::Spanish => es::get(key).unwrap_or(key),
        Language::Italian => it::get(key).unwrap_or(key),
        Language::Portuguese => pt::get(key).unwrap_or(key),
        Language::Polish => pl::get(key).unwrap_or(key),
        Language::Ukrainian => uk::get(key).unwrap_or(key),
        Language::Turkish => tr_::get(key).unwrap_or(key),
        Language::Chinese => zh::get(key).unwrap_or(key),
        Language::Japanese => ja::get(key).unwrap_or(key),
        Language::Korean => ko::get(key).unwrap_or(key),
    }
}

/// Translate a template and substitute `{0}`, `{1}`, … with `args`.
pub fn tr_fmt(lang: Language, key: &str, args: &[&str]) -> String {
    let mut out = tr(lang, key).to_string();
    for (i, arg) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), arg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_identity_and_fallback() {
        assert_eq!(tr(Language::English, "Save settings"), "Save settings");
        // Unknown keys fall back to English in every language.
        for lang in Language::ALL {
            assert_eq!(tr(lang, "some key that does not exist"), "some key that does not exist");
        }
    }

    #[test]
    fn templates_substitute_in_order() {
        let s = tr_fmt(Language::English, "Stop {0}?", &["Survival"]);
        assert_eq!(s, "Stop Survival?");
        let s = tr_fmt(Language::English, "Install {0} {1} on {2}", &["Fabric", "0.16", "1.21"]);
        assert_eq!(s, "Install Fabric 0.16 on 1.21");
    }

    #[test]
    fn every_language_translates_common_chrome() {
        // Keys whose translations are language-specific in every supported
        // language. A missing translation falls back to English and would
        // make the comparison below fail, so this catches translation gaps.
        let keys = [
            "Save settings",
            "Cancel",
            "Confirm",
            "Launch",
            "Download",
            "Delete",
            "Refresh",
            "Settings",
            "Accounts",
            "Language",
        ];
        for lang in Language::ALL {
            if lang == Language::English {
                continue;
            }
            for key in keys {
                let t = tr(lang, key);
                assert_ne!(t, key, "{lang:?} did not translate {key:?}");
                assert!(!t.is_empty(), "{lang:?} has an empty translation for {key:?}");
            }
        }
    }
}
