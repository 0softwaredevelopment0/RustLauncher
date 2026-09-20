//! Launcher settings persisted as `config.json` in the launcher home.
//!
//! The old Java launcher kept ~30 loosely-typed key/value pairs in SQLite;
//! this port keeps the settings that affect behavior in a typed struct.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::home;
use crate::jvm;
use crate::lang::Language;

/// How much of the game output the Console tab displays (display filter
/// only — it does not affect what is written to the log files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConsoleMode {
    /// Every line.
    #[default]
    All,
    /// Only warnings.
    Warnings,
    /// Only errors (errors, exceptions, crashes).
    Errors,
    /// Errors plus warnings.
    ErrorsAndWarnings,
    /// No game output at all (launcher messages still show).
    Nothing,
}

impl ConsoleMode {
    pub const ALL: [ConsoleMode; 5] = [
        ConsoleMode::All,
        ConsoleMode::Warnings,
        ConsoleMode::Errors,
        ConsoleMode::ErrorsAndWarnings,
        ConsoleMode::Nothing,
    ];

    pub fn label(self, lang: Language) -> &'static str {
        use crate::lang::tr;
        match self {
            ConsoleMode::All => tr(lang, "All"),
            ConsoleMode::Warnings => tr(lang, "Warnings"),
            ConsoleMode::Errors => tr(lang, "Errors"),
            ConsoleMode::ErrorsAndWarnings => tr(lang, "Errors + warnings"),
            ConsoleMode::Nothing => tr(lang, "Nothing"),
        }
    }

    /// Whether a line should be shown: the launcher's own `[RustLauncher]`
    /// lines always pass, game output is filtered by the mode.
    pub fn allows_launcher_aware(&self, line: &str) -> bool {
        if line.starts_with("[RustLauncher]") || line.starts_with("[ERROR ") {
            return true;
        }
        self.allows(line)
    }

    /// Whether a game-output line should be shown under this mode.
    pub fn allows(&self, line: &str) -> bool {
        match self {
            ConsoleMode::All => true,
            ConsoleMode::Nothing => false,
            ConsoleMode::Warnings => line_matches(line, WARNING_MARKERS),
            ConsoleMode::Errors => line_matches(line, ERROR_MARKERS),
            ConsoleMode::ErrorsAndWarnings => {
                line_matches(line, ERROR_MARKERS) || line_matches(line, WARNING_MARKERS)
            }
        }
    }
}

const ERROR_MARKERS: &[&str] = &[
    "error",
    "exception",
    "failed",
    "fatal",
    "crash",
    "severe",
    "stack trace",
];
const WARNING_MARKERS: &[&str] = &["warn"];

fn line_matches(line: &str, markers: &[&str]) -> bool {
    let lower = line.to_ascii_lowercase();
    markers.iter().any(|m| lower.contains(m))
}

/// What a log FILE captures (launcher log / game log). The Console tab has
/// its own display filter (`ConsoleMode`); these two are independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FileLogMode {
    /// Do not write the file at all.
    Nothing,
    /// Only warnings.
    Warnings,
    /// Only errors.
    Errors,
    /// Warnings and errors.
    WarningsAndErrors,
    /// Every line.
    #[default]
    All,
}

impl FileLogMode {
    pub const ALL: [FileLogMode; 5] = [
        FileLogMode::Nothing,
        FileLogMode::Warnings,
        FileLogMode::Errors,
        FileLogMode::WarningsAndErrors,
        FileLogMode::All,
    ];

