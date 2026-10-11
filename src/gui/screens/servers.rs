//! The Servers screen: the launcher server list with ping checks and
//! import/export to Minecraft's `servers.dat`.

use crate::gui::state::App;
use crate::lang::{tr, tr_fmt};
use crate::services::servers;

impl App {
    pub(crate) fn ui_servers(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        ui.heading(tr(lang, "Servers"));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(tr(lang, "Name"));
            ui.text_edit_singleline(&mut self.new_server_name);
            ui.label(tr(lang, "Address"));
            ui.text_edit_singleline(&mut self.new_server_addr);
            if ui.button(tr(lang, "Add")).clicked() {
                if self.new_server_addr.trim().is_empty() {
                    self.servers_dat_status = tr(lang, "Address is required").into();
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
                        let color = if *status == servers::ServerStatus::Online {
                            egui::Color32::LIGHT_GREEN
                        } else {
                            egui::Color32::LIGHT_RED
                        };
                        ui.colored_label(color, status.display(lang));
                    }
                    if ui.small_button(tr(lang, "Check")).clicked() {
                        let address = entry.display_ip();
                        self.spawn_job(
                            move || servers::check_status(&address, lang),
                            move |app, status| {
                                app.server_status.insert(index, status);
                            },
                        );
                    }
                    if ui.small_button(tr(lang, "Delete")).clicked() {
                        self.servers.remove(index);
                        self.server_status.clear();
                        self.save_servers();
                    }
                });
            }
            if self.servers.servers.is_empty() {
                ui.weak(tr(
                    lang,
                    "No servers yet. Add one above or import servers.dat.",
                ));
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(tr(lang, "Save to servers.dat")).clicked() {
                let path = servers::servers_dat_path(&self.active_game_dir());
                let list = self.servers.servers.clone();
                match servers::write_servers_dat(&path, &list) {
                    Ok(()) => {
                        self.servers_dat_status =
                            tr_fmt(lang, "written to {0}", &[&path.display().to_string()]);
                    }
                    Err(e) => {
                        self.servers_dat_status = tr_fmt(lang, "failed: {0}", &[&format!("{e:#}")])
                    }
                }
            }
            if ui.button(tr(lang, "Import from servers.dat")).clicked() {
                let path = servers::servers_dat_path(&self.active_game_dir());
                let imported = servers::read_servers_dat(&path);
                if imported.is_empty() {
                    self.servers_dat_status = tr_fmt(
                        lang,
                        "nothing to import from {0}",
                        &[&path.display().to_string()],
                    );
                } else {
                    self.servers.servers = imported;
                    self.save_servers();
                    self.servers_dat_status = tr(lang, "imported").into();
                }
            }
            if !self.servers_dat_status.is_empty() {
                ui.label(&self.servers_dat_status);
            }
        });
    }
}
