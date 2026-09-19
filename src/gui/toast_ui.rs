//! Toast rendering: the bottom-left notification stack, the toast card and
//! the material-style painter icons (warning triangle, info, search, cross).

use std::sync::{Arc, Mutex};

use crate::notifications::{Toast, ToastKind};

use super::state::CONSOLE_CAP;

/// Draw one toast card; returns `(close_clicked, body_clicked, size)`.
pub(crate) fn toast_body(
    ui: &mut egui::Ui,
    toast: &Toast,
    kind: ToastKind,
    index: usize,
    expanded: bool,
    expand_progress: f32,
    full_log: Option<&str>,
    lang: crate::lang::Language,
) -> (bool, bool, egui::Vec2) {
    // The ✕ zone's rect, recorded while drawing the header row.
    let cross_rect = std::cell::Cell::new(egui::Rect::NOTHING);

    let frame_rect = egui::Frame::popup(ui.style())
        .fill(match kind {
            ToastKind::Error => egui::Color32::from_rgb(0x3B, 0x2E, 0x2A), // warm dark red-brown
            ToastKind::Info => ui.style().visuals.widgets.inactive.bg_fill,
        })
        .stroke(match kind {
            ToastKind::Error => {
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0xE5, 0x7F, 0x62))
            }
            ToastKind::Info => ui.style().visuals.widgets.inactive.bg_stroke,
        })
        .show(ui, |ui| {
            ui.set_min_width(320.0);
            ui.set_max_width(360.0);
            // Padding inside the frame; used to compute the real text width.
            let inner_pad = 16.0;
            // Width available for text between the icon column and the ✕.
            let text_width = 360.0_f32 - inner_pad - 26.0 - 24.0;

            // The ✕ close zone, as part of the header row. Its clickable
            // response is registered *after* the card-wide body interact
            // (see the tail of this function) so it wins clicks inside it.
            ui.horizontal(|ui| {
                match kind {
                    ToastKind::Error => draw_warning_triangle(ui, 24.0),
                    ToastKind::Info => draw_info_icon(ui, 24.0),
                }
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    // Hard-wrap the title into at most 2 lines with an
                    // ellipsis on the overflow — a very long single-line
                    // error message must never widen the card.
                    ui.set_min_width(text_width);
                    ui.set_max_width(text_width);
                    let title_lines = collapse_detail(&toast.title, 2);
                    for line in &title_lines {
                        ui.add(
                            egui::Label::new(egui::RichText::new(line).strong())
                                .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                    }
                    if let Some(detail) = &toast.detail {
                        let mono = egui::FontId::monospace(10.0);
                        for line in collapse_detail(detail, 3) {
                            let shown = ellipsize_line(ui, &line, text_width, &mono);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(shown).monospace().small().weak(),
                                )
                                .wrap_mode(egui::TextWrapMode::Truncate),
                            );
                        }
                    }
                });

                // Reserve the ✕ space without a click sense here: the ✕
                // click is registered after the body interact below, and
                // egui routes a click to the last registered interact
                // containing the pointer — that is what makes the ✕ win
                // inside its corner.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                cross_rect.set(rect);
            });

            // The expanding log section (error toasts with a log). The
            // height animates 0 → TOAST_MAX_LOG_HEIGHT; the galley is
            // bottom-anchored inside the clip rect so the section visually
            // opens upward from the header.
            if let Some(log) = full_log {
                if expand_progress > 0.001 {
                    let target_h = crate::notifications::TOAST_MAX_LOG_HEIGHT * expand_progress;
                    ui.add_space(6.0 * expand_progress);
                    let (log_rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), target_h),
                        egui::Sense::hover(),
                    );
                    let painter = ui.painter_at(log_rect);
                    painter.rect_filled(log_rect, 2.0, egui::Color32::from_black_alpha(90));
                    let galley = ui.painter().layout(
                        log.to_string(),
                        egui::FontId::monospace(10.0),
                        egui::Color32::from_rgb(0xC8, 0xC8, 0xC8),
                        (log_rect.width() - 8.0).max(40.0),
                    );
                    // Keep the last log line pinned to the bottom of the
                    // section while it grows.
                    let dy = (galley.size().y - log_rect.height()).max(0.0);
                    painter.galley(
                        egui::pos2(log_rect.left() + 4.0, log_rect.top() - dy),
                        galley,
                        egui::Color32::TRANSPARENT,
                    );
                }

                ui.label(
                    egui::RichText::new(if expanded {
                        crate::lang::tr(lang, "▲ click to collapse")
                    } else {
                        crate::lang::tr(lang, "▼ click to expand log")
                    })
                    .small()
                    .weak(),
                );
            }
        })
        .response
        .rect;

    // Hit-testing order matters: egui routes a click to the *last*
    // registered interact containing the pointer, so the card body goes
    // first and the ✕ corner second — that is what makes the ✕ clickable
    // at all (a cross registered before the body never receives clicks).
    let body = ui.interact(
        frame_rect,
        egui::Id::new(("toast_body", index)),
        egui::Sense::click(),
    );
    let cross_area = cross_rect.get();
    let cross = ui.interact(
        cross_area,
        egui::Id::new(("toast_close", index)),
        egui::Sense::click(),
    );

    // Paint the ✕ here so its hover highlight tracks the live pointer.
    ui.painter_at(cross_area)
        .add(draw_material_cross(cross_area, cross.hovered()));

    // Countdown bar along the bottom edge: drains from full to empty over
    // the hold time and disappears once the toast is locked by a click.
    let frac = toast.hold_frac();
    if frac > 0.0 {
        let track = egui::Rect::from_min_max(
            egui::pos2(frame_rect.left() + 2.0, frame_rect.bottom() - 4.0),
            egui::pos2(frame_rect.right() - 2.0, frame_rect.bottom() - 1.0),
        );
        let p = ui.painter_at(frame_rect);
        p.rect_filled(track, 1.0, egui::Color32::from_black_alpha(80));
        let accent = match kind {
            ToastKind::Error => egui::Color32::from_rgb(0xFF, 0xC1, 0x07), // amber 500
            ToastKind::Info => egui::Color32::from_rgb(0x21, 0x96, 0xF3),  // blue 500
        };
        let fill = egui::Rect::from_min_max(
            track.min,
            egui::pos2(track.left() + track.width() * frac, track.max.y),
        );
        p.rect_filled(fill, 1.0, accent);
    }

    let close_clicked = cross.clicked();
    let body_clicked = body.clicked()
        && !close_clicked
        && body
            .interact_pointer_pos()
            .is_none_or(|pos| !cross_area.contains(pos));
    (close_clicked, body_clicked, frame_rect.size())
}

