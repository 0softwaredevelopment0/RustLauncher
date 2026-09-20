//! Font setup: the embedded Material Icons fallback plus a system CJK
//! fallback, so Chinese / Japanese / Korean text renders as real glyphs
//! instead of tofu squares (the default egui fonts have no CJK coverage).
//!
//! The CJK face is loaded from the OS at startup rather than embedded: a
//! full CJK font is 10-20 MiB, which would balloon the binary, while only
//! the glyphs actually drawn end up in the texture atlas.

use std::sync::OnceLock;

use crate::icons;

/// Upper bound on how many system CJK faces to load, to bound memory use.
/// Windows ships separate faces for Chinese/Japanese (YaHei, MS Gothic) and
/// Korean (Malgun), so three are needed to cover every CJK UI language.
const MAX_CJK_FONTS: usize = 3;

/// Cache of the (expensive) system CJK font reads. The definitions are still
/// applied per context, so several egui contexts each get the fallbacks.
static CJK_FONTS: OnceLock<Vec<(String, egui::FontData)>> = OnceLock::new();

/// Install the launcher fonts into the egui context. Call once at startup,
/// before the first frame is drawn.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // Material Icons: appended as a fallback so any glyph missing from
    // the default fonts (i.e. every icon codepoint) falls through. The
    // atlas only rasterizes glyphs actually drawn, so embedding the full
    // font costs ~350 KiB of binary size and nothing at runtime.
    fonts.font_data.insert(
        "material-icons".to_owned(),
        egui::FontData::from_static(icons::FONT_BYTES),
    );
    let mut fallbacks = vec!["material-icons".to_owned()];

    // System CJK fallback(s), in preference order.
    for (key, data) in CJK_FONTS.get_or_init(cjk_fonts) {
        fonts.font_data.insert(key.clone(), data.clone());
        fallbacks.push(key.clone());
    }

    // Register the fallbacks for both proportional text and the monospace
    // console (mods print CJK to stdout too).
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .extend(fallbacks.iter().cloned());
    }

    ctx.set_fonts(fonts);
}

/// Candidate system font files that cover CJK, in preference order.
///
/// The first entry is the broadest (it also covers kana), so a single face
/// already renders both Chinese and Japanese; the extras improve coverage
/// for Japanese-specific glyph shapes and Korean.
fn cjk_candidates() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &[
            r"C:\Windows\Fonts\msyh.ttc",     // Microsoft YaHei (SC + kana)
            r"C:\Windows\Fonts\msgothic.ttc", // MS Gothic (JP)
            r"C:\Windows\Fonts\malgun.ttf",   // Malgun Gothic (KR)
        ]
    } else if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/Library/Fonts/Arial Unicode.ttf",
        ]
    } else {
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
        ]
    }
}

/// Load the first [`MAX_CJK_FONTS`] existing CJK font files as egui font data.
fn cjk_fonts() -> Vec<(String, egui::FontData)> {
    let mut out = Vec::new();
    for (i, path) in cjk_candidates().iter().enumerate() {
        if out.len() >= MAX_CJK_FONTS {
            break;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if !is_font_bytes(&bytes) {
            continue;
        }
        out.push((format!("cjk-{i}"), egui::FontData::from_owned(bytes)));
    }
    out
}

/// Whether the bytes look like a supported font container: a TrueType/OpenType
/// face (`0x00010000` / `OTTO`) or a TrueType collection (`ttcf`).
fn is_font_bytes(bytes: &[u8]) -> bool {
    bytes.len() > 4
        && (bytes.starts_with(&[0x00, 0x01, 0x00, 0x00])
            || bytes.starts_with(b"OTTO")
            || bytes.starts_with(b"ttcf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_declared_for_every_platform() {
        assert!(!cjk_candidates().is_empty());
    }

    #[test]
    fn font_signature_detection() {
        assert!(is_font_bytes(&[0x00, 0x01, 0x00, 0x00, 0x00]));
        assert!(is_font_bytes(b"OTTO...."));
        assert!(is_font_bytes(b"ttcf...."));
        assert!(!is_font_bytes(b"not a font"));
        assert!(!is_font_bytes(&[]));
        assert!(!is_font_bytes(&[0x00, 0x01]));
    }

    #[test]
    fn existing_candidates_are_valid_fonts() {
        // Any candidate that exists on this machine must be a real font file
        // (guards against a wrong path silently loading garbage).
        for path in cjk_candidates() {
            if let Ok(bytes) = std::fs::read(path) {
                assert!(is_font_bytes(&bytes), "not a font: {path}");
            }
        }
    }

    #[test]
    fn installing_and_laying_out_cjk_does_not_panic() {
        // epaint panics when a registered font fails to parse, so actually
        // run a pass and lay out CJK text through the installed fallbacks.
        let ctx = egui::Context::default();
        install(&ctx);
        let id = egui::FontId::proportional(14.0);
        let found = std::cell::Cell::new(false);
        let found_ko = std::cell::Cell::new(false);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            ctx.fonts(|fonts| {
                let _ = fonts.layout_no_wrap(
                    "中文测试 日本語 한국어".to_owned(),
                    id.clone(),
                    egui::Color32::WHITE,
                );
                found.set(fonts.has_glyphs(&id, "中文"));
                found_ko.set(fonts.has_glyphs(&id, "한국어"));
            });
        });
        // When a CJK face was actually loaded, the glyphs must resolve
        // (a tofu-square regression would make this false).
        if !cjk_fonts().is_empty() {
            assert!(found.get(), "CJK fallback loaded but glyphs are missing");
        }
        // Windows ships Malgun Gothic, which the candidate list loads, so
        // Korean must resolve there too.
        if cfg!(target_os = "windows") {
            assert!(found_ko.get(), "Korean glyphs are missing on Windows");
        }
    }
}
