//! Theme application and background painting: egui visuals from the theme
//! settings plus the gradient/photo background layer.

use super::super::state::App;
use crate::services::settings;

impl App {
    pub(crate) fn sync_visuals(&self, ctx: &egui::Context) {
        let theme = &self.settings.theme;
        let mut visuals = if theme.dark_base {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let accent = theme.accent32();
        let button = theme.button32();
        match theme.background {
            settings::BackgroundMode::Color => {
                // Flat color: panels and dialogs take it directly.
                let bg = theme.bg_color32();
                visuals.panel_fill = bg;
                visuals.window_fill = bg;
            }
            settings::BackgroundMode::Gradient | settings::BackgroundMode::Image => {
                // The gradient/photo is painted on the background layer every
                // frame (see `paint_background`); panels stay transparent so
                // it shows through, with a readability veil over it. Dialogs
                // keep the solid base fill so they stay legible.
                visuals.panel_fill = egui::Color32::TRANSPARENT;
            }
        }
        visuals.selection.bg_fill = accent;
        visuals.hyperlink_color = accent;
        visuals.widgets.inactive.bg_fill = button;
        visuals.widgets.inactive.weak_bg_fill = button;
        visuals.widgets.hovered.bg_fill = button;
        visuals.widgets.hovered.weak_bg_fill = button;
        visuals.widgets.active.bg_fill = accent;
        // Optional custom text color, applied to every widget and label.
        visuals.override_text_color = if theme.custom_text {
            Some(theme.text_color32())
        } else {
            None
        };
        ctx.set_visuals(visuals);
    }

    /// Paint the themed window background (gradient or photo) on the
    /// background layer, underneath all panels. Flat-color mode needs no
    /// painting: `sync_visuals` already sets the panel fill.
    pub(crate) fn paint_background(&mut self, ctx: &egui::Context) {
        use settings::BackgroundMode;
        if self.settings.theme.background == BackgroundMode::Color {
            return;
        }
        let screen = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::background());
        match self.settings.theme.background {
            BackgroundMode::Color => {}
            BackgroundMode::Gradient => {
                paint_vertical_gradient(
                    &painter,
                    screen,
                    self.settings.theme.bg_top32(),
                    self.settings.theme.bg_bottom32(),
                );
            }
            BackgroundMode::Image => {
                let path = self.settings.theme.bg_image.clone();
                if path.trim().is_empty() {
                    painter.rect_filled(screen, 0.0, self.settings.theme.bg_color32());
                } else {
                    let needs_load = match &self.bg_texture {
                        Some((cached, _)) => cached != &path,
                        None => true,
                    };
                    if needs_load {
                        let texture = load_background_texture(ctx, &path);
                        self.bg_texture = Some((path, texture));
                    }
                    match &self.bg_texture {
                        Some((_, Some(tex))) => paint_cover(&painter, screen, tex),
                        _ => {
                            painter.rect_filled(screen, 0.0, self.settings.theme.bg_color32());
                        }
                    }
                }
            }
        }
        // Readability veil so text stays legible over photos/gradients.
        let veil = if self.settings.theme.dark_base {
            egui::Color32::from_black_alpha(120)
        } else {
            egui::Color32::from_white_alpha(120)
        };
        painter.rect_filled(screen, 0.0, veil);
    }
}

/// A vertical two-stop gradient filling `rect` (one untextured mesh).
fn paint_vertical_gradient(
    painter: &egui::Painter,
    rect: egui::Rect,
    top: egui::Color32,
    bottom: egui::Color32,
) {
    let uv = egui::Pos2::new(0.0, 0.0);
    let mesh = egui::Mesh {
        indices: vec![0, 1, 2, 0, 2, 3],
        vertices: vec![
            egui::epaint::Vertex {
                pos: rect.left_top(),
                uv,
                color: top,
            },
            egui::epaint::Vertex {
                pos: rect.right_top(),
                uv,
                color: top,
            },
            egui::epaint::Vertex {
                pos: rect.right_bottom(),
                uv,
                color: bottom,
            },
            egui::epaint::Vertex {
                pos: rect.left_bottom(),
                uv,
                color: bottom,
            },
        ],
        texture_id: egui::TextureId::default(),
    };
    painter.add(mesh);
}

/// Paint `tex` stretched to cover `screen` (centered crop, like CSS
/// `background-size: cover`).
fn paint_cover(painter: &egui::Painter, screen: egui::Rect, tex: &egui::TextureHandle) {
    let [iw, ih] = tex.size();
    if iw == 0 || ih == 0 {
        return;
    }
    let scale = (screen.width() / iw as f32).max(screen.height() / ih as f32);
    let size = egui::vec2(iw as f32 * scale, ih as f32 * scale);
    let rect = egui::Rect::from_center_size(screen.center(), size);
    painter.image(
        tex.id(),
        rect,
        egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

/// Decode a background photo (PNG/JPG) into a texture. `None` means the
/// caller falls back to the flat background color.
fn load_background_texture(ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let color = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba.into_raw());
    Some(ctx.load_texture("background", color, egui::TextureOptions::LINEAR))
}