    pub fn label(self, lang: Language) -> &'static str {
        use crate::lang::tr;
        match self {
            FileLogMode::Nothing => tr(lang, "Nothing"),
            FileLogMode::Warnings => tr(lang, "Warnings"),
            FileLogMode::Errors => tr(lang, "Errors"),
            FileLogMode::WarningsAndErrors => tr(lang, "Warnings + errors"),
            FileLogMode::All => tr(lang, "All"),
        }
    }

    /// Whether a log line belongs in the file under this mode. Launcher's own
    /// metadata lines (`[RustLauncher] …`) always pass — they are not game
    /// output, they are the file's context.
    pub fn allows(&self, line: &str) -> bool {
        if line.starts_with("[RustLauncher]") {
            return true;
        }
        match self {
            FileLogMode::Nothing => false,
            FileLogMode::All => true,
            FileLogMode::Errors => line_matches(line, ERROR_MARKERS),
            FileLogMode::Warnings => line_matches(line, WARNING_MARKERS),
            FileLogMode::WarningsAndErrors => {
                line_matches(line, ERROR_MARKERS) || line_matches(line, WARNING_MARKERS)
            }
        }
    }
}

/// A built-in look, or `Custom` once the user tweaks any color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemePreset {
    White,
    Light,
    Gray,
    #[default]
    Dark,
    Black,
    Custom,
}

impl ThemePreset {
    pub const ALL: [ThemePreset; 6] = [
        ThemePreset::White,
        ThemePreset::Light,
        ThemePreset::Gray,
        ThemePreset::Dark,
        ThemePreset::Black,
        ThemePreset::Custom,
    ];

    pub fn label(self, lang: Language) -> &'static str {
        use crate::lang::tr;
        match self {
            ThemePreset::White => tr(lang, "White"),
            ThemePreset::Light => tr(lang, "Light"),
            ThemePreset::Gray => tr(lang, "Gray"),
            ThemePreset::Dark => tr(lang, "Dark"),
            ThemePreset::Black => tr(lang, "Black"),
            ThemePreset::Custom => tr(lang, "Custom"),
        }
    }
}

/// How the window background is painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BackgroundMode {
    /// A single flat color.
    #[default]
    Color,
    /// A vertical gradient (`bg_top` → `bg_bottom`).
    Gradient,
    /// A photo from `bg_image` (PNG/JPG), stretched to cover the window.
    Image,
}

impl BackgroundMode {
    pub const ALL: [BackgroundMode; 3] = [
        BackgroundMode::Color,
        BackgroundMode::Gradient,
        BackgroundMode::Image,
    ];

    pub fn label(self, lang: Language) -> &'static str {
        use crate::lang::tr;
        match self {
            BackgroundMode::Color => tr(lang, "Color"),
            BackgroundMode::Gradient => tr(lang, "Gradient"),
            BackgroundMode::Image => tr(lang, "Image"),
        }
    }
}

/// Full UI theme: background (flat color, gradient or photo), button and
/// accent colors. Colors are stored as sRGB triplets (the color picker
/// edits them with the full palette); `preset` only remembers which
/// built-in look they came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub preset: ThemePreset,
    /// Dark widget base (dark text on light off). Presets set it; in
    /// Custom mode it is a manual checkbox.
    pub dark_base: bool,
    pub background: BackgroundMode,
    /// Flat background color (`BackgroundMode::Color`).
    pub bg_color: [u8; 3],
    /// Gradient stops (`BackgroundMode::Gradient`).
    pub bg_top: [u8; 3],
    pub bg_bottom: [u8; 3],
    /// Photo path (`BackgroundMode::Image`).
    pub bg_image: String,
    /// Normal button/control fill.
    pub button: [u8; 3],
    /// Selection, links and pressed-button fill.
    pub accent: [u8; 3],
    /// Override every text color (launcher + console) when `custom_text`.
    pub text_color: [u8; 3],
    /// When true, all text uses [`Theme::text_color`]; when false the theme's
    /// default text color applies (white-ish on the dark base).
    pub custom_text: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_preset(ThemePreset::Dark)
    }
}

impl Theme {
    pub fn dark() -> Self {
        Self::from_preset(ThemePreset::Dark)
    }

    pub fn light() -> Self {
        Self::from_preset(ThemePreset::Light)
    }

