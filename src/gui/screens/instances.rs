//! The Instances screen: create instances (optionally importing a Modrinth
//! modpack), the running-games list with per-entry Stop/Kill, and the
//! instance list with delete and terminate confirmation dialogs.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::gui::state::{App, Screen, TerminateKind};
use crate::gui::toast_ui::draw_warning_triangle;
use crate::gui::theme::icons;
use crate::lang::{tr, tr_fmt};

impl App {
    pub(crate) fn ui_instances(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        ui.heading(format!(
            "{}  {}",
            icons::VIDEOGAME_ASSET,
            tr(lang, "Instances")
        ));
        ui.add_space(6.0);

        // ── Create section ──
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("{}  {}", icons::ADD, tr(lang, "New instance"))).strong(),
        )
        .default_open(self.instance_store.instances.is_empty())
        .show(ui, |ui| {
            egui::Grid::new("new_instance_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label(tr(lang, "Name"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_instance_name)
                            .hint_text(tr(lang, "My instance"))
                            .desired_width(260.0),
                    );
                    ui.end_row();

                    ui.label(tr(lang, "Game directory"));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_instance_dir)
                                .hint_text("C:\\Games\\MyPack")
                                .desired_width(260.0),
                        );
                        if ui
                            .button(format!("{} {}", icons::FOLDER, tr(lang, "Browse…")))
                            .clicked()
                        {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                self.new_instance_dir = dir.to_string_lossy().to_string();
                            }
                        }
                    });
                    ui.end_row();
                });

            // ── Modpack import mode ──
            ui.add_space(4.0);
            ui.checkbox(
                &mut self.new_instance_modpack,
                format!(
                    "{}  {}",
                    icons::INVENTORY_2,
                    tr(lang, "Install modpack (.mrpack)")
                ),
            )
            .on_hover_text(tr(lang, "Install a Modrinth modpack (.mrpack) into this instance"));
            if self.new_instance_modpack {
                egui::Grid::new("modpack_file_grid")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label(tr(lang, "Modpack file"));
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_instance_pack_file)
                                    .hint_text(tr(lang, "Select a .mrpack archive…"))
                                    .desired_width(260.0),
                            );
                            if ui
                                .button(format!("{} {}", icons::FOLDER, tr(lang, "Browse…")))
                                .clicked()
                            {
                                self.browse_modpack_file();
                            }
                        });
                        ui.end_row();
                    });
                if let Some(summary) = &self.new_instance_pack_summary {
                    ui.weak(summary);
                }
            }

            if ui
                .add_enabled(
                    !self.modpack_installing,
                    egui::Button::new(format!(
                        "{}  {}",
                        icons::CHECK_CIRCLE,
                        tr(lang, "Create instance")
                    )),
                )
                .clicked()
            {
                self.instances_error = None;
                if self.new_instance_modpack {
                    self.start_modpack_install();
                } else {
                    match self.instance_store.create(
                        &self.new_instance_name,
                        &self.new_instance_dir,
                        lang,
                    ) {
                        Ok(name) => {
                            if let Err(e) = self.instance_store.save(&self.home_dir, lang) {
                                self.instances_error = Some(format!("{e:#}"));
                                self.notify_error("INSTANCES", format!("{e:#}"));
                            } else {
                                self.notify_info(tr_fmt(lang, "Instance '{0}' created", &[&name]));
                                self.new_instance_name.clear();
                                self.new_instance_dir.clear();
                            }
                        }
                        Err(e) => {
                            self.instances_error = Some(format!("{e:#}"));
                        }
                    }
                }
            }

            // Live progress of a running modpack import.
            if self.modpack_installing {
                let progress = self
                    .modpack_progress
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                crate::gui::state::ui_progress_line(ui, &progress, lang);
            }
        });

        ui.add_space(4.0);
        ui.separator();

        // ── Running games ──
        let running_count = self.running_games.len();
        if running_count > 0 {
            egui::CollapsingHeader::new(
                egui::RichText::new(format!(
                    "{}  {}",
                    icons::PLAY_ARROW,
                    tr_fmt(
                        lang,
                        "Running games ({0})",
                        &[&self
                            .running_games
                            .iter()
                            .filter(|g| g.running.load(Ordering::SeqCst))
                            .count()
                            .to_string()]
                    )
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
                            ui.weak(tr(lang, "(running)"));
                        } else if let Some(g) = self.running_games.get(idx) {
                            let s = g.status.lock().unwrap_or_else(|e| e.into_inner()).clone();
                            if !s.is_empty() {
                                ui.weak(format!("({s})"));
                            }
                        }

                        if ui
                            .add_enabled(
                                alive,
                                egui::Button::new(format!("{} {}", icons::STOP, tr(lang, "Stop"))),
                            )
                            .on_disabled_hover_text(tr(lang, "Not running"))
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
                                    egui::RichText::new(format!(
                                        "{} {}",
                                        icons::KILL,
                                        tr(lang, "Kill")
                                    ))
                                    .color(egui::Color32::LIGHT_RED),
                                ),
                            )
                            .on_disabled_hover_text(tr(lang, "Not running"))
                            .clicked()
                        {
                            let pid = self.running_games[idx].pid.clone();
                            if self.settings.confirm_kill {
                                self.instance_terminate_pending = Some((pid, TerminateKind::Kill));
                            } else {
                                self.kill_instance_pid(idx);
                            }
                        }
                        if ui.button(tr(lang, "Console")).clicked() {
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
                            ui.colored_label(
                                egui::Color32::LIGHT_RED,
                                tr_fmt(lang, "Error: {0}", &[&err]),
                            );
                        }
                    }
                }
                ui.add_space(4.0);
                // Clean up finished entries on demand.
                if ui.button(tr(lang, "Clear finished")).clicked() {
                    self.running_games
                        .retain(|g| g.running.load(Ordering::SeqCst));
                }
            });
            ui.separator();
        }

        // ── Instance list ──
        if self.instance_store.instances.is_empty() {
            ui.add_space(6.0);
            ui.weak(tr(
                lang,
                "No instances yet — create one above to start playing.",
            ));
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
                        if game.is_some() {
                            format!("  {} {}", icons::FIBER_MANUAL_RECORD, tr(lang, "running"))
                        } else {
                            String::new()
                        }
                    ))
                    .strong(),
                )
                .id_salt(("instance", name.as_str()))
                .show(ui, |ui| {
                    ui.label(tr_fmt(lang, "Directory: {0}", &[&instance.game_dir]));

                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                game.is_none(),
                                egui::Button::new(format!(
                                    "{}  {}",
                                    icons::PLAY_ARROW,
                                    tr(lang, "Launch")
                                )),
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
                                egui::Button::new(format!("{} {}", icons::STOP, tr(lang, "Stop"))),
                            )
                            .on_disabled_hover_text(tr(lang, "Not running"))
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
                                    egui::RichText::new(format!(
                                        "{} {}",
                                        icons::KILL,
                                        tr(lang, "Kill")
                                    ))
                                    .color(egui::Color32::LIGHT_RED),
                                ),
                            )
                            .on_disabled_hover_text(tr(lang, "Not running"))
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
                                ui.colored_label(
                                    egui::Color32::LIGHT_RED,
                                    tr_fmt(lang, "Error: {0}", &[&err]),
                                );
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
                                    egui::RichText::new(format!(
                                        "{}  {}",
                                        icons::DELETE,
                                        tr(lang, "Delete")
                                    ))
                                    .color(egui::Color32::LIGHT_RED),
                                ),
                            )
                            .on_disabled_hover_text(tr(lang, "Stop the game before deleting"))
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
        let lang = self.settings.language;

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

        egui::Window::new(egui::RichText::new(tr(lang, "Confirm")).strong())
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
                            egui::RichText::new(tr(lang, "Delete this instance?"))
                                .strong()
                                .size(16.0),
                        );
                        ui.label(tr_fmt(
                            lang,
                            "'{0}' will be removed from the launcher. The game directory on \
                             disk will NOT be deleted.",
                            &[&name],
                        ));
                    });
                });
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(tr(lang, "Confirm")).strong(),
                        ))
                        .clicked()
                    {
                        self.instance_delete_pending = None;
                        if self.instance_store.delete(&name) {
                            if let Err(e) = self.instance_store.save(&self.home_dir, lang) {
                                self.instances_error = Some(format!("{e:#}"));
                                self.notify_error("INSTANCES", format!("{e:#}"));
                            } else {
                                self.notify_info(tr_fmt(lang, "Instance '{0}' deleted", &[&name]));
                                if self.launch_instance == name {
                                    self.launch_instance.clear();
                                }
                            }
                        }
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
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
        let lang = self.settings.language;
        let (title, body) = match kind {
            TerminateKind::Stop => (
                tr_fmt(lang, "Stop {0}?", &[&name]),
                tr(
                    lang,
                    "The game will be asked to close. It usually exits within a few seconds, but unsaved progress may be lost.",
                )
                .to_string(),
            ),
            TerminateKind::Kill => (
                tr_fmt(lang, "Force kill {0}?", &[&name]),
                tr(
                    lang,
                    "The game process tree will be terminated immediately. Unsaved progress will be lost.",
                )
                .to_string(),
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
        egui::Window::new(egui::RichText::new(tr(lang, "Confirm")).strong())
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
                ui.checkbox(&mut self.terminate_dont_ask, tr(lang, "Don't ask again"));
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(tr(lang, "Confirm")).strong(),
                        ))
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
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        self.instance_terminate_pending = None;
                    }
                });
            });
    }
}