/// Collapse a multi-line detail to at most `max_lines`: everything past
/// the cap is replaced by a single "…" line, so the collapsed toast stays
/// a fixed size no matter how long the message is.
pub(crate) fn collapse_detail(detail: &str, max_lines: usize) -> Vec<String> {
    let lines: Vec<&str> = detail.lines().collect();
    if lines.len() <= max_lines {
        return lines.iter().map(|s| s.to_string()).collect();
    }
    let mut out: Vec<String> = lines[..max_lines.saturating_sub(1)]
        .iter()
        .map(|s| s.to_string())
        .collect();
    out.push("…".to_string());
    out
}

/// Trim one line so it fits `max_width` in the given font, appending "…".
pub(crate) fn ellipsize_line(
    ui: &egui::Ui,
    line: &str,
    max_width: f32,
    font: &egui::FontId,
) -> String {
    let color = ui.visuals().text_color();
    if ui
        .painter()
        .layout_no_wrap(line.to_string(), font.clone(), color)
        .size()
        .x
        <= max_width
    {
        return line.to_string();
    }
    // Binary search the longest prefix that fits, then append the ellipsis.
    let bytes = line.as_bytes();
    let mut lo = 0usize;
    let mut hi = bytes.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if !line.is_char_boundary(mid) {
            hi = mid - 1;
            continue;
        }
        let w = ui
            .painter()
            .layout_no_wrap(line[..mid].to_string(), font.clone(), color)
            .size()
            .x;
        if w <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut end = lo;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    // Make room for the ellipsis itself.
    let ell = "…";
    let ell_w = ui
        .painter()
        .layout_no_wrap(ell.to_string(), font.clone(), color)
        .size()
        .x;
    while end > 0 {
        let cand = &line[..end];
        let w = ui
            .painter()
            .layout_no_wrap(format!("{cand}{ell}"), font.clone(), color)
            .size()
            .x;
        if w <= max_width {
            return format!("{cand}{ell}");
        }
        end -= 1;
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        let _ = ell_w;
    }
    ell.to_string()
}

/// A material-style ✕ cross shape for the given square rect.
pub(crate) fn draw_material_cross(rect: egui::Rect, hovered: bool) -> egui::Shape {
    let color = if hovered {
        egui::Color32::WHITE
    } else {
        egui::Color32::GRAY
    };
    let stroke = egui::Stroke::new(1.6_f32, color);
    let inset = rect.width() * 0.28;
    let a = egui::pos2(rect.left() + inset, rect.top() + inset);
    let b = egui::pos2(rect.right() - inset, rect.bottom() - inset);
    let c = egui::pos2(rect.right() - inset, rect.top() + inset);
    let d = egui::pos2(rect.left() + inset, rect.bottom() - inset);
    egui::Shape::Vec(vec![
        egui::Shape::line_segment([a, b], stroke),
        egui::Shape::line_segment([c, d], stroke),
    ])
}