    /// The full theme for a built-in preset.
    pub fn from_preset(preset: ThemePreset) -> Self {
        let (dark_base, bg, button, accent) = match preset {
            ThemePreset::White => (false, [255, 255, 255], [225, 228, 232], [25, 118, 210]),
            ThemePreset::Light => (false, [242, 242, 242], [220, 223, 227], [25, 118, 210]),
            ThemePreset::Gray => (true, [110, 110, 110], [140, 140, 140], [255, 176, 66]),
            ThemePreset::Dark => (true, [30, 30, 30], [60, 60, 60], [100, 181, 246]),
            ThemePreset::Black => (true, [0, 0, 0], [32, 32, 32], [0, 200, 255]),
            ThemePreset::Custom => {
                return Self {
                    preset: ThemePreset::Custom,
                    ..Self::from_preset(ThemePreset::Dark)
                };
            }
        };
        Self {
            preset,
            dark_base,
            background: BackgroundMode::Color,
            bg_color: bg,
            bg_top: bg,
            bg_bottom: bg,
            bg_image: String::new(),
            button,
            accent,
            text_color: [255, 255, 255],
            custom_text: false,
        }
    }

    pub fn bg_color32(&self) -> egui::Color32 {
        let [r, g, b] = self.bg_color;
        egui::Color32::from_rgb(r, g, b)
    }

    pub fn bg_top32(&self) -> egui::Color32 {
        let [r, g, b] = self.bg_top;
        egui::Color32::from_rgb(r, g, b)
    }

    pub fn bg_bottom32(&self) -> egui::Color32 {
        let [r, g, b] = self.bg_bottom;
        egui::Color32::from_rgb(r, g, b)
    }

    pub fn button32(&self) -> egui::Color32 {
        let [r, g, b] = self.button;
        egui::Color32::from_rgb(r, g, b)
    }

    pub fn accent32(&self) -> egui::Color32 {
        let [r, g, b] = self.accent;
        egui::Color32::from_rgb(r, g, b)
    }

    pub fn text_color32(&self) -> egui::Color32 {
        let [r, g, b] = self.text_color;
        egui::Color32::from_rgb(r, g, b)
    }
}

/// Which Java runtime the launcher uses for games.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JavaSelection {
    /// Auto-detect the best runtime for each game (bundled runtime, then the
    /// system install matching the required version).
    #[default]
    Auto,
    /// A specific installed runtime, picked from the detected list.
    Installed(String),
    /// An arbitrary java executable chosen by the user.
    Custom(String),
}

impl JavaSelection {
    /// The configured executable path, if a concrete runtime is selected.
    /// Blank paths are treated as unset (`None`).
    pub fn path(&self) -> Option<&str> {
        match self {
            JavaSelection::Auto => None,
            JavaSelection::Installed(p) | JavaSelection::Custom(p) => {
                let p = p.trim();
                if p.is_empty() {
                    None
                } else {
                    Some(p)
                }
            }
        }
    }

