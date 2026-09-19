//! Skin management: download skins from Mojang/Crafatar, import local PNG
//! files, and render 64x64 skin data as RGBA for the GUI (port of the Java
//! `SkinManager`, with the base64 detour removed — we pass pixels directly).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use image::ImageFormat;

use crate::lang::{tr, tr_fmt, Language};
use crate::net;

/// Download the skin PNG for `username` into `skins_dir` and return its path.
pub fn download_skin(
    agent: &ureq::Agent,
    skins_dir: &Path,
    username: &str,
    lang: Language,
) -> Result<PathBuf> {
    // 1. UUID from the Mojang profile API (404 = unknown player).
    let profile_url = format!("https://api.mojang.com/users/profiles/minecraft/{username}");
    let profile_body = net::get_string(agent, &profile_url, lang)
        .map_err(|e| anyhow!("{}", tr_fmt(lang, "player '{0}' not found ({1})", &[username, &e.to_string()])))?;
    let profile: serde_json::Value = serde_json::from_str(&profile_body)
        .with_context(|| tr_fmt(lang, "bad profile response for '{0}'", &[username]))?;
    let uuid = profile
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            anyhow!("{}", tr_fmt(lang, "profile response for '{0}' has no 'id' field", &[username]))
        })?;

    // 2. Skin from Crafatar.
    let skin_url = format!("https://crafatar.com/skins/{uuid}");
    let bytes = net::get_bytes(agent, &skin_url, lang)?;

    // 3. Validate it is a real 64x32/64x64 PNG before saving.
    let img = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .context(tr(lang, "downloaded skin is not a valid PNG"))?;
    let (w, h) = (img.width(), img.height());
    if !((w == 64 && h == 32) || (w == 64 && h == 64)) {
        return Err(anyhow!(
            "{}",
            tr_fmt(
                lang,
                "unexpected skin size {0}x{1} (expected 64x32 or 64x64)",
                &[&w.to_string(), &h.to_string()]
            )
        ));
    }

    std::fs::create_dir_all(skins_dir)?;
    let path = skins_dir.join(format!("{username}.png"));
    std::fs::write(&path, &bytes)?;
    Ok(path)
}

/// Import a local PNG skin; copies it into `skins_dir`. Returns the skin name.
pub fn import_skin(skins_dir: &Path, source: &Path, lang: Language) -> Result<String> {
    let bytes = std::fs::read(source)
        .with_context(|| tr_fmt(lang, "cannot read {0}", &[&source.display().to_string()]))?;
    let img = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .context(tr(lang, "not a valid PNG skin"))?;
    let (w, h) = (img.width(), img.height());
    if !((w == 64 && h == 32) || (w == 64 && h == 64)) {
        return Err(anyhow!(
            "{}",
            tr_fmt(
                lang,
                "skin must be 64x32 or 64x64, got {0}x{1}",
                &[&w.to_string(), &h.to_string()]
            )
        ));
    }
    std::fs::create_dir_all(skins_dir)?;
    let Some(name) = source.file_stem().and_then(|n| n.to_str()) else {
        return Err(anyhow!("{}", tr(lang, "skin file has no usable name")));
    };
    std::fs::copy(source, skins_dir.join(format!("{name}.png")))?;
    Ok(name.to_string())
}

/// A skin decoded into RGBA pixels ready for texture upload.
#[allow(dead_code)] // `name` is informational
pub struct SkinImage {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Load a skin PNG into RGBA pixels for display.
pub fn load_skin(path: &Path) -> Result<SkinImage> {
    let img = image::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let name = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("skin")
        .to_string();
    Ok(SkinImage {
        name,
        width: img.width(),
        height: img.height(),
        rgba: img.to_rgba8().into_raw(),
    })
}

/// List the skins stored in `skins_dir`.
pub fn list_skins(skins_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(skins_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("png"))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Extract the 8x8 head (face) area of a skin as RGBA, for account avatars.
#[allow(dead_code)] // reserved for avatar rendering
pub fn face_rgba(skin: &SkinImage) -> Option<Vec<u8>> {
    if skin.width != 64 || (skin.height != 32 && skin.height != 64) {
        return None;
    }
    let mut out = Vec::with_capacity(8 * 8 * 4);
    for y in 8..16 {
        for x in 8..16 {
            let i = ((y * skin.width + x) * 4) as usize;
            out.extend_from_slice(&skin.rgba[i..i + 4]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid 64x64 PNG (fully transparent) to exercise loaders.
    fn make_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([120, 200, 90, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }
    /// Windows AV can hold a freshly written file briefly; retry a few times.
    fn retry_load(path: &Path) -> SkinImage {
        let mut last = None;
        for _ in 0..10 {
            match load_skin(path) {
                Ok(skin) => return skin,
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        panic!("skin never became readable: {:?}", last);
    }

    /// Same retry idea for the import call itself.
    fn retry_import(skins_dir: &Path, source: &Path) -> String {
        use crate::lang::Language;
        let mut last = None;
        for _ in 0..10 {
            match import_skin(skins_dir, source, Language::English) {
                Ok(name) => return name,
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        panic!("import never succeeded: {:?}", last);
    }

    #[test]
    fn import_rejects_wrong_size() {
        use crate::lang::Language;
        let dir = std::env::temp_dir().join(format!("rl-skin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bad = dir.join("bad.png");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&bad, make_png(32, 32)).unwrap();
        let err = import_skin(&dir, &bad, Language::English).unwrap_err();
        assert!(err.to_string().contains("64x32 or 64x64"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_accepts_canonical_size_and_lists_it() {
        let dir = std::env::temp_dir().join(format!("rl-skin-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The source lives outside the skins dir (importing a file onto
        // itself would be a self-copy error).
        let downloads = dir.join("downloads");
        let skins = dir.join("skins");
        let src = downloads.join("input.png");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::write(&src, make_png(64, 64)).unwrap();
        let name = retry_import(&skins, &src);
        assert_eq!(name, "input");
        assert_eq!(list_skins(&skins).len(), 1);

        let skin = retry_load(&skins.join("input.png"));
        assert_eq!((skin.width, skin.height), (64, 64));
        let face = face_rgba(&skin).unwrap();
        assert_eq!(face.len(), 8 * 8 * 4);
        // Pixel (8,8) of the solid image.
        assert_eq!(&face[0..4], &[120, 200, 90, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
