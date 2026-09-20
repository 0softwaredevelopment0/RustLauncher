//! Material Icons (Google's official icon font, Apache 2.0) embedded into the
//! binary and registered as an egui fallback font (see [`crate::fonts`]), so
//! UI text can use real vector icons instead of unicode placeholders that
//! render as empty boxes.
//!
//! Usage in UI code: `ui.button(format!("{} Back", icons::ARROW_BACK))` — the
//! glyph is looked up in the appended `material-icons` font when the default
//! fonts (which lack these codepoints) fall through.

/// The embedded Material Icons font bytes.
pub(crate) static FONT_BYTES: &[u8] = include_bytes!("../assets/MaterialIcons-Regular.ttf");

// Icon codepoints from the Material Icons `codepoints` file. Each constant is
// a string holding the single glyph, ready for `format!()`-ing into labels
// and buttons.
pub const ARROW_BACK: &str = "\u{e5c4}"; // arrow_back
pub const PLAY_ARROW: &str = "\u{e037}"; // play_arrow
pub const TERMINAL: &str = "\u{eb8e}"; // terminal
pub const LAYERS: &str = "\u{e53b}"; // layers
pub const DNS: &str = "\u{e875}"; // dns
pub const CONTENT_COPY: &str = "\u{e14c}"; // content_copy
pub const ACCOUNT_CIRCLE: &str = "\u{e853}"; // account_circle
pub const FACE: &str = "\u{e87c}"; // face
pub const EXTENSION: &str = "\u{e87b}"; // extension
pub const ARTICLE: &str = "\u{ef42}"; // article
pub const SETTINGS: &str = "\u{e8b8}"; // settings
pub const BUILD: &str = "\u{e869}"; // build
pub const VIDEOGAME_ASSET: &str = "\u{e338}"; // videogame_asset
pub const ADD: &str = "\u{e145}"; // add
pub const DELETE: &str = "\u{e872}"; // delete
pub const STOP: &str = "\u{e047}"; // stop
pub const KILL: &str = "\u{e14c}"; // cancel
pub const FOLDER: &str = "\u{e2c8}"; // folder
pub const CHECK_CIRCLE: &str = "\u{e86c}"; // check_circle
pub const CHEVRON_RIGHT: &str = "\u{e5cc}"; // chevron_right
pub const ARROW_UPWARD: &str = "\u{e5d8}"; // arrow_upward
pub const ARROW_DOWNWARD: &str = "\u{e5db}"; // arrow_downward
pub const ARROW_FORWARD: &str = "\u{e5c8}"; // arrow_forward
pub const FAVORITE: &str = "\u{e87d}"; // favorite (heart)
pub const CHECK: &str = "\u{e5ca}"; // check
pub const RESTORE: &str = "\u{e8b3}"; // restore (settings_backup_restore)
pub const FIBER_MANUAL_RECORD: &str = "\u{e061}"; // fiber_manual_record (status dot)
pub const FILE_DOWNLOAD: &str = "\u{e2c4}"; // file_download
pub const REFRESH: &str = "\u{e5d5}"; // refresh
pub const EXPAND_MORE: &str = "\u{e313}"; // expand_more
pub const EXPAND_LESS: &str = "\u{e316}"; // expand_less

/// All icon constants, used by tests.
#[cfg(test)]
pub const ALL_ICONS: &[&str] = &[
    ARROW_BACK,
    PLAY_ARROW,
    TERMINAL,
    LAYERS,
    DNS,
    ACCOUNT_CIRCLE,
    FACE,
    EXTENSION,
    ARTICLE,
    SETTINGS,
    BUILD,
    VIDEOGAME_ASSET,
    ADD,
    DELETE,
    STOP,
    KILL,
    FOLDER,
    CHECK_CIRCLE,
    CONTENT_COPY,
    CHEVRON_RIGHT,
    ARROW_UPWARD,
    ARROW_DOWNWARD,
    ARROW_FORWARD,
    FAVORITE,
    CHECK,
    RESTORE,
    FIBER_MANUAL_RECORD,
    FILE_DOWNLOAD,
    REFRESH,
    EXPAND_MORE,
    EXPAND_LESS,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_constants_are_single_codepoints() {
        for icon in ALL_ICONS {
            assert_eq!(icon.chars().count(), 1, "icon constant {icon:?}");
        }
    }

    #[test]
    fn font_bytes_are_embedded_ttf() {
        // TrueType fonts start with the sfnt version 0x00010000.
        assert_eq!(&FONT_BYTES[..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn every_icon_has_a_glyph_in_the_font() {
        // Guards against a typo'd codepoint rendering as a tofu box: install
        // the fonts and check the embedded icon font actually has each glyph.
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let id = egui::FontId::proportional(16.0);
        let missing = std::cell::RefCell::new(Vec::new());
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            ctx.fonts(|fonts| {
                for icon in ALL_ICONS {
                    if !fonts.has_glyph(&id, icon.chars().next().unwrap()) {
                        missing.borrow_mut().push(format!("{icon:?}"));
                    }
                }
            });
        });
        assert!(
            missing.borrow().is_empty(),
            "icons without a glyph: {}",
            missing.borrow().join(", ")
        );
    }
}