    /// True when a concrete runtime is selected but its path is blank.
    pub fn is_empty_path(&self) -> bool {
        match self {
            JavaSelection::Auto => false,
            JavaSelection::Installed(p) | JavaSelection::Custom(p) => p.trim().is_empty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Currently selected account name.
    pub username: String,
    pub game_width: u32,
    pub game_height: u32,
    pub use_custom_resolution: bool,
    /// JVM arguments. Heap size lives here (`-Xms`/`-Xmx`); the game refuses
    /// to launch without them. Defaults to [`jvm::DEFAULT_JVM_ARGS`].
    pub java_args: String,
    /// Which JVM flag preset is active: a [`jvm::PRESETS`] id, or
    /// [`jvm::CUSTOM_PRESET_ID`] when the flags were edited by hand or the
    /// user pinned the manual mode in the picker.
    pub java_args_preset: String,
    /// Which Java runtime to launch games with.
    pub java_mode: JavaSelection,
    /// Game directory. The launcher refuses to launch without it (empty means
    /// "not configured yet").
    pub game_directory: String,
    pub selected_version: String,
    /// Server to auto-connect to ("host" or "host:port").
    pub connect_server_ip: String,
    pub auto_connect: bool,
    /// Write the game console output to a numbered log file.
    /// What BOTH `logs/` files capture (`launcher-N.log` and `game-N.log`).
    /// Replaces the old per-file `launcher_file_log` / `game_file_log`
    /// pair (old config keys are ignored on load).
    pub file_log: FileLogMode,
    /// How much of the game output the Console tab shows.
    pub console_log_mode: ConsoleMode,
    /// Full UI theme (background, buttons, accent). Replaces the old
    /// `dark_theme` checkbox.
    pub theme: Theme,
    /// Legacy flag from pre-theme builds. Migrated to [`Theme`] on load,
    /// never written back.
    #[serde(default, skip_serializing)]
    pub dark_theme: Option<bool>,
    /// Ask for confirmation before force-killing the game (Kill button).
    pub confirm_kill: bool,
    /// Ask for confirmation before politely stopping the game (Stop button).
    pub confirm_stop: bool,
    /// Base URL of the website (news API is `<base>/api/news`).
    pub news_url: String,
    /// GUI language (English by default).
    pub language: Language,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            username: String::new(),
            game_width: 854,
            game_height: 480,
            use_custom_resolution: false,
            java_args: jvm::DEFAULT_JVM_ARGS.to_string(),
            java_args_preset: jvm::preset_id_for_args(jvm::DEFAULT_JVM_ARGS).to_string(),
            java_mode: JavaSelection::Auto,
            game_directory: String::new(),
            selected_version: String::new(),
            connect_server_ip: String::new(),
            auto_connect: false,
            file_log: FileLogMode::All,
            console_log_mode: ConsoleMode::All,
            theme: Theme::default(),
            dark_theme: None,
            confirm_kill: true,
            confirm_stop: true,
            news_url: "https://rizer001.opik.net".to_string(),
            language: Language::default(),
        }
    }
}

