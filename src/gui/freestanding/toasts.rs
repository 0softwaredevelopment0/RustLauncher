//! Launcher-side logging and the toast notification stack: starting the
//! launcher log file, recording errors/informational events, toast aging
//! and rendering.

use super::super::state::App;
use super::super::toast_ui::{tail_lines, toast_body};
use crate::lang::tr;
use crate::services::notifications::{Toast, ToastKind};
use crate::util::home;
use crate::util::logs::SessionLog;
use crate::services::settings;

impl App {
    /// Open `logs/launcher-N.log` for the whole app lifetime; the file
    /// captures what `file_log` allows (Nothing = no file at all).
    pub(crate) fn start_launcher_log(&mut self) {
        if self.settings.file_log == settings::FileLogMode::Nothing {
            return;
        }
        match SessionLog::start(&home::logs_dir(&self.home_dir), "launcher") {
            Ok(log) => {
                log.write(&format!(
                    "[Log] RustLauncher {} started",
                    env!("CARGO_PKG_VERSION")
                ));
                self.launcher_log = Some(log);
            }
            Err(e) => eprintln!("[RustLauncher] launcher log failed: {e:#}"),
        }
    }

    /// Record a launcher error: to the launcher log, console and as a toast.
    pub(crate) fn notify_error(&mut self, code: &str, message: impl std::fmt::Display) {
        let lang = self.settings.language;
        let line = format!("[ERROR {code}] {message}");
        self.log_launcher_line(&line);
        self.log_console(line.clone());

        // The collapsed toast shows the last 3 lines; a click expands the
        // full (capped) log, animated.
        let log_path = self.launcher_log.as_ref().map(|l| l.path.clone());
        let detail = log_path.as_deref().and_then(|p| tail_lines(p, 3));
        let full_log = log_path
            .as_deref()
            .and_then(|p| tail_lines(p, crate::services::notifications::TOAST_MAX_LOG_LINES));

        self.toasts.push(Toast::error(
            tr(lang, "An error occurred"),
            Some(code.to_string()),
            detail,
            full_log,
        ));
    }

    /// Push an informational toast (game started/stopped etc.).
    pub(crate) fn notify_info(&mut self, title: impl Into<String>) {
        self.toasts.push(Toast::info(title));
    }

    /// Write a launcher-side line to the launcher log file, honoring the
    /// `file_log` mode.
    pub(crate) fn log_launcher_line(&self, line: &str) {
        if !self.settings.file_log.allows(line) {
            return;
        }
        if let Some(log) = &self.launcher_log {
            log.write(line);
        }
    }

    /// Advance toast aging; called once per frame.
    pub(crate) fn tick_toasts(&mut self) {
        let now = std::time::Instant::now();
        let dt = self
            .last_frame
            .take()
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.0);
        self.last_frame = Some(now);
        self.toasts.tick(dt);
    }

    /// Render the toast stack anchored to the bottom-left corner of the
    /// screen. Each toast is its own Area anchored at LEFT_BOTTOM, offset
    /// upward by the measured heights of the toasts above it, so the stack
    /// always hugs the corner regardless of screen size.
    pub(crate) fn show_toasts(&mut self, ctx: &egui::Context) {
        // Deferred actions: mutating self inside the Area closure fights the
        // outer borrow, so collect and apply them after the UI pass.
        enum Action {
            Pin(usize),
            Close(usize),
            Toggle(usize),
        }
        let mut actions: Vec<Action> = Vec::new();

        const MARGIN: f32 = 12.0;
        const GAP: f32 = 8.0;

        // Newest on top: iterate the queue in reverse, stacking each toast
        // below the previous one.
        let mut y_offset: f32 = 0.0;
        for i in (0..self.toasts.items().len()).rev() {
            let kind = self.toasts.items()[i].kind;
            let slide = self.toasts.items()[i].slide_offset();
            let anchor_y = -MARGIN - y_offset;
            let (close_pressed, body_clicked, height) =
                self.toast_area(ctx, i, egui::vec2(MARGIN + slide, anchor_y));
            if close_pressed {
                actions.push(Action::Close(i));
            } else if body_clicked {
                actions.push(Action::Pin(i));
                if kind == ToastKind::Error {
                    actions.push(Action::Toggle(i));
                }
            }
            // Stack the next (older) toast below this one; fall back to an
            // estimate until the first render has measured the real height.
            let h = if height > 0.0 { height } else { 70.0 };
            y_offset += h + GAP;
        }

        for action in actions {
            match action {
                Action::Close(i) => self.toasts.remove(i),
                Action::Pin(i) => {
                    if let Some(t) = self.toasts.items_mut().get_mut(i) {
                        t.pin();
                    }
                }
                Action::Toggle(i) => {
                    if let Some(t) = self.toasts.items_mut().get_mut(i) {
                        t.toggle_expanded();
                    }
                }
            }
        }
    }

    /// Render one toast in its own bottom-left-anchored Area; returns
    /// `(close_clicked, body_clicked, measured_height)`.
    pub(crate) fn toast_area(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        anchor_offset: egui::Vec2,
    ) -> (bool, bool, f32) {
        let toast = &self.toasts.items()[index];
        let alpha = toast.visual_alpha();
        let kind = toast.kind;
        let expanded = toast.expanded();
        let expand_progress = toast.expand_progress();
        let full_log = toast.full_log().map(str::to_string);
        let id = egui::Id::new(("toast", index));

        let (close_clicked, body_clicked, size) = egui::Area::new(id)
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::LEFT_BOTTOM, anchor_offset)
            .show(ctx, |ui| {
                ui.multiply_opacity(alpha);
                toast_body(
                    ui,
                    toast,
                    kind,
                    index,
                    expanded,
                    expand_progress,
                    full_log.as_deref(),
                    self.settings.language,
                )
            })
            .inner;

        // Record the measured height for the next frame's stacking.
        if let Some(t) = self.toasts.items_mut().get_mut(index) {
            t.set_height(size.y);
        }
        (close_clicked, body_clicked, size.y)
    }
}