/// The last `n` lines of a text file, if it can be read.
pub(crate) fn tail_lines(path: &std::path::Path, n: usize) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let start = content.len().saturating_sub(n.saturating_mul(80));
    let window = &content[start.min(content.len())..];
    let lines: Vec<&str> = window
        .lines()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

pub(crate) fn push_line(console: &Arc<Mutex<Vec<String>>>, line: String) {
    let mut buf = console.lock().unwrap_or_else(|e| e.into_inner());
    buf.push(line);
    let excess = buf.len().saturating_sub(CONSOLE_CAP);
    if excess > 0 {
        buf.drain(..excess);
    }
}

/// Draw a material-style warning triangle (yellow fill, black exclamation
/// mark) of the given height, vertically centered on the current layout.
pub(crate) fn draw_warning_triangle(ui: &mut egui::Ui, height: f32) {
    const YELLOW: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xC1, 0x07); // amber 500
    const BLACK: egui::Color32 = egui::Color32::BLACK;

    let (rect, _) = ui.allocate_exact_size(egui::vec2(height * 1.1, height), egui::Sense::hover());
    let p = ui.painter_at(rect);

    // Triangle: apex at the top-center, base at the bottom.
    let top = egui::pos2(rect.center().x, rect.top());
    let left = egui::pos2(rect.left(), rect.bottom());
    let right = egui::pos2(rect.right(), rect.bottom());
    p.add(egui::Shape::convex_polygon(
        vec![top, left, right],
        YELLOW,
        egui::Stroke::NONE,
    ));

    // Rounded exclamation mark: a stem bar plus a dot.
    let cx = rect.center().x;
    let bar_top = rect.top() + height * 0.34;
    let bar_bottom = rect.top() + height * 0.62;
    let stroke = egui::Stroke::new(height * 0.09, BLACK);
    p.line_segment(
        [egui::pos2(cx, bar_top), egui::pos2(cx, bar_bottom)],
        stroke,
    );
    p.circle_filled(
        egui::pos2(cx, rect.top() + height * 0.78),
        height * 0.06,
        BLACK,
    );
}

/// Draw a material-style info icon: a filled blue circle with a white "i",
/// the counterpart of [`draw_warning_triangle`] for informational toasts.
pub(crate) fn draw_info_icon(ui: &mut egui::Ui, height: f32) {
    const BLUE: egui::Color32 = egui::Color32::from_rgb(0x21, 0x96, 0xF3); // blue 500
    const WHITE: egui::Color32 = egui::Color32::WHITE;

    let (rect, _) = ui.allocate_exact_size(egui::vec2(height, height), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let c = rect.center();

    p.circle_filled(c, height / 2.0, BLUE);

    // The "i": a dot above, a stem below.
    p.circle_filled(
        egui::pos2(c.x, rect.top() + height * 0.30),
        height * 0.055,
        WHITE,
    );
    p.line_segment(
        [
            egui::pos2(c.x, rect.top() + height * 0.46),
            egui::pos2(c.x, rect.top() + height * 0.72),
        ],
        egui::Stroke::new(height * 0.09, WHITE),
    );
}

/// A material-style magnifying-glass icon (search), stroked in the text
/// color so it sits naturally in front of a text input.
pub(crate) fn draw_search_icon(ui: &mut egui::Ui, height: f32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(height, height), egui::Sense::hover());
    let p = ui.painter_at(rect);

    // Lens circle, offset toward the top-left.
    let lens_r = height * 0.30;
    let lens_c = egui::pos2(rect.left() + height * 0.40, rect.top() + height * 0.40);
    // The handle starts on the lens rim toward the bottom-right.
    let handle_dir = std::f32::consts::SQRT_2 / 2.0;
    let handle_start = egui::pos2(
        lens_c.x + lens_r * handle_dir,
        lens_c.y + lens_r * handle_dir,
    );
    let handle_end = egui::pos2(rect.right() - height * 0.10, rect.bottom() - height * 0.10);

    p.circle_stroke(lens_c, lens_r, egui::Stroke::new(height * 0.10, color));
    p.line_segment(
        [handle_start, handle_end],
        egui::Stroke::new(height * 0.10, color),
    );
}