impl Settings {
    /// Load settings; missing or corrupt file falls back to defaults
    /// (the old launcher silently carried on too, but at least we say why).
    pub fn load(home_dir: &Path) -> Settings {
        const OLD_BROKEN_NEWS_URL: &str = "https://rizer001.opik.net/news";
        let path = home::config_file(home_dir);
        match std::fs::read(&path) {
            Ok(bytes) => {
                // Configs from before the Java picker carried a plain
                // `use_custom_java` flag + `java_path`; map them onto the
                // enum (a config that already has `java_mode` keeps it).
                let legacy_java = legacy_java_selection(&bytes);
                match serde_json::from_slice::<Settings>(&bytes) {
                    Ok(mut settings) => {
                        if settings.java_mode == JavaSelection::Auto {
                            if let Some(mode) = legacy_java {
                                settings.java_mode = mode;
                            }
                        }
                        // Older builds shipped a wrong default (`…/news`), which
                        // made the news feed request `…/news/api/news` → HTTP 404.
                        // Migrate silently to the site root.
                        if settings.news_url.trim_end_matches('/') == OLD_BROKEN_NEWS_URL {
                            settings.news_url = Settings::default().news_url;
                        }
                        // Pre-theme configs only have the `dark_theme` checkbox:
                        // map it onto the matching preset (a config that already
                        // carries a theme keeps it).
                        if settings.theme == Theme::default() {
                            if let Some(dark) = settings.dark_theme {
                                settings.theme = if dark { Theme::dark() } else { Theme::light() };
                            }
                        }
                        settings.dark_theme = None;
                        // Flags edited outside the picker no longer match the
                        // stored preset: re-derive it from the args (a pinned
                        // `custom` stays untouched).
                        if settings.java_args_preset != jvm::CUSTOM_PRESET_ID {
                            settings.java_args_preset =
                                jvm::preset_id_for_args(&settings.java_args).to_string();
                        }
                        settings
                    }
                    Err(e) => {
                        eprintln!(
                            "[RustLauncher] corrupt {} ({e}); using defaults",
                            path.display()
                        );
                        Settings::default()
                    }
                }
            }
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, home_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(home_dir)?;
        let path = home::config_file(home_dir);
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Extract the pre-enum Java selection from a legacy config: `None` when the
/// config already carries `java_mode`.
fn legacy_java_selection(bytes: &[u8]) -> Option<JavaSelection> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if value.get("java_mode").is_some() {
        return None;
    }
    if value
        .get("use_custom_java")
        .and_then(serde_json::Value::as_bool)?
    {
        Some(JavaSelection::Custom(
            value
                .get("java_path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ))
    } else {
        Some(JavaSelection::Auto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("rl-set-{}-{tag}", std::process::id()))
    }

    #[test]
    fn java_args_preset_roundtrips_and_self_heals() {
        let dir = tmp_home("jvm-preset");
        let _ = std::fs::remove_dir_all(&dir);

        // Default: the default args equal the minimal preset.
        let mut s = Settings::default();
        assert_eq!(s.java_args_preset, "minimal");

        // Picking the G1GC preset survives a save/load.
        s.java_args = jvm::find_preset("g1gc").unwrap().args.to_string();
        s.java_args_preset = "g1gc".into();
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir).java_args_preset, "g1gc");

        // Args edited outside the picker → derived back to custom.
        s.java_args = "-Xms2g -Xmx6g -Dextra".into();
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir).java_args_preset, jvm::CUSTOM_PRESET_ID);

        // A pinned custom is never overwritten on load.
        s.java_args_preset = jvm::CUSTOM_PRESET_ID.into();
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir).java_args_preset, jvm::CUSTOM_PRESET_ID);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tmp_home("missing");
        let _ = std::fs::remove_dir_all(&dir);
        let s = Settings::load(&dir);
        assert_eq!(s.java_args, jvm::DEFAULT_JVM_ARGS);
        assert_eq!(s.java_mode, JavaSelection::Auto);
        assert_eq!(s.game_width, 854);
        assert_eq!(s.theme.preset, ThemePreset::Dark);
        assert!(s.theme.dark_base);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tmp_home("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let s = Settings {
            java_args: "-Xms2g -Xmx8g -XX:+UseZGC".into(),
            java_args_preset: jvm::CUSTOM_PRESET_ID.into(),
            username: "Rizer001".into(),
            java_mode: JavaSelection::Custom("C:/java/bin/java.exe".into()),
            ..Settings::default()
        };
        s.save(&dir).unwrap();
        let loaded = Settings::load(&dir);
        assert_eq!(loaded, s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = tmp_home("corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), b"{ not json").unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s, Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_and_legacy_fields_do_not_break_loading() {
        let dir = tmp_home("extra");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // `ram` and `all_logs` were removed from the struct; old configs must
        // still load.
        std::fs::write(
            dir.join("config.json"),
            br#"{"ram": 2048, "all_logs": true, "someRemovedSetting": true, "launcher_file_log": "Errors", "game_file_log": "Warnings"}"#,
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.java_args, jvm::DEFAULT_JVM_ARGS);
        assert_eq!(s.console_log_mode, ConsoleMode::All);
        // The per-file modes were merged into a single `file_log`.
        assert_eq!(s.file_log, FileLogMode::All);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_java_config_migrates_to_selection_enum() {
        let dir = tmp_home("legacy-java");
        std::fs::create_dir_all(&dir).unwrap();
        // Pre-enum config: custom Java on.
        let legacy = serde_json::json!({
            "java_args": "-Xms1m -Xmx4g",
            "use_custom_java": true,
            "java_path": "C:/java/bin/java.exe"
        });
        std::fs::write(
            home::config_file(&dir),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(
            s.java_mode,
            JavaSelection::Custom("C:/java/bin/java.exe".into())
        );

        // Pre-enum config: auto mode.
        let legacy = serde_json::json!({ "use_custom_java": false });
        std::fs::write(
            home::config_file(&dir),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.java_mode, JavaSelection::Auto);

        // Modern config keeps its java_mode untouched.
        let modern = serde_json::json!({ "java_mode": { "installed": "Z:/j/bin/java.exe" } });
        std::fs::write(
            home::config_file(&dir),
            serde_json::to_vec(&modern).unwrap(),
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(
            s.java_mode,
            JavaSelection::Installed("Z:/j/bin/java.exe".into())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn console_mode_filters_lines() {
        let err = "java.lang.RuntimeException: boom";
        let warn = "[12:00] WARN: low disk space";
        let plain = "Rendering world chunk 42";
        assert!(ConsoleMode::All.allows(err));
        assert!(ConsoleMode::All.allows(plain));
        assert!(ConsoleMode::Warnings.allows(warn));
        assert!(!ConsoleMode::Warnings.allows(err));
        assert!(!ConsoleMode::Warnings.allows(plain));
        assert!(ConsoleMode::Errors.allows(err));
        assert!(!ConsoleMode::Errors.allows(warn));
        assert!(!ConsoleMode::Errors.allows(plain));
        assert!(ConsoleMode::ErrorsAndWarnings.allows(err));
        assert!(ConsoleMode::ErrorsAndWarnings.allows(warn));
        assert!(!ConsoleMode::ErrorsAndWarnings.allows(plain));
        assert!(!ConsoleMode::Nothing.allows(err));
    }

    #[test]
    fn file_log_mode_filters_lines() {
        let err = "java.lang.RuntimeException: boom";
        let warn = "[12:00] WARN: low disk space";
        let plain = "Rendering world chunk 42";
        let meta = "[RustLauncher] Launching 1.21";
        assert!(FileLogMode::All.allows(err));
        assert!(FileLogMode::All.allows(plain));
        assert!(!FileLogMode::Nothing.allows(err));
        assert!(FileLogMode::Nothing.allows(meta));
        assert!(FileLogMode::Errors.allows(err));
        assert!(!FileLogMode::Errors.allows(warn));
        assert!(!FileLogMode::Errors.allows(plain));
        assert!(FileLogMode::Warnings.allows(warn));
        assert!(!FileLogMode::Warnings.allows(err));
        assert!(FileLogMode::WarningsAndErrors.allows(err));
        assert!(FileLogMode::WarningsAndErrors.allows(warn));
        assert!(!FileLogMode::WarningsAndErrors.allows(plain));
    }

    #[test]
    fn theme_presets_have_sane_bases() {
        assert!(!Theme::from_preset(ThemePreset::White).dark_base);
        assert!(!Theme::from_preset(ThemePreset::Light).dark_base);
        for preset in [ThemePreset::Gray, ThemePreset::Dark, ThemePreset::Black] {
            assert!(Theme::from_preset(preset).dark_base, "{preset:?}");
        }
        for preset in ThemePreset::ALL {
            let theme = Theme::from_preset(preset);
            assert_eq!(theme.preset, preset);
            assert_eq!(theme.background, BackgroundMode::Color);
        }
        assert_eq!(Theme::default().preset, ThemePreset::Dark);
    }

    #[test]
    fn legacy_dark_theme_flag_migrates_to_preset() {
        for (flag, preset) in [(true, ThemePreset::Dark), (false, ThemePreset::Light)] {
            let dir = tmp_home(&format!("darkflag-{flag}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("config.json"),
                format!(r#"{{"dark_theme": {flag}}}"#),
            )
            .unwrap();
            let s = Settings::load(&dir);
            assert_eq!(s.theme.preset, preset, "dark_theme={flag}");
            assert_eq!(s.theme, Theme::from_preset(preset));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn theme_roundtrips_through_save_load() {
        let dir = tmp_home("theme");
        let _ = std::fs::remove_dir_all(&dir);
        let s = Settings {
            theme: Theme {
                preset: ThemePreset::Custom,
                dark_base: true,
                background: BackgroundMode::Gradient,
                bg_color: [10, 20, 30],
                bg_top: [0, 0, 0],
                bg_bottom: [255, 255, 255],
                bg_image: "C:/pics/bg.jpg".into(),
                button: [1, 2, 3],
                accent: [4, 5, 6],
                text_color: [7, 8, 9],
                custom_text: true,
            },
            ..Settings::default()
        };
        s.save(&dir).unwrap();
        let loaded = Settings::load(&dir);
        assert_eq!(loaded.theme, s.theme);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
