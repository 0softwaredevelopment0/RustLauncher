//! The main screens: General, Console, Instances (with their confirmation
//! dialogs), Versions, Servers, Accounts (including the removal dialog) and
//! Skins.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::accounts::AccountStore;
use crate::auth::{self, AccountKind};
use crate::home;
use crate::icons;
use crate::servers;
use crate::skins;
use crate::updater;

use super::state::{
    merge_versions, sort_version_rows, App, LauncherPaths, MsLoginState, Screen, TerminateKind,
    VersionFilter, VersionRow, VersionSort,
};
use super::toast_ui::{draw_search_icon, draw_warning_triangle};
use crate::settings;

impl App {
    pub(crate) fn ui_general(&mut self, ui: &mut egui::Ui) {
        ui.heading("General");
        ui.add_space(6.0);

        egui::Grid::new("play_grid").num_columns(2).show(ui, |ui| {
            ui.label("Instance");
            let instance_names: Vec<String> = self
                .instance_store
                .instances
                .iter()
                .map(|i| i.name.clone())
                .collect();
            if self.launch_instance.is_empty() && !instance_names.is_empty() {
                self.launch_instance = instance_names[0].clone();
            }
            egui::ComboBox::from_id_salt("instance_combo")
                .selected_text(if self.launch_instance.is_empty() {
                    "— none —".to_string()
                } else {
                    self.launch_instance.clone()
                })
                .show_ui(ui, |ui| {
                    for name in &instance_names {
                        ui.selectable_value(&mut self.launch_instance, name.clone(), name.clone());
                    }
                });
            ui.end_row();

            ui.label("Account");
            let current = self
                .accounts
                .current()
                .map(|a| a.username.clone())
                .unwrap_or_default();
            ui.label(if current.is_empty() {
                "— none —".to_string()
            } else {
                current
            });
            ui.end_row();

            ui.label("Version");
            let names: Vec<String> = self.versions.iter().map(|v| v.name.clone()).collect();
            egui::ComboBox::from_id_salt("version_combo")
                .selected_text(if self.settings.selected_version.is_empty() {
                    "— select —".to_string()
                } else {
                    self.settings.selected_version.clone()
                })
                .show_ui(ui, |ui| {
                    for name in &names {
                        ui.selectable_value(
                            &mut self.settings.selected_version,
                            name.clone(),
                            name,
                        );
                    }
                });
            ui.end_row();

            if self.settings.auto_connect {
                ui.label("Auto-connect");
                ui.text_edit_singleline(&mut self.settings.connect_server_ip);
                ui.end_row();
            }
        });
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            // Several instances may run at once; only the selected one being
            // alive blocks the button.
            let play_enabled = !self.instance_running(&self.launch_instance);
            if ui
                .add_enabled(
                    play_enabled,
                    egui::Button::new(format!("{}  Launch", icons::PLAY_ARROW)),
                )
                .clicked()
            {
                self.start_game();
            }
            if ui.button("Rescan versions").clicked() {
                self.reload_versions();
            }
            let game_running = self.any_game_running();
            let stop = egui::Button::new(format!("{} Stop", icons::STOP));
            if ui
                .add_enabled(game_running, stop)
                .on_disabled_hover_text("The game is not running")
                .clicked()
            {
                if self.settings.confirm_stop {
                    self.terminate_dont_ask = false;
                    self.terminate_confirm = Some(TerminateKind::Stop);
                } else {
                    self.stop_game();
                }
            }
            let kill = egui::Button::new(
                egui::RichText::new(format!("{} Kill", icons::KILL))
                    .color(egui::Color32::LIGHT_RED),
            );
            if ui
                .add_enabled(game_running, kill)
                .on_disabled_hover_text("The game is not running")
                .clicked()
            {
                if self.settings.confirm_kill {
                    self.terminate_dont_ask = false;
                    self.terminate_confirm = Some(TerminateKind::Kill);
                } else {
                    self.kill_game();
                }
            }
        });

        if let Some(kind) = self.terminate_confirm {
            let ctx = ui.ctx().clone();
            self.show_terminate_confirmation(&ctx, kind);
        }

        if !self.play_status.is_empty() {
            ui.add_space(6.0);
            ui.label(&self.play_status);
        }
        if let Some(error) = &self.launch_error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }

    pub(crate) fn ui_console(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Console");
            // When several games run at once, pick which console to show.
            let alive: Vec<(usize, String)> = self
                .running_games
                .iter()
                .enumerate()
                .filter(|(_, g)| g.running.load(Ordering::SeqCst))
                .map(|(idx, g)| (idx, g.instance.clone()))
                .collect();
            if alive.len() > 1 {
                let current = alive
                    .iter()
                    .find(|(idx, _)| {
                        self.running_games
                            .get(*idx)
                            .map(|g| Arc::ptr_eq(&g.console, &self.console))
                            .unwrap_or(false)
                    })
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| "…".to_string());
                egui::ComboBox::from_id_salt("console_instance")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (idx, name) in &alive {
                            if ui.selectable_label(false, name.clone()).clicked() {
                                if let Some(g) = self.running_games.get(*idx) {
                                    self.console = g.console.clone();
                                    self.console_seq = 0;
                                }
                            }
                        }
                    });
            }
            // How much of the game output to display in the console
            // (display filter only — log files are configured in Settings).
            ui.label("Show in console:");
            egui::ComboBox::from_id_salt("console_mode")
                .selected_text(self.settings.console_log_mode.label())
                .width(150.0)
                .show_ui(ui, |ui| {
                    for mode in settings::ConsoleMode::ALL {
                        ui.selectable_value(
                            &mut self.settings.console_log_mode,
                            mode,
                            mode.label(),
                        );
                    }
                });
            if ui.button("Export to file…").clicked() {
                self.export_console();
            }
            if ui.button("Clear").clicked() {
                self.console
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clear();
            }
            if ui.button("Copy all").clicked() {
                let text = self
                    .console
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .join("\n");
                ui.output_mut(|o| o.copied_text = text);
            }
        });
        ui.separator();

        let all_lines: Vec<String> = {
            let buf = self.console.lock().unwrap_or_else(|e| e.into_inner());
            buf.clone()
        };
        let changed = all_lines.len() != self.console_seq;
        self.console_seq = all_lines.len();

        // Apply the log mode: launcher's own lines always pass, game output
        // is filtered by the selected mode.
        let lines: Vec<&String> = all_lines
            .iter()
            .filter(|l| self.settings.console_log_mode.allows_launcher_aware(l))
            .collect();

        egui::ScrollArea::vertical()
            .stick_to_bottom(changed)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    ui.set_min_width(ui.available_width());
                    egui::Grid::new("console_grid")
                        .num_columns(1)
                        .show(ui, |ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                            ui.style_mut()
                                .text_styles
                                .insert(egui::TextStyle::Body, egui::FontId::monospace(12.0));
                            for line in &lines {
                                ui.label(line.as_str());
                                ui.end_row();
                            }
                        });
                });
            });
    }

    // ── Instances tab ───────────────────────────────────────

    pub(crate) fn ui_instances(&mut self, ui: &mut egui::Ui) {
        ui.heading(format!("{}  Instances", icons::VIDEOGAME_ASSET));
        ui.add_space(6.0);

        // ── Create section ──
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("{}  New instance", icons::ADD)).strong(),
        )
        .default_open(self.instance_store.instances.is_empty())
        .show(ui, |ui| {
            egui::Grid::new("new_instance_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_instance_name)
                            .hint_text("My instance")
                            .desired_width(260.0),
                    );
                    ui.end_row();

                    ui.label("Game directory");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_instance_dir)
                                .hint_text("C:\\Games\\MyPack")
                                .desired_width(260.0),
                        );
                        if ui.button(format!("{} Browse…", icons::FOLDER)).clicked() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                self.new_instance_dir = dir.to_string_lossy().to_string();
                            }
                        }
                    });
                    ui.end_row();
                });
            if ui
                .button(format!("{}  Create instance", icons::CHECK_CIRCLE))
                .clicked()
            {
                self.instances_error = None;
                match self
                    .instance_store
                    .create(&self.new_instance_name, &self.new_instance_dir)
                {
                    Ok(name) => {
                        if let Err(e) = self.instance_store.save(&self.home_dir) {
                            self.instances_error = Some(format!("{e:#}"));
                            self.notify_error("INSTANCES", format!("{e:#}"));
                        } else {
                            self.notify_info(format!("Instance '{name}' created"));
                            self.new_instance_name.clear();
                            self.new_instance_dir.clear();
                        }
                    }
                    Err(e) => {
                        self.instances_error = Some(format!("{e:#}"));
                    }
                }
            }
        });

        ui.add_space(4.0);
        ui.separator();

        // ── Running games ──
        let running_count = self.running_games.len();
        if running_count > 0 {
            egui::CollapsingHeader::new(
                egui::RichText::new(format!(
                    "{}  Running games ({})",
                    icons::PLAY_ARROW,
                    self.running_games
                        .iter()
                        .filter(|g| g.running.load(Ordering::SeqCst))
                        .count()
                ))
                .strong(),
            )
            .default_open(true)
            .show(ui, |ui| {
                let entries: Vec<(usize, String, String, bool)> = self
                    .running_games
                    .iter()
                    .enumerate()
                    .map(|(idx, g)| {
                        (
                            idx,
                            g.instance.clone(),
                            g.version.clone(),
                            g.running.load(Ordering::SeqCst),
                        )
                    })
                    .collect();
                for (idx, inst, ver, alive) in entries {
                    ui.horizontal(|ui| {
                        let status_icon = if alive {
                            egui::RichText::new(icons::CHECK_CIRCLE)
                                .color(egui::Color32::LIGHT_GREEN)
                        } else {
                            egui::RichText::new(icons::STOP).color(egui::Color32::GRAY)
                        };
                        ui.label(status_icon);
                        ui.label(format!("{inst} — {ver}"));
                        if alive {
                            ui.weak("(running)");
                        } else if let Some(g) = self.running_games.get(idx) {
                            let s = g.status.lock().unwrap_or_else(|e| e.into_inner()).clone();
                            if !s.is_empty() {
                                ui.weak(format!("({s})"));
                            }
                        }

                        if ui
                            .add_enabled(alive, egui::Button::new(format!("{} Stop", icons::STOP)))
                            .on_disabled_hover_text("Not running")
                            .clicked()
                        {
                            let pid = self.running_games[idx].pid.clone();
                            if self.settings.confirm_stop {
                                self.instance_terminate_pending = Some((pid, TerminateKind::Stop));
                            } else {
                                self.stop_instance_pid(idx);
                            }
                        }
                        if ui
                            .add_enabled(
                                alive,
                                egui::Button::new(
                                    egui::RichText::new(format!("{} Kill", icons::KILL))
                                        .color(egui::Color32::LIGHT_RED),
                                ),
                            )
                            .on_disabled_hover_text("Not running")
                            .clicked()
                        {
                            let pid = self.running_games[idx].pid.clone();
                            if self.settings.confirm_kill {
                                self.instance_terminate_pending = Some((pid, TerminateKind::Kill));
                            } else {
                                self.kill_instance_pid(idx);
                            }
                        }
                        if ui.button("Console").clicked() {
                            if let Some(g) = self.running_games.get(idx) {
                                self.console = g.console.clone();
                                self.console_seq = 0;
                            }
                            self.screen = Screen::Console;
                        }
                    });
                    if let Some(g) = self.running_games.get(idx) {
                        let err = g.error.lock().unwrap_or_else(|e| e.into_inner()).clone();
                        if let Some(err) = err {
                            ui.colored_label(egui::Color32::LIGHT_RED, format!("Error: {err}"));
                        }
                    }
                }
                ui.add_space(4.0);
                // Clean up finished entries on demand.
                if ui.button("Clear finished").clicked() {
                    self.running_games
                        .retain(|g| g.running.load(Ordering::SeqCst));
                }
            });
            ui.separator();
        }

        // ── Instance list ──
        if self.instance_store.instances.is_empty() {
            ui.add_space(6.0);
            ui.weak("No instances yet — create one above to start playing.");
        }

        let names: Vec<String> = self
            .instance_store
            .instances
            .iter()
            .map(|i| i.name.clone())
            .collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for name in &names {
                let Some(instance) = self.instance_store.get(name).cloned() else {
                    continue;
                };
                let game = self
                    .running_games
                    .iter()
                    .position(|g| g.instance == *name && g.running.load(Ordering::SeqCst));

                ui.add_space(2.0);
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!(
                        "{}  {}{}",
                        icons::VIDEOGAME_ASSET,
                        instance.name,
                        if game.is_some() { "  ● running" } else { "" }
                    ))
                    .strong(),
                )
                .id_salt(("instance", name.as_str()))
                .show(ui, |ui| {
                    ui.label(format!("Directory: {}", instance.game_dir));

                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                game.is_none(),
                                egui::Button::new(format!("{}  Launch", icons::PLAY_ARROW)),
                            )
                            .clicked()
                        {
                            self.launch_instance = instance.name.clone();
                            self.start_game();
                        }

                        let stop_kill_enabled = game.is_some();
                        if ui
                            .add_enabled(
                                stop_kill_enabled,
                                egui::Button::new(format!("{} Stop", icons::STOP)),
                            )
                            .on_disabled_hover_text("Not running")
                            .clicked()
                        {
                            let idx = game.unwrap();
                            let pid = self.running_games[idx].pid.clone();
                            if self.settings.confirm_stop {
                                self.instance_terminate_pending = Some((pid, TerminateKind::Stop));
                            } else {
                                self.stop_instance_pid(idx);
                            }
                        }
                        if ui
                            .add_enabled(
                                stop_kill_enabled,
                                egui::Button::new(
                                    egui::RichText::new(format!("{} Kill", icons::KILL))
                                        .color(egui::Color32::LIGHT_RED),
                                ),
                            )
                            .on_disabled_hover_text("Not running")
                            .clicked()
                        {
                            let idx = game.unwrap();
                            let pid = self.running_games[idx].pid.clone();
                            if self.settings.confirm_kill {
                                self.instance_terminate_pending = Some((pid, TerminateKind::Kill));
                            } else {
                                self.kill_instance_pid(idx);
                            }
                        }
                    });

                    if let Some(idx) = game {
                        if let Some(g) = self.running_games.get(idx) {
                            let err = g.error.lock().unwrap_or_else(|e| e.into_inner()).clone();
                            if let Some(err) = err {
                                ui.colored_label(egui::Color32::LIGHT_RED, format!("Error: {err}"));
                            }
                            let status = g.status.lock().unwrap_or_else(|e| e.into_inner()).clone();
                            if !status.is_empty() {
                                ui.weak(status);
                            }
                        }
                    }

                    ui.horizontal(|ui| {
                        let game_alive = self.instance_running(&instance.name);
                        if ui
                            .add_enabled(
                                !game_alive,
                                egui::Button::new(
                                    egui::RichText::new(format!("{}  Delete", icons::DELETE))
                                        .color(egui::Color32::LIGHT_RED),
                                ),
                            )
                            .on_disabled_hover_text("Stop the game before deleting")
                            .clicked()
                        {
                            self.instance_delete_pending = Some(instance.name.clone());
                        }
                    });
                });
            }
        });

        if let Some(err) = &self.instances_error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        }
    }

    /// The instance-delete confirmation dialog (same style as Stop/Kill).
    pub(crate) fn show_instance_delete_confirmation(&mut self, ctx: &egui::Context) {
        let Some(name) = self.instance_delete_pending.clone() else {
            return;
        };
        let screen = ctx.screen_rect();

        egui::Area::new(egui::Id::new("instance_delete_dim"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.left_top())
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
                if resp.clicked() {
                    self.instance_delete_pending = None;
                }
            });

        egui::Window::new(egui::RichText::new("Confirm").strong())
            .id(egui::Id::new("instance_delete_dialog"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .fixed_size(egui::vec2(400.0, 190.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    draw_warning_triangle(ui, 36.0);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Delete this instance?")
                                .strong()
                                .size(16.0),
                        );
                        ui.label(format!(
                            "'{name}' will be removed from the launcher. The game directory on \
                             disk will NOT be deleted."
                        ));
                    });
                });
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(egui::RichText::new("Confirm").strong()))
                        .clicked()
                    {
                        self.instance_delete_pending = None;
                        if self.instance_store.delete(&name) {
                            if let Err(e) = self.instance_store.save(&self.home_dir) {
                                self.instances_error = Some(format!("{e:#}"));
                                self.notify_error("INSTANCES", format!("{e:#}"));
                            } else {
                                self.notify_info(format!("Instance '{name}' deleted"));
                                if self.launch_instance == name {
                                    self.launch_instance.clear();
                                }
                            }
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.instance_delete_pending = None;
                    }
                });
            });
    }

    /// Stop/Kill confirmation for a specific instance entry (reuses the
    /// standard terminate dialog text and the "don't ask again" flag).
    pub(crate) fn show_instance_terminate_confirmation(
        &mut self,
        ctx: &egui::Context,
        pid_handle: &Arc<Mutex<Option<u32>>>,
        kind: TerminateKind,
    ) {
        // The game may have exited (or the registry shifted) while the
        // dialog was open — resolve the entry by its PID handle, not index.
        let Some(game) = self
            .running_games
            .iter()
            .find(|g| Arc::ptr_eq(&g.pid, pid_handle))
        else {
            self.instance_terminate_pending = None;
            return;
        };
        let name = game.instance.clone();
        let (title, body) = match kind {
            TerminateKind::Stop => (
                format!("Stop {name}?"),
                "The game will be asked to close. It usually exits within a few seconds, but unsaved progress may be lost.",
            ),
            TerminateKind::Kill => (
                format!("Force kill {name}?"),
                "The game process tree will be terminated immediately. Unsaved progress will be lost.",
            ),
        };
        let screen = ctx.screen_rect();

        egui::Area::new(egui::Id::new("instance_terminate_dim"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.left_top())
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
                if resp.clicked() {
                    self.instance_terminate_pending = None;
                }
            });

        let kill_confirmed = kind == TerminateKind::Kill;
        egui::Window::new(egui::RichText::new("Confirm").strong())
            .id(egui::Id::new("instance_terminate_dialog"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .fixed_size(egui::vec2(400.0, 190.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    draw_warning_triangle(ui, 36.0);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(title).strong().size(16.0));
                        ui.label(body);
                    });
                });
                ui.add_space(10.0);
                ui.checkbox(&mut self.terminate_dont_ask, "Don't ask again");
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(egui::RichText::new("Confirm").strong()))
                        .clicked()
                    {
                        self.instance_terminate_pending = None;
                        if self.terminate_dont_ask {
                            match kind {
                                TerminateKind::Stop => self.settings.confirm_stop = false,
                                TerminateKind::Kill => self.settings.confirm_kill = false,
                            }
                            self.save_settings();
                        }
                        if kill_confirmed {
                            self.kill_pid(*pid_handle.lock().unwrap_or_else(|e| e.into_inner()));
                        } else {
                            self.stop_pid(*pid_handle.lock().unwrap_or_else(|e| e.into_inner()));
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.instance_terminate_pending = None;
                    }
                });
            });
    }

    pub(crate) fn ui_versions(&mut self, ui: &mut egui::Ui) {
        ui.heading("Versions");
        ui.add_space(4.0);

        if self.manifest.is_none() && !self.manifest_loading {
            self.manifest_loading = true;
            self.spawn_job(
                || updater::fetch_manifest(&crate::net::agent()).map_err(|e| e.to_string()),
                |app, result| {
                    app.manifest = Some(result);
                    app.manifest_loading = false;
                },
            );
        }

        // ── Filters section (top level, above the list) ──
        ui.strong("Filters");
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            for filter in [
                VersionFilter::All,
                VersionFilter::Mojang,
                VersionFilter::Loaders,
                VersionFilter::Release,
                VersionFilter::Snapshot,
                VersionFilter::Old,
                VersionFilter::Installed,
            ] {
                ui.selectable_value(&mut self.version_filter, filter, filter.label());
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.weak("Loader:");
            ui.selectable_value(&mut self.version_loader_filter, None, "Any");
            for l in updater::Loader::ALL {
                ui.selectable_value(&mut self.version_loader_filter, Some(l), l.label());
            }
        });
        ui.horizontal(|ui| {
            draw_search_icon(ui, 16.0, ui.visuals().text_color());
            ui.add(
                egui::TextEdit::singleline(&mut self.version_search)
                    .hint_text("Search…")
                    .desired_width(200.0),
            );
            if ui.button("Rescan").clicked() {
                self.reload_versions();
            }
            if ui.button("Refresh manifest").clicked() {
                self.manifest = None;
            }
        });
        ui.separator();

        // ── Loader installer (top level, above the list) ──
        let installing = self.installing.load(Ordering::SeqCst);
        let progress = self
            .install_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if installing {
            ui.label(&progress);
            ui.add(egui::ProgressBar::new(1.0).animate(true).text("working…"));
            ui.separator();
        }
        egui::CollapsingHeader::new("Install a mod loader")
            .id_salt("loader_installer")
            .default_open(false)
            .show(ui, |ui| {
                self.ui_loader_row(ui, installing);
            });
        ui.separator();

        if let Some(Err(e)) = &self.manifest {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("Manifest unavailable ({e}) — showing local versions only"),
            );
        }

        // ── Sort bar (same level as the list) ──
        ui.horizontal(|ui| {
            ui.weak("Sort:");
            egui::ComboBox::from_id_salt("version_sort")
                .selected_text(self.version_sort.label())
                .width(140.0)
                .show_ui(ui, |ui| {
                    for s in [
                        VersionSort::Newest,
                        VersionSort::Number,
                        VersionSort::ReleaseDate,
                        VersionSort::LoaderType,
                        VersionSort::Alphabetical,
                        VersionSort::AlphabeticalReverse,
                    ] {
                        ui.selectable_value(&mut self.version_sort, s, s.label());
                    }
                });
            let total = self.versions.len();
            ui.weak(format!("{} installed", total));
        });

        let manifest_opt = match &self.manifest {
            Some(Ok(manifest)) => Some(manifest),
            _ => None,
        };
        let rows = merge_versions(&self.versions, manifest_opt);
        let search = self.version_search.trim().to_lowercase();
        let mut shown: Vec<VersionRow> = rows
            .iter()
            .filter(|row| self.version_filter.matches(row))
            .filter(|row| {
                self.version_loader_filter.is_none() || row.loader == self.version_loader_filter
            })
            .filter(|row| search.is_empty() || row.name.to_lowercase().contains(&search))
            .cloned()
            .collect();
        sort_version_rows(&mut shown, self.version_sort);

        ui.weak(format!(
            "selected: {}",
            if self.settings.selected_version.is_empty() {
                "— none —"
            } else {
                &self.settings.selected_version
            }
        ));

        // The version list takes all remaining space.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in shown.iter() {
                    self.version_row_ui(ui, row, installing);
                }
                if shown.is_empty() {
                    ui.weak("No versions match the current filter.");
                }
            });
    }

    /// Render one row of the version list (Select / Install controls).
    pub(crate) fn version_row_ui(&mut self, ui: &mut egui::Ui, row: &VersionRow, installing: bool) {
        ui.horizontal(|ui| {
            if row.installed {
                ui.label("✔");
            } else {
                ui.label(" ");
            }
            ui.monospace(&row.name);
            if let Some(loader) = row.loader {
                ui.colored_label(
                    egui::Color32::from_rgb(0xBA, 0x8E, 0xFF),
                    format!("[{}]", loader.label()),
                );
            } else {
                ui.weak(format!("[{}]", row.kind));
            }
            if row.is_latest_release {
                ui.weak("(latest release)");
            }
            let is_selected = self.settings.selected_version == row.name;
            if is_selected {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "selected");
            }
            if row.installed {
                if is_selected {
                    ui.weak("current");
                } else if ui.button("Select").clicked() {
                    self.settings.selected_version = row.name.clone();
                    self.save_settings();
                }
            } else if ui
                .add_enabled(!installing, egui::Button::new("Install"))
                .clicked()
            {
                if let Some(remote) = row.remote.clone() {
                    self.install_version(remote);
                }
            }
        });
    }

    /// Load (or take from cache) the loader builds for `mc`.
    pub(crate) fn ensure_loader_builds(&mut self, loader: updater::Loader, mc: &str) {
        let key = (loader, mc.to_string());
        if self.loader_builds.contains_key(&key) || self.loader_builds_loading {
            return;
        }
        self.loader_builds_loading = true;
        let mc_task = mc.to_string();
        let mc_key = mc.to_string();
        self.spawn_job(
            move || {
                let agent = crate::net::agent();
                updater::fetch_loader_builds(&agent, loader, &mc_task).map_err(|e| e.to_string())
            },
            move |app, result| {
                app.loader_builds.insert((loader, mc_key), Some(result));
                app.loader_builds_loading = false;
            },
        );
    }

    pub(crate) fn install_loader(
        &mut self,
        mc: String,
        loader: updater::Loader,
        build: updater::LoaderBuild,
    ) {
        self.installing.store(true, Ordering::SeqCst);
        self.install_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let game_dir = self.active_game_dir();
        let progress = self.install_progress.clone();
        let installing = self.installing.clone();
        let mc_display = mc.clone();

        self.spawn_job(
            move || {
                let agent = crate::net::agent();
                let result =
                    updater::install_loader(&agent, &game_dir, loader, &mc, &build, &mut |msg| {
                        *progress.lock().unwrap_or_else(|e| e.into_inner()) = msg.to_string();
                    });
                installing.store(false, Ordering::SeqCst);
                result.map_err(|e| e.to_string())
            },
            move |app, result| {
                let mc = mc_display;
                match &result {
                    Ok(outcome) => {
                        app.settings.selected_version = outcome.version_id.clone();
                        app.save_settings();
                        app.log_console(format!(
                            "[RustLauncher] installed {} ({} new files)",
                            outcome.version_id, outcome.downloaded_files
                        ));
                        app.notify_info(format!(
                            "{} {} on {mc} is ready to play",
                            loader.label(),
                            outcome.version_id
                        ));
                    }
                    Err(e) => {
                        app.notify_error(
                            "LOADER-INSTALL",
                            format!(
                                "{loader_label} install failed: {e}",
                                loader_label = loader.label()
                            ),
                        );
                    }
                }
                app.reload_versions();
                app.play_status.clear();
            },
        );
    }

    pub(crate) fn ui_loader_row(&mut self, ui: &mut egui::Ui, installing: bool) {
        ui.strong("Install a mod loader");
        ui.add_space(2.0);

        // The game root must be configured; the installer writes into
        // `<game dir>/versions/…`.
        let game_dir = self.active_game_dir();
        let game_dir_ok = game_dir.exists();
        if !game_dir_ok {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!(
                    "Root game directory is not set — configure it in Settings; installs would go to {}",
                    game_dir.display()
                ),
            );
        }

        // Direct Mojang release choice (releases only).
        let releases: Vec<String> = self
            .manifest
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|m| {
                m.versions
                    .iter()
                    .filter(|v| v.kind == "release")
                    .map(|v| v.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if releases.is_empty() {
            ui.weak("Load the Mojang manifest (Refresh manifest) to pick a version.");
            return;
        }
        let mc_selected = self
            .loader_mc_pick
            .clone()
            .unwrap_or_else(|| releases[0].clone());
        egui::ComboBox::from_label("Minecraft")
            .width(160.0)
            .selected_text(&mc_selected)
            .show_ui(ui, |ui| {
                for r in &releases {
                    ui.selectable_value(
                        self.loader_mc_pick
                            .get_or_insert_with(|| releases[0].clone()),
                        r.clone(),
                        r,
                    );
                }
            });
        let mc = mc_selected;

        // Loader + build.
        ui.horizontal(|ui| {
            for l in updater::Loader::ALL {
                ui.selectable_value(&mut self.loader_pick, l, l.label());
            }
        });

        self.ensure_loader_builds(self.loader_pick, &mc);
        let key = (self.loader_pick, mc.clone());
        let builds_state = self.loader_builds.get(&key);
        match builds_state {
            None => {
                ui.weak(format!("Loading {} builds…", self.loader_pick.label()));
            }
            Some(None) => {
                ui.weak(format!(
                    "{} is not available for {mc}",
                    self.loader_pick.label()
                ));
            }
            Some(Some(Err(e))) => {
                ui.colored_label(egui::Color32::YELLOW, e.to_string());
            }
            Some(Some(Ok(builds))) => {
                egui::ComboBox::from_label(self.loader_pick.label())
                    .selected_text(
                        self.loader_selected
                            .as_deref()
                            .unwrap_or(&builds[0].version),
                    )
                    .show_ui(ui, |ui| {
                        for b in builds {
                            let text = if b.stable {
                                b.version.clone()
                            } else {
                                format!("{} (beta)", b.version)
                            };
                            ui.selectable_value(
                                self.loader_selected
                                    .get_or_insert_with(|| b.version.clone()),
                                b.version.clone(),
                                text,
                            );
                        }
                    });
                let chosen = self
                    .loader_selected
                    .clone()
                    .unwrap_or_else(|| builds[0].version.clone());
                if let Some(build) = builds.iter().find(|b| b.version == chosen) {
                    if ui
                        .add_enabled(
                            !installing && game_dir_ok,
                            egui::Button::new(format!(
                                "Install {} {} on {mc}",
                                self.loader_pick.label(),
                                build.version
                            )),
                        )
                        .clicked()
                    {
                        self.install_loader(mc.clone(), self.loader_pick, build.clone());
                    }
                }
            }
        }
    }

    pub(crate) fn install_version(&mut self, version: updater::ManifestVersion) {
        self.installing.store(true, Ordering::SeqCst);
        self.install_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let game_dir = self.active_game_dir();
        let progress = self.install_progress.clone();
        let installing = self.installing.clone();

        self.spawn_job(
            move || {
                let agent = crate::net::agent();
                let result = updater::install_version(&agent, &game_dir, &version, &mut |msg| {
                    *progress.lock().unwrap_or_else(|e| e.into_inner()) = msg.to_string();
                });
                installing.store(false, Ordering::SeqCst);
                result.map_err(|e| e.to_string())
            },
            |app, result| {
                match &result {
                    Ok(outcome) => {
                        app.settings.selected_version = outcome.version_id.clone();
                        app.save_settings();
                        app.log_console(format!(
                            "[RustLauncher] installed {} ({} new files)",
                            outcome.version_id, outcome.downloaded_files
                        ));
                    }
                    Err(e) => app.log_console(format!("[RustLauncher] install failed: {e}")),
                }
                app.reload_versions();
                app.play_status.clear();
            },
        );
    }

    pub(crate) fn ui_servers(&mut self, ui: &mut egui::Ui) {
        ui.heading("Servers");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.new_server_name);
            ui.label("Address");
            ui.text_edit_singleline(&mut self.new_server_addr);
            if ui.button("Add").clicked() {
                if self.new_server_addr.trim().is_empty() {
                    self.servers_dat_status = "Address is required".into();
                } else {
                    self.servers
                        .add(&self.new_server_name, &self.new_server_addr);
                    self.new_server_name.clear();
                    self.new_server_addr.clear();
                    self.save_servers();
                }
            }
        });
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for index in 0..self.servers.servers.len() {
                let entry = self.servers.servers[index].clone();
                ui.horizontal(|ui| {
                    ui.monospace(format!("{:>2}.", index + 1));
                    ui.label(&entry.name);
                    ui.weak(entry.display_ip());
                    if let Some(status) = self.server_status.get(&index) {
                        let color = if status == "online" {
                            egui::Color32::LIGHT_GREEN
                        } else {
                            egui::Color32::LIGHT_RED
                        };
                        ui.colored_label(color, status);
                    }
                    if ui.small_button("Check").clicked() {
                        let address = entry.display_ip();
                        self.spawn_job(
                            move || servers::check_status(&address).to_string(),
                            move |app, status| {
                                app.server_status.insert(index, status);
                            },
                        );
                    }
                    if ui.small_button("Delete").clicked() {
                        self.servers.remove(index);
                        self.server_status.clear();
                        self.save_servers();
                    }
                });
            }
            if self.servers.servers.is_empty() {
                ui.weak("No servers yet. Add one above or import servers.dat.");
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Save to servers.dat").clicked() {
                let path = servers::servers_dat_path(&self.active_game_dir());
                let list = self.servers.servers.clone();
                match servers::write_servers_dat(&path, &list) {
                    Ok(()) => {
                        self.servers_dat_status = format!("written to {}", path.display());
                    }
                    Err(e) => self.servers_dat_status = format!("failed: {e:#}"),
                }
            }
            if ui.button("Import from servers.dat").clicked() {
                let path = servers::servers_dat_path(&self.active_game_dir());
                let imported = servers::read_servers_dat(&path);
                if imported.is_empty() {
                    self.servers_dat_status = format!("nothing to import from {}", path.display());
                } else {
                    self.servers.servers = imported;
                    self.save_servers();
                    self.servers_dat_status = "imported".into();
                }
            }
            if !self.servers_dat_status.is_empty() {
                ui.label(&self.servers_dat_status);
            }
        });
    }

    /// The Accounts screen: three account kinds with their own creation and
    /// removal proofs — Offline (nickname + password, Argon2id in the DB),
    /// Ely.by (username + password against the Yggdrasil authserver) and
    /// Mojang/Microsoft (OAuth device-code flow).
    pub(crate) fn ui_accounts(&mut self, ui: &mut egui::Ui) {
        ui.heading("Accounts");
        ui.add_space(4.0);

        // ── Add account ──────────────────────────────────────────
        ui.group(|ui| {
            ui.strong("Add account");
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                for kind in auth::AccountKind::ALL {
                    ui.selectable_value(&mut self.new_account_kind, *kind, kind.label());
                }
            });
            match self.new_account_kind {
                AccountKind::Offline => {
                    ui.horizontal(|ui| {
                        ui.label("Nickname");
                        let name = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.username_input)
                                    .hint_text("3-16 chars")
                                    .desired_width(140.0),
                            )
                            .lost_focus();
                        ui.label("Password");
                        let pass = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.new_account_password)
                                    .password(true)
                                    .hint_text("required")
                                    .desired_width(140.0),
                            )
                            .lost_focus();
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.button("Create").clicked() || ((name || pass) && enter))
                            && !self.account_busy
                        {
                            let name = self.username_input.trim().to_string();
                            let password = self.new_account_password.clone();
                            self.account_busy = true;
                            self.spawn_job(
                                move || {
                                    // Argon2id hashing is intentionally slow;
                                    // keep it off the UI thread. The insert goes
                                    // through the DB so any store can do it; the
                                    // live in-memory store is refreshed afterwards.
                                    let mut store = AccountStore::load(&LauncherPaths::probe());
                                    store.add_offline(&name, &password)
                                },
                                move |app, result| {
                                    app.account_busy = false;
                                    app.finish_account_change(result);
                                },
                            );
                        }
                    });
                    ui.weak(
                        "The password is stored as an Argon2id hash in the launcher's database.",
                    );
                }
                AccountKind::ElyBy => {
                    ui.horizontal(|ui| {
                        ui.label("Ely.by email / login");
                        let user = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.online_login_input)
                                    .hint_text("you@example.com")
                                    .desired_width(180.0),
                            )
                            .lost_focus();
                        ui.label("Password");
                        let pass = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.online_password_input)
                                    .password(true)
                                    .desired_width(140.0),
                            )
                            .lost_focus();
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.button("Log in").clicked() || ((user || pass) && enter))
                            && !self.account_busy
                        {
                            let user = self.online_login_input.trim().to_string();
                            let password = self.online_password_input.clone();
                            self.account_busy = true;
                            self.spawn_job(
                                move || auth::login_elyby(&user, &password),
                                move |app, result| {
                                    app.account_busy = false;
                                    app.finish_online_login(result);
                                },
                            );
                        }
                    });
                    ui.weak(
                        "Logs in against authserver.ely.by; the session token is stored in the DB.",
                    );
                }
                AccountKind::Mojang => {
                    ui.label("Sign in with a Microsoft account that owns Minecraft: Java Edition.");
                    if self.ms_login.is_none()
                        && !self.account_busy
                        && ui.button("Start Microsoft sign-in").clicked()
                    {
                        self.account_busy = true;
                        self.spawn_job(
                            move || {
                                auth::microsoft_begin(&crate::net::agent())
                                    .map_err(|e| e.to_string())
                            },
                            move |app, result| match result {
                                Ok((device_code, user_code, _expires, _interval)) => {
                                    app.account_busy = false;
                                    app.ms_login = Some(MsLoginState {
                                        device_code,
                                        user_code: user_code.clone(),
                                        error: None,
                                        cancelled: false,
                                    });
                                    app.notify_info(format!(
                                        "Enter the code {user_code} at microsoft.com/link"
                                    ));
                                }
                                Err(e) => {
                                    app.account_busy = false;
                                    app.notify_error("MS-BEGIN", e);
                                }
                            },
                        );
                    }
                    let ms_state = self.ms_login.take();
                    if let Some(mut state) = ms_state {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("Go to");
                                ui.hyperlink("https://www.microsoft.com/link");
                                ui.add_space(8.0);
                                ui.label("and enter code:");
                                ui.label(
                                    egui::RichText::new(&state.user_code)
                                        .strong()
                                        .monospace()
                                        .size(18.0),
                                );
                                if ui.button("Copy").clicked() {
                                    ui.ctx().copy_text(state.user_code.clone());
                                }
                            });
                            ui.weak("Waiting for you to finish in the browser…");
                            if let Some(err) = &state.error {
                                ui.colored_label(egui::Color32::LIGHT_RED, err);
                            }
                            if ui.button("Cancel").clicked() {
                                state.cancelled = true;
                            }
                        });

                        if state.cancelled {
                            self.ms_login = None;
                        } else {
                            // Put the state back; the poll job below updates it.
                            self.ms_login = Some(state);

                            // Poll in the background; the result arrives via a job.
                            if !self.account_busy && !self.ms_polling {
                                let device_code = self
                                    .ms_login
                                    .as_ref()
                                    .map(|s| s.device_code.clone())
                                    .unwrap_or_default();
                                self.ms_polling = true;
                                self.spawn_job(
                                    move || {
                                        auth::microsoft_poll(&crate::net::agent(), &device_code)
                                            .map_err(|e| e.to_string())
                                    },
                                    move |app, result| {
                                        app.ms_polling = false;
                                        match result {
                                            Ok(Some(login)) => {
                                                app.ms_login = None;
                                                app.ms_polling = false;
                                                // Add to the live store directly: the
                                                // HTTP work is done, the insert is a
                                                // fast local SQLite write.
                                                let result = app.accounts.add_mojang(
                                                    &login.username,
                                                    &login.uuid,
                                                    &login.access_token,
                                                    &login.refresh_token,
                                                );
                                                app.finish_account_change(result);
                                            }
                                            Ok(None) => {} // still waiting; poll again next frames
                                            Err(e) => {
                                                if let Some(state) = &mut app.ms_login {
                                                    state.error = Some(e);
                                                }
                                            }
                                        }
                                    },
                                );
                            }
                        }
                    }
                }
            }
            if self.account_busy {
                ui.spinner();
                ui.weak("working…");
            }
            if let Some(error) = &self.account_error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        });

        ui.add_space(6.0);

        // ── Account list ─────────────────────────────────────────
        ui.strong("Your accounts");
        ui.add_space(2.0);
        let rows: Vec<(AccountKind, String)> = self
            .accounts
            .accounts
            .iter()
            .map(|a| (a.kind, a.username.clone()))
            .collect();
        for (kind, name) in &rows {
            ui.horizontal(|ui| {
                let selected = self.accounts.current.as_deref() == Some(name.as_str());
                if ui.radio(selected, name).clicked() {
                    self.accounts.select(name);
                    self.save_accounts();
                }
                ui.weak(kind.label());
                if ui.small_button("Remove").clicked() {
                    // Removal always asks for proof: the password for offline
                    // accounts, a fresh login for online ones.
                    self.account_remove_pending = Some(name.clone());
                    self.account_remove_password.clear();
                    self.account_remove_error = None;
                }
            });
        }
        if rows.is_empty() {
            ui.weak("No accounts yet — create one above.");
        }
    }

    /// Common tail of every successful account mutation: refresh selection
    /// state, clear inputs, drop the error.
    pub(crate) fn finish_account_change(&mut self, result: Result<String>) {
        match result {
            Ok(name) => {
                self.username_input.clear();
                self.new_account_password.clear();
                self.account_error = None;
                // Background jobs add the account to a throwaway store loaded
                // from the DB; reload the in-memory copy so the new account
                // actually shows up in the list and can be selected.
                self.accounts = AccountStore::load(&self.home_dir);
                self.accounts.select(&name);
                self.save_accounts();
                self.notify_info(format!("Account {name} added"));
            }
            Err(e) => self.account_error = Some(e.to_string()),
        }
    }

    /// Store an online login result (Ely.by or Microsoft) as an account.
    /// Runs on the UI thread: the HTTP work already happened in the job; the
    /// store write is a fast local SQLite insert.
    pub(crate) fn finish_online_login(&mut self, result: Result<auth::Account>) {
        match result {
            Ok(account) => {
                self.online_password_input.clear();
                // Mutate the live store directly so the new account is in the
                // list immediately (and persisted by the store itself).
                let result = match account.kind {
                    AccountKind::ElyBy => self.accounts.add_elyby(
                        &account.username,
                        &account.uuid,
                        &account.access_token,
                    ),
                    AccountKind::Mojang => self.accounts.add_mojang(
                        &account.username,
                        &account.uuid,
                        &account.access_token,
                        "",
                    ),
                    AccountKind::Offline => unreachable!(),
                };
                self.finish_account_change(result);
            }
            Err(e) => {
                self.account_error = Some(e.to_string());
            }
        }
    }

    /// The removal dialog: password proof for offline accounts, re-login for
    /// online ones. Mirrors the Stop/Kill confirmation styling.
    pub(crate) fn show_account_removal(&mut self, ctx: &egui::Context) {
        let Some(name) = self.account_remove_pending.clone() else {
            return;
        };
        let rec = self.accounts.accounts.iter().find(|a| a.username == name);
        let kind = rec.map(|r| r.kind);
        let screen = ctx.screen_rect();

        egui::Area::new(egui::Id::new("account_remove_dim"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.left_top())
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
                if resp.clicked() {
                    self.account_remove_pending = None;
                }
            });

        egui::Window::new(egui::RichText::new("Confirm removal").strong())
            .id(egui::Id::new("account_remove_dialog"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            // Hard clamp: min = max = default, so the dialog physically
            // cannot stretch regardless of what egui remembers or measures.
            .fixed_size(egui::vec2(400.0, 230.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    draw_warning_triangle(ui, 36.0);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("Remove {name}?"))
                                    .strong()
                                    .size(16.0),
                            )
                            .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                        match kind {
                            Some(AccountKind::Offline) => {
                                ui.label("Enter the account password to confirm removal.");
                            }
                            Some(AccountKind::ElyBy) => {
                                ui.label("Sign in to Ely.by again to confirm removal.");
                            }
                            Some(AccountKind::Mojang) => {
                                ui.label("Sign in with Microsoft again to confirm removal.");
                            }
                            None => {}
                        }
                    });
                });
                ui.add_space(10.0);

                let confirmed = match kind {
                    Some(AccountKind::Offline) => {
                        ui.horizontal(|ui| {
                            ui.label("Password:");
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.account_remove_password)
                                    .password(true)
                                    .hint_text("type the account password")
                                    .desired_width(240.0)
                                    .id(egui::Id::new("account_remove_password")),
                            );
                            // Auto-focus the password field while nothing else
                            // in the dialog holds focus, so it is obvious where
                            // to type right after the dialog opens.
                            if ui.memory(|m| m.focused().is_none()) {
                                ui.memory_mut(|m| {
                                    m.request_focus(egui::Id::new("account_remove_password"))
                                });
                            }
                            let enter =
                                resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            enter
                                && self
                                    .accounts
                                    .verify_offline_password(&name, self.account_remove_password.trim())
                        });
                if let Some(err) = &self.account_remove_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
                }
                let ok = !self.account_remove_password.trim().is_empty()
                    && self
                        .accounts
                        .verify_offline_password(&name, self.account_remove_password.trim());
                let mut clicked = false;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    clicked = ui
                        .add_enabled(
                            ok,
                            egui::Button::new(egui::RichText::new("Confirm").strong()),
                        )
                        .clicked();
                });
                ok && clicked
            }
            Some(AccountKind::ElyBy) => {
                ui.horizontal(|ui| {
                    ui.label("Ely.by login:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.account_remove_login)
                            .desired_width(220.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Password:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.account_remove_password)
                            .password(true)
                            .desired_width(220.0),
                    );
                });
                if let Some(err) = &self.account_remove_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
                }
                let ready = !self.account_remove_login.trim().is_empty()
                    && !self.account_remove_password.is_empty();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(egui::RichText::new("Confirm").strong()),
                        )
                        .clicked()
                    {
                        self.account_remove_error = None;
                        let user = self.account_remove_login.trim().to_string();
                        let password = self.account_remove_password.clone();
                        let name = name.clone();
                        self.spawn_job(
                            move || auth::login_elyby(&user, &password),
                            move |app, res| match res {
                                Ok(acc) => {
                                    if acc.username == name {
                                        app.remove_account_confirmed(&name);
                                    } else {
                                        app.account_remove_error = Some(
                                            "that login belongs to another account".into(),
                                        );
                                    }
                                }
                                Err(e) => app.account_remove_error = Some(e.to_string()),
                            },
                        );
                    }
                });
                false
            }
            Some(AccountKind::Mojang) => {
                ui.label("A Microsoft sign-in window will open. Complete it to remove this account.");
                if ui.button("Start Microsoft sign-in").clicked() {
                    let name = name.clone();
                    self.account_busy = true;
                    self.spawn_job(
                        move || {
                            auth::microsoft_begin(&crate::net::agent())
                                .map_err(|e| e.to_string())
                        },
                        move |app, result| match result {
                            Ok((device_code, user_code, _, _)) => {
                                app.account_busy = false;
                                app.account_remove_pending = Some(name.clone());
                                app.ms_removal = Some(MsLoginState {
                                    device_code,
                                    user_code: user_code.clone(),
                                    error: None,
                                    cancelled: false,
                                });
                                app.notify_info(format!(
                                    "Enter the code {user_code} at microsoft.com/link"
                                ));
                            }
                            Err(e) => {
                                app.account_busy = false;
                                app.account_remove_error = Some(e);
                            }
                        },
                    );
                }
                if let Some(err) = &self.account_remove_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
                }
                false
            }
            None => false,
        };
        if confirmed {
            self.remove_account_confirmed(&name);
        }

        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                self.account_remove_pending = None;
                self.ms_removal = None;
            }
        });
    });

        // The Microsoft re-login for removal shares the device-code poller.
        if self.ms_removal.is_some() && !self.account_busy && !self.ms_polling {
            let device_code = self
                .ms_removal
                .as_ref()
                .map(|s| s.device_code.clone())
                .unwrap_or_default();
            let name = name.clone();
            self.ms_polling = true;
            self.spawn_job(
                move || {
                    auth::microsoft_poll(&crate::net::agent(), &device_code)
                        .map_err(|e| e.to_string())
                },
                move |app, result| {
                    app.ms_polling = false;
                    match result {
                        Ok(Some(login)) => {
                            if login.username == name {
                                app.ms_removal = None;
                                app.remove_account_confirmed(&name);
                            } else {
                                if let Some(state) = &mut app.ms_removal {
                                    state.error =
                                        Some("that Microsoft account is not this account".into());
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            if let Some(state) = &mut app.ms_removal {
                                state.error = Some(e);
                            }
                        }
                    }
                },
            );
        }
    }

    /// Remove the account (proof already collected). Falls back to the first
    /// remaining account when the removed one was selected.
    pub(crate) fn remove_account_confirmed(&mut self, name: &str) {
        let _ = self.accounts.remove(name);
        // Re-sync with the DB so the in-memory list matches what is stored.
        self.accounts = AccountStore::load(&self.home_dir);
        self.account_remove_pending = None;
        self.account_remove_password.clear();
        self.account_remove_login.clear();
        self.account_remove_error = None;
        self.save_accounts();
        self.notify_info(format!("Account {name} removed"));
    }

    pub(crate) fn ui_skins(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Skins");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let mut username = self
                .accounts
                .current()
                .map(|a| a.username.clone())
                .unwrap_or_default();
            ui.label("Download for");
            ui.add_enabled(
                false,
                egui::TextEdit::singleline(&mut username).desired_width(120.0),
            );
            if ui.button("Download skin").clicked() && !username.is_empty() {
                let dir = home::skins_dir(&self.home_dir);
                let name = username.clone();
                let name2 = username.clone();
                self.skin_status = "Downloading…".into();
                self.spawn_job(
                    move || {
                        skins::download_skin(&crate::net::agent(), &dir, &name)
                            .map_err(|e| e.to_string())
                    },
                    |app, result| match result {
                        Ok(path) => {
                            app.skin_status = format!("saved {}", path.display());
                            app.refresh_skins();
                            app.selected_skin = Some(name2);
                        }
                        Err(e) => app.skin_status = e,
                    },
                );
            }
            if ui.button("Import PNG…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PNG skin", &["png"])
                    .pick_file()
                {
                    let dir = home::skins_dir(&self.home_dir);
                    match skins::import_skin(&dir, &path) {
                        Ok(name) => {
                            self.refresh_skins();
                            self.selected_skin = Some(name);
                            self.skin_status.clear();
                        }
                        Err(e) => self.skin_status = e.to_string(),
                    }
                }
            }
        });
        if !self.skin_status.is_empty() {
            ui.label(&self.skin_status);
        }
        ui.separator();

        ui.horizontal(|ui| {
            egui::ScrollArea::horizontal().show(ui, |ui| {
                for name in self.skin_names.clone() {
                    ui.selectable_value(&mut self.selected_skin, Some(name.clone()), &name);
                }
            });
        });

        if let (Some(name), ctx) = (self.selected_skin.clone(), ctx) {
            let path = home::skins_dir(&self.home_dir).join(format!("{name}.png"));
            let needs_load = match &self.skin_texture {
                Some((cached, _)) => cached != &name,
                None => true,
            };
            if needs_load && path.is_file() {
                if let Err(e) = skins::load_skin(&path).map(|skin| {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [skin.width as usize, skin.height as usize],
                        &skin.rgba,
                    );
                    let texture = ctx.load_texture("skin", image, egui::TextureOptions::NEAREST);
                    self.skin_texture = Some((name.clone(), texture));
                }) {
                    ui.colored_label(egui::Color32::LIGHT_RED, e.to_string());
                }
            }
            if let Some((_, texture)) = &self.skin_texture {
                ui.add_space(8.0);
                ui.label(format!("Skin: {name}"));
                let size = if texture.size()[1] == 32 {
                    egui::vec2(256.0, 128.0)
                } else {
                    egui::vec2(256.0, 256.0)
                };
                ui.add(egui::Image::new(texture).fit_to_exact_size(size));
            }
        } else {
            ui.weak("No skin selected. Download one or import a 64x32/64x64 PNG.");
        }
    }
}
