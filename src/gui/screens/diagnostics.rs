//! The Diagnostics screen: system/network checks streamed line by line
//! from background tasks, with copy/save of the finished report.

use crate::gui::state::App;
use crate::gui::theme::icons;
use crate::lang::{tr, tr_fmt};
use crate::services::diagnostics;

impl App {
    pub(crate) fn ui_diagnostics(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        ui.heading(tr(lang, "Network diagnostics"));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.diag_running, egui::Button::new(tr(lang, "Run tests")))
                .clicked()
            {
                self.diag_running = true;
                let lang = self.settings.language;
                let input = diagnostics::DiagInput {
                    game_directory: self.settings.game_directory.clone(),
                    news_url: self.settings.news_url.clone(),
                    java_path: self.settings.java_mode.path().map(str::to_string),
                    home_dir: Some(self.home_dir.clone()),
                };
                // Stream results: every check runs in its own background
                // thread and appends its row to the list as soon as it
                // finishes, so the user sees progress line by line.
                let tasks = diagnostics::suite(crate::util::net::agent(), lang, input);
                self.diag_results.clear();
                self.diag_pending = tasks.iter().map(|t| t.name.clone()).collect();
                for task in tasks {
                    let name = task.name.clone();
                    self.spawn_job(
                        move || (task.run)(),
                        move |app, result: diagnostics::CheckResult| {
                            app.diag_results.push(result);
                            if let Some(pos) = app.diag_pending.iter().position(|n| *n == name) {
                                app.diag_pending.remove(pos);
                            }
                            // The run is over when no check is pending any
                            // more — re-enable the Run tests button.
                            if app.diag_pending.is_empty() {
                                app.diag_running = false;
                            }
                        },
                    );
                }
            }
            if self.diag_running {
                ui.label(tr(lang, "Testing…"));
            }
        });
        ui.separator();
        if self.diag_results.is_empty() && self.diag_pending.is_empty() && !self.diag_running {
            ui.weak(tr(
                lang,
                "Press Run tests to check system, Java, DNS, HTTP and TCP connectivity.",
            ));
        } else {
            let passed = self.diag_results.iter().filter(|r| r.ok).count();
            let all_ok = passed == self.diag_results.len();
            let finished = self.diag_pending.is_empty();
            let mut save_clicked = false;
            ui.horizontal(|ui| {
                ui.colored_label(
                    if all_ok {
                        egui::Color32::LIGHT_GREEN
                    } else {
                        egui::Color32::LIGHT_RED
                    },
                    tr_fmt(
                        lang,
                        "{0}/{1} passed",
                        &[&passed.to_string(), &self.diag_results.len().to_string()],
                    ),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Copy / save are only offered once every planned check
                    // has finished, so the report is never partial.
                    let save = ui.add_enabled(
                        finished,
                        egui::Button::new(format!(
                            "{} {}",
                            icons::FILE_DOWNLOAD,
                            tr(lang, "Save to file")
                        )),
                    );
                    if save.clicked() {
                        ui.ctx().request_repaint();
                        save_clicked = true;
                    }
                    let copy = ui.add_enabled(
                        finished,
                        egui::Button::new(format!("{} {}", icons::SAVE, tr(lang, "Copy report"))),
                    );
                    if copy.clicked() {
                        ui.ctx()
                            .copy_text(diagnostics::report_text(&self.diag_results));
                    }
                    if !finished {
                        copy.on_disabled_hover_text(tr(lang, "Waiting for all tests to finish…"));
                        save.on_disabled_hover_text(tr(lang, "Waiting for all tests to finish…"));
                    }
                });
            });
            if save_clicked {
                self.export_diagnostics();
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for result in &self.diag_results {
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
                    for name in &self.diag_pending {
                        ui.horizontal(|ui| {
                            ui.weak("…   ");
                            ui.monospace(name);
                            ui.weak(tr(lang, "running…"));
                        });
                    }
                });
        }
    }
}
