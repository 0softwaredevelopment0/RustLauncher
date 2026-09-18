//! Material Icons (Google's official icon font, Apache 2.0) embedded into the
//! binary and registered as an egui fallback font, so UI text can use real
//! vector icons instead of unicode placeholders that render as empty boxes.
//!
//! Usage in UI code: `ui.button(format!("{} Back", icons::ARROW_BACK))` — the
//! glyph is looked up in the appended `material-icons` font when the default
//! fonts (which lack these codepoints) fall through.

use std::sync::OnceLock;

use egui::FontData;

/// The embedded Material Icons font bytes.
static FONT_BYTES: &[u8] = include_bytes!("../assets/MaterialIcons-Regular.ttf");

// Icon codepoints from the Material Icons `codepoints` file. Each constant is
// a string holding the single glyph, ready for `format!()`-ing into labels
// and buttons.
pub const ARROW_BACK: &str = "\u{e5c4}"; // arrow_back
pub const PLAY_ARROW: &str = "\u{e037}"; // play_arrow
pub const TERMINAL: &str = "\u{eb8e}"; // terminal
pub const LAYERS: &str = "\u{e53b}"; // layers
pub const DNS: &str = "\u{e875}"; // dns
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
];

/// Install the icon font into the egui context (idempotent). Call once at
/// startup, before the first frame is drawn.
pub fn install(ctx: &egui::Context) {
    INSTALL.get_or_init(|| {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "material-icons".to_owned(),
            FontData::from_static(FONT_BYTES),
        );
        // Append to Proportional so any glyph missing from the default fonts
        // (i.e. every icon codepoint) falls through to the icon font. The
        // atlas only rasterizes glyphs actually drawn, so embedding the full
        // font costs ~350 KiB of binary size and nothing at runtime.
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("material-icons".to_owned());
        ctx.set_fonts(fonts);
    });
}

static INSTALL: OnceLock<()> = OnceLock::new();

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
}
