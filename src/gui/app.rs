//! The eframe entry point: the frame-update loop, navigation sidebar and the
//! confirmation dialogs that overlay any screen.

use std::sync::atomic::Ordering;

use crate::diagnostics;
use crate::icons;
use crate::news::{self, NewsItem};
use crate::settings;

use super::state::{App, ContentPlatform, Screen};

impl App {
    pub(crate) fn ui_news(&mut self, ui: &mut egui::Ui) {
        ui.heading("News");
        ui.add_space(4.0);
        if ui.button("Refresh").clicked() {
            self.news = None;
            self.fetch_news();
        }
        ui.separator();
        match &self.news {
            None => {
                ui.label("Loading…");
            }
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("Feed unavailable: {e}"));
                ui.add_space(4.0);
                for item in news::fallback_news() {
                    self.news_item(ui, &item);
                }
            }
            Some(Ok(items)) => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for item in items {
                        self.news_item(ui, item);
                        ui.separator();
                    }
                });
            }
        }
    }

    pub(crate) fn news_item(&self, ui: &mut egui::Ui, item: &NewsItem) {
        ui.strong(&item.title);
        if !item.date.is_empty() {
            ui.weak(&item.date);
        }
        for line in item.content.lines() {
            ui.label(line);
        }
        ui.add_space(2.0);
    }

    pub(crate) fn ui_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(4.0);
        // Java-mode checkboxes mirror `use_custom_java`; only one is checked.
        let mut default_java = !self.settings.use_custom_java;
        let mut custom_java = self.settings.use_custom_java;
        egui::ScrollArea::vertical().show(ui, |ui| {

            // Java: Default vs Custom, switched with checkboxes.
            ui.strong("Java");
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut default_java, "Default (auto-detect)")
                    .changed()
                    && default_java
                {
                    self.settings.use_custom_java = false;
                }
                if ui.checkbox(&mut custom_java, "Custom path").changed() && custom_java {
                    self.settings.use_custom_java = true;
                }
            });
            ui.add_enabled_ui(self.settings.use_custom_java, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Java executable");
                    ui.add_enabled(
                        true,
                        egui::TextEdit::singleline(&mut self.settings.java_path)
                            .desired_width(360.0),
                    );
                    if ui.button("…").clicked() {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            self.settings.java_path = file.to_string_lossy().to_string();
                        }
                    }
                });
            });
            ui.add_space(4.0);

            // JVM flags: presets + free edit; heap flags are mandatory.
            ui.strong("JVM flags");
            ui.horizontal(|ui| {
                for preset in crate::jvm::PRESETS {
                    let selected = self.settings.java_args == preset.args;
                    if ui
                        .add_enabled(!selected, egui::Button::new(preset.label))
                        .clicked()
                    {
                        self.settings.java_args = preset.args.to_string();
                    }
                }
            });
            ui.add(
                egui::TextEdit::multiline(&mut self.settings.java_args)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            {
                let flags: Vec<String> = self
                    .settings
                    .java_args
                    .split_whitespace()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                match crate::jvm::validate_jvm_args(&flags) {
                    Ok(()) => {
                        ui.colored_label(egui::Color32::LIGHT_GREEN, "✓ heap flags present");
                    }
                    Err(e) => {
                        ui.colored_label(egui::Color32::LIGHT_RED, e);
                    }
                }
            }
            ui.add_space(4.0);

            ui.strong("Game");
            ui.checkbox(
                &mut self.settings.use_custom_resolution,
                "Custom resolution",
            );

            ui.strong("Launcher");
            ui.checkbox(
                &mut self.settings.save_console_log,
                "Save game console to logs/",
            );
            ui.strong("Console log mode");
            for mode in settings::ConsoleMode::ALL {
                ui.radio_value(&mut self.settings.console_log_mode, mode, mode.label());
            }
            ui.checkbox(&mut self.settings.dark_theme, "Dark theme");
        });
        ui.separator();
        if ui.button("Save settings").clicked() {
            self.save_settings();
            self.reload_versions();
            self.play_status = "Settings saved".into();
        }
    }

    pub(crate) fn ui_diagnostics(&mut self, ui: &mut egui::Ui) {
        ui.heading("Network diagnostics");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.diag_running, egui::Button::new("Run tests"))
                .clicked()
            {
                self.diag_running = true;
                self.spawn_job(
                    || diagnostics::run_all(&crate::net::agent()),
                    |app, results| {
                        app.diag_results = Some(results);
                        app.diag_running = false;
                    },
                );
            }
            if self.diag_running {
                ui.label("Testing…");
            }
        });
        ui.separator();
        match &self.diag_results {
            None => {
                ui.weak("Press Run tests to check DNS, HTTP and TCP connectivity.");
            }
            Some(results) => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for result in results {
                        ui.horizontal(|ui| {
                            let color = if result.ok {
                                egui::Color32::LIGHT_GREEN
                            } else {
                                egui::Color32::LIGHT_RED
                            };
                            ui.colored_label(color, if result.ok { "OK  " } else { "FAIL" });
                            ui.monospace(&result.name);
                            ui.weak(&result.detail);
                        });
                    }
                });
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_pending_jobs();
        self.sync_visuals(ctx);
        self.tick_toasts();

        // Repaint while the game runs or a download is in progress so the
        // console and progress labels keep moving without user input.
        let busy = self.any_game_running() || self.installing.load(Ordering::SeqCst);
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }

        // Toasts animate (slide-out) and must keep repainting while visible.
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(66));
        }

        egui::SidePanel::left("nav").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.heading("RustLauncher");
            ui.add_space(8.0);
            for (screen, label) in [
                (Screen::General, format!("{}  General", icons::PLAY_ARROW)),
                (Screen::Console, format!("{}  Console", icons::TERMINAL)),
                (
                    Screen::Instances,
                    format!("{}  Instances", icons::VIDEOGAME_ASSET),
                ),
                (Screen::Versions, format!("{}  Versions", icons::LAYERS)),
                (Screen::Servers, format!("{}  Servers", icons::DNS)),
                (
                    Screen::Accounts,
                    format!("{}  Accounts", icons::ACCOUNT_CIRCLE),
                ),
                (Screen::Skins, format!("{}  Skins", icons::FACE)),
                (Screen::Modrinth, format!("{}  Modrinth", icons::EXTENSION)),
                (Screen::News, format!("{}  News", icons::ARTICLE)),
                (Screen::Settings, format!("{}  Settings", icons::SETTINGS)),
                (
                    Screen::Diagnostics,
                    format!("{}  Diagnostics", icons::BUILD),
                ),
            ] {
                let selected = self.screen == screen;
                if ui.selectable_label(selected, label).clicked() {
                    self.screen = screen;
                    if screen == Screen::Skins {
                        self.refresh_skins();
                        self.select_saved_skin();
                    }
                }
            }
            ui.separator();
            if let Some(account) = self.accounts.current() {
                ui.label(format!("Player: {}", account.username));
            }
            ui.label(format!("Version: {}", self.settings.selected_version));
            if self.any_game_running() {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "Game running");
            }
        });

        self.show_toasts(ctx);

        if self.account_remove_pending.is_some() {
            self.show_account_removal(ctx);
        }

        if self.instance_delete_pending.is_some() {
            self.show_instance_delete_confirmation(ctx);
        }
        if let Some((pid_handle, kind)) = self.instance_terminate_pending.clone() {
            self.show_instance_terminate_confirmation(ctx, &pid_handle, kind);
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.screen {
            Screen::General => self.ui_general(ui),
            Screen::Console => self.ui_console(ui),
            Screen::Instances => self.ui_instances(ui),
            Screen::Versions => self.ui_versions(ui),
            Screen::Servers => self.ui_servers(ui),
            Screen::Accounts => self.ui_accounts(ui),
            Screen::Skins => self.ui_skins(ui, ctx),
            Screen::Modrinth => self.ui_content(ui, ContentPlatform::Modrinth),
            Screen::News => self.ui_news(ui),
            Screen::Settings => self.ui_settings(ui),
            Screen::Diagnostics => self.ui_diagnostics(ui),
        });
    }
}

// ── screens ────────────────────────────────────────────────────
