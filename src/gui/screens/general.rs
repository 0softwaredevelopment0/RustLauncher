//! The General screen (launch, stop, kill) and the Console screen
//! (combined launcher/game log with a stdin command line).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::gui::state::{App, TerminateKind};
use crate::gui::theme::icons;
use crate::lang::tr;
use crate::services::settings;

impl App {
    pub(crate) fn ui_general(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        ui.heading(tr(lang, "General"));
        ui.add_space(6.0);

        egui::Grid::new("play_grid").num_columns(2).show(ui, |ui| {
            ui.label(tr(lang, "Instance"));
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
                    tr(lang, "— none —").to_string()
                } else {
                    self.launch_instance.clone()
                })
                .show_ui(ui, |ui| {
                    for name in &instance_names {
                        ui.selectable_value(&mut self.launch_instance, name.clone(), name.clone());
                    }
                });
            ui.end_row();

            ui.label(tr(lang, "Account"));
            let current = self
                .accounts
                .current()
                .map(|a| a.username.clone())
                .unwrap_or_default();
            ui.label(if current.is_empty() {
                tr(lang, "— none —").to_string()
            } else {
                current
            });
            ui.end_row();

            ui.label(tr(lang, "Version"));
            let names: Vec<String> = self.versions.iter().map(|v| v.name.clone()).collect();
            egui::ComboBox::from_id_salt("version_combo")
                .selected_text(if self.settings.selected_version.is_empty() {
                    tr(lang, "— select —").to_string()
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
                ui.label(tr(lang, "Auto-connect"));
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
                    egui::Button::new(format!("{}  {}", icons::PLAY_ARROW, tr(lang, "Launch"))),
                )
                .clicked()
            {
                self.start_game();
            }
            if ui.button(tr(lang, "Rescan versions")).clicked() {
                self.reload_versions();
            }
            let game_running = self.any_game_running();
            let stop = egui::Button::new(format!("{} {}", icons::STOP, tr(lang, "Stop")));
            if ui
                .add_enabled(game_running, stop)
                .on_disabled_hover_text(tr(lang, "The game is not running"))
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
                egui::RichText::new(format!("{} {}", icons::KILL, tr(lang, "Kill")))
                    .color(egui::Color32::LIGHT_RED),
            );
            if ui
                .add_enabled(game_running, kill)
                .on_disabled_hover_text(tr(lang, "The game is not running"))
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
        let lang = self.settings.language;
        ui.horizontal(|ui| {
            ui.heading(tr(lang, "Console"));
            // Follow one of the running instances. Only active instances are
            // listed; with none running the picker is not shown at all.
            // Every instance keeps filling its own console buffer even while
            // it is not the one being displayed, so nothing is lost.
            let alive: Vec<(usize, String)> = self
                .running_games
                .iter()
                .enumerate()
                .filter(|(_, g)| g.running.load(Ordering::SeqCst))
                .map(|(idx, g)| (idx, g.instance.clone()))
                .collect();
            if !alive.is_empty() {
                let followed = self
                    .running_games
                    .iter()
                    .find(|g| Arc::ptr_eq(&g.console, &self.console))
                    .map(|g| g.instance.clone())
                    .unwrap_or_else(|| "…".to_string());
                ui.label(tr(lang, "Following:"));
                egui::ComboBox::from_id_salt("console_instance")
                    .selected_text(followed)
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        for (idx, name) in &alive {
                            let selected = self
                                .running_games
                                .get(*idx)
                                .map(|g| Arc::ptr_eq(&g.console, &self.console))
                                .unwrap_or(false);
                            if ui.selectable_label(selected, name.clone()).clicked() {
                                if let Some(g) = self.running_games.get(*idx) {
                                    self.console = g.console.clone();
                                    self.console_seq = 0;
                                }
                            }
                        }
                    });
            }
            // What the console logs: launcher's own lines and game output
            // alike, applied at capture time (a rejected line is never
            // stored). Log FILES are configured in Settings.
            ui.label(tr(lang, "Log to console:"));
            egui::ComboBox::from_id_salt("console_mode")
                .selected_text(self.settings.console_log_mode.label(lang))
                .width(150.0)
                .show_ui(ui, |ui| {
                    for mode in settings::ConsoleMode::ALL {
                        ui.selectable_value(
                            &mut self.settings.console_log_mode,
                            mode,
                            mode.label(lang),
                        );
                    }
                });
            if ui.button(tr(lang, "Export to file…")).clicked() {
                self.export_console();
            }
            if ui.button(tr(lang, "Clear")).clicked() {
                self.console
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clear();
            }
            if ui.button(tr(lang, "Copy all")).clicked() {
                let text = self
                    .console
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .lines()
                    .join("\n");
                ui.output_mut(|o| o.copied_text = text);
            }
        });
        ui.separator();

        // Command line at the bottom: sends a chat line / command to the
        // followed game's stdin. Reserved before the log area so the log
        // scrolls in the space above it.
        egui::TopBottomPanel::bottom("console_input_bar").show_inside(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let active = self.console_follows_running();
                ui.label(tr(lang, "Command:"));
                // Leave room for the Send button on the same row.
                let field_w = (ui.available_width() - 80.0).max(120.0);
                let resp = ui.add_enabled(
                    active,
                    egui::TextEdit::singleline(&mut self.console_input)
                        .hint_text(tr(lang, "Type a command or chat message…"))
                        .desired_width(field_w),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let send = ui
                    .add_enabled(active, egui::Button::new(tr(lang, "Send")))
                    .clicked();
                if active && (enter || send) {
                    let line = self.console_input.trim().to_string();
                    if !line.is_empty() {
                        self.console_input.clear();
                        self.send_console_line(&line);
                    }
                    resp.request_focus();
                }
            });
            ui.add_space(2.0);
        });

        let all_lines: Vec<String> = {
            let buf = self.console.lock().unwrap_or_else(|e| e.into_inner());
            buf.lines().to_vec()
        };
        let changed = all_lines.len() != self.console_seq;
        self.console_seq = all_lines.len();

        // Everything in the buffer already passed the logging mode at
        // capture time — show it as is.
        let lines: Vec<&String> = all_lines.iter().collect();

        egui::ScrollArea::vertical()
            .stick_to_bottom(changed)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    ui.set_min_width(ui.available_width());
                    // Long lines wrap onto the next line instead of running
                    // off the right edge.
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    ui.style_mut()
                        .text_styles
                        .insert(egui::TextStyle::Body, egui::FontId::monospace(12.0));
                    for line in &lines {
                        ui.add(egui::Label::new(line.as_str()).wrap());
                    }
                });
            });
    }
}
