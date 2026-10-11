//! The eframe entry point: the frame-update loop, navigation sidebar and the
//! confirmation dialogs that overlay any screen. The screens themselves live
//! in [`super::screens`].

use std::sync::atomic::Ordering;

use super::state::{App, ContentPlatform, Screen};
use crate::gui::theme::icons;
use crate::lang::tr;

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_pending_jobs();
        // The console logging mode is captured into the buffers themselves
        // (worker threads push without seeing the settings).
        self.sync_console_modes();
        self.sync_visuals(ctx);
        self.paint_background(ctx);
        self.tick_toasts();

        // Window close: when the setting asks for it, politely stop every
        // running game (WM_CLOSE, no force kill). This frame is the last
        // one — without CancelClose eframe exits the event loop right
        // after it, so the fire-and-forget taskkill is all we can do.
        if self.settings.close_games_on_exit
            && ctx.input(|i| i.viewport().close_requested())
            && self.any_game_running()
        {
            self.stop_all_games();
        }

        // Drag & drop: in modpack import mode a dropped .mrpack fills the
        // file field of the New instance dialog.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() {
            self.handle_dropped_files(dropped);
        }

        // Repaint while the game runs, an install/download is in progress or
        // a modpack import is running, so the console and progress bars keep
        // moving without user input.
        let busy = self.any_game_running()
            || self.installing.load(Ordering::SeqCst)
            || self.modpack_installing
            || self.content_downloading
            || self.java_downloading.is_some();
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }

        // Toasts animate (slide-out) and must keep repainting while visible.
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(66));
        }

        egui::SidePanel::left("nav").show(ctx, |ui| {
            let lang = self.settings.language;
            ui.add_space(8.0);
            ui.heading("RustLauncher");
            ui.add_space(8.0);
            for (screen, label) in [
                (
                    Screen::General,
                    format!("{}  {}", icons::PLAY_ARROW, tr(lang, "General")),
                ),
                (
                    Screen::Console,
                    format!("{}  {}", icons::TERMINAL, tr(lang, "Console")),
                ),
                (
                    Screen::Instances,
                    format!("{}  {}", icons::VIDEOGAME_ASSET, tr(lang, "Instances")),
                ),
                (
                    Screen::Versions,
                    format!("{}  {}", icons::LAYERS, tr(lang, "Versions")),
                ),
                (
                    Screen::Servers,
                    format!("{}  {}", icons::DNS, tr(lang, "Servers")),
                ),
                (
                    Screen::Accounts,
                    format!("{}  {}", icons::ACCOUNT_CIRCLE, tr(lang, "Accounts")),
                ),
                (
                    Screen::Skins,
                    format!("{}  {}", icons::FACE, tr(lang, "Skins")),
                ),
                (
                    Screen::Modrinth,
                    format!("{}  {}", icons::EXTENSION, tr(lang, "Modrinth")),
                ),
                (
                    Screen::News,
                    format!("{}  {}", icons::ARTICLE, tr(lang, "News")),
                ),
                (
                    Screen::Settings,
                    format!("{}  {}", icons::SETTINGS, tr(lang, "Settings")),
                ),
                (
                    Screen::Diagnostics,
                    format!("{}  {}", icons::BUILD, tr(lang, "Diagnostics")),
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
                ui.label(format!("{}: {}", tr(lang, "Player"), account.username));
            }
            ui.label(format!(
                "{}: {}",
                tr(lang, "Version"),
                self.settings.selected_version
            ));
            if self.any_game_running() {
                ui.colored_label(egui::Color32::LIGHT_GREEN, tr(lang, "Game running"));
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

        if self.settings_reset_pending {
            self.show_settings_reset_confirmation(ctx);
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
