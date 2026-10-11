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

    /// The name of this language written in `ui` — the currently selected
    /// UI language (`Language::English.name_in(Language::Russian)` is
    /// "Английский"). Rows are UI languages, columns follow [`Language::ALL`].
    pub fn name_in(self, ui: Language) -> &'static str {
        const NAMES: [[&str; 13]; 13] = [
            // English UI
            [
                "English", "Russian", "German", "French", "Spanish", "Italian", "Portuguese",
                "Polish", "Ukrainian", "Turkish", "Chinese", "Japanese", "Korean",
            ],
            // Russian UI
            [
                "Английский", "Русский", "Немецкий", "Французский", "Испанский", "Итальянский",
                "Португальский", "Польский", "Украинский", "Турецкий", "Китайский", "Японский",
                "Корейский",
            ],
            // German UI
            [
                "Englisch", "Russisch", "Deutsch", "Französisch", "Spanisch", "Italienisch",
                "Portugiesisch", "Polnisch", "Ukrainisch", "Türkisch", "Chinesisch", "Japanisch",
                "Koreanisch",
            ],
            // French UI
            [
                "Anglais", "Russe", "Allemand", "Français", "Espagnol", "Italien", "Portugais",
                "Polonais", "Ukrainien", "Turc", "Chinois", "Japonais", "Coréen",
            ],
            // Spanish UI
            [
                "Inglés", "Ruso", "Alemán", "Francés", "Español", "Italiano", "Portugués",
                "Polaco", "Ucraniano", "Turco", "Chino", "Japonés", "Coreano",
            ],
            // Italian UI
            [
                "Inglese", "Russo", "Tedesco", "Francese", "Spagnolo", "Italiano", "Portoghese",
                "Polacco", "Ucraino", "Turco", "Cinese", "Giapponese", "Coreano",
            ],
            // Portuguese UI
            [
                "Inglês", "Russo", "Alemão", "Francês", "Espanhol", "Italiano", "Português",
                "Polonês", "Ucraniano", "Turco", "Chinês", "Japonês", "Coreano",
            ],
            // Polish UI
            [
                "Angielski", "Rosyjski", "Niemiecki", "Francuski", "Hiszpański", "Włoski",
                "Portugalski", "Polski", "Ukraiński", "Turecki", "Chiński", "Japoński",
                "Koreański",
            ],
            // Ukrainian UI
            [
                "Англійська", "Російська", "Німецька", "Французька", "Іспанська", "Італійська",
                "Португальська", "Польська", "Українська", "Турецька", "Китайська", "Японська",
                "Корейська",
            ],
            // Turkish UI
            [
                "İngilizce", "Rusça", "Almanca", "Fransızca", "İspanyolca", "İtalyanca",
                "Portekizce", "Lehçe", "Ukraynaca", "Türkçe", "Çince", "Japonca", "Korece",
            ],
            // Chinese UI
            [
                "英语", "俄语", "德语", "法语", "西班牙语", "意大利语", "葡萄牙语", "波兰语",
                "乌克兰语", "土耳其语", "中文", "日语", "韩语",
            ],
            // Japanese UI
            [
                "英語", "ロシア語", "ドイツ語", "フランス語", "スペイン語", "イタリア語",
                "ポルトガル語", "ポーランド語", "ウクライナ語", "トルコ語", "中国語", "日本語",
                "韓国語",
            ],
            // Korean UI
            [
                "영어", "러시아어", "독일어", "프랑스어", "스페인어", "이탈리아어", "포르투갈어",
                "폴란드어", "우크라이나어", "터키어", "중국어", "일본어", "한국어",
            ],
        ];
        NAMES[ui as usize][self as usize]
    }

    /// The Settings picker label: the language's own name plus, in
    /// parentheses, its name in the selected UI language — `English` in a
    /// Russian UI renders as `English (Английский)`.
    pub fn picker_label(self, ui: Language) -> String {
        format!("{} ({})", self.label(), self.name_in(ui))
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
            assert_eq!(
                tr(lang, "some key that does not exist"),
                "some key that does not exist"
            );
        }
    }

    #[test]
    fn templates_substitute_in_order() {
        let s = tr_fmt(Language::English, "Stop {0}?", &["Survival"]);
        assert_eq!(s, "Stop Survival?");
        let s = tr_fmt(
            Language::English,
            "Install {0} {1} on {2}",
            &["Fabric", "0.16", "1.21"],
        );
        assert_eq!(s, "Install Fabric 0.16 on 1.21");
    }

    #[test]
    fn name_in_diagonal_matches_native_labels() {
        // The name of a language in its own UI must be its native label,
        // and every row must cover all languages (compile-time by shape,
        // runtime check for the diagonal).
        for lang in Language::ALL {
            assert_eq!(lang.name_in(lang), lang.label());
        }
    }

    #[test]
    fn picker_label_shows_selected_language_name_in_parens() {
        assert_eq!(
            Language::English.picker_label(Language::English),
            "English (English)"
        );
        assert_eq!(
            Language::English.picker_label(Language::Russian),
            "English (Английский)"
        );
        assert_eq!(
            Language::Russian.picker_label(Language::English),
            "Русский (Russian)"
        );
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
                assert!(
                    !t.is_empty(),
                    "{lang:?} has an empty translation for {key:?}"
                );
            }
        }
    }
}
