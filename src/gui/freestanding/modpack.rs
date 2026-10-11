//! The `.mrpack` import flow of the New instance dialog: file pick/drop,
//! index validation and the background install that registers the instance.

use anyhow::Result;

use super::super::state::App;
use crate::lang::{tr, tr_fmt};

impl App {
    /// Point the dialog at a `.mrpack` archive: validate the extension,
    /// read the index and auto-fill name / game dir / summary.
    pub(crate) fn set_modpack_file(&mut self, path: &std::path::Path) {
        let lang = self.settings.language;
        self.instances_error = None;
        if !path
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".mrpack")
        {
            self.instances_error = Some(
                tr(
                    lang,
                    "the modpack file must have the .mrpack extension",
                )
                .to_string(),
            );
            return;
        }
        let index = match crate::content::modpack::read_index(path, lang) {
            Ok(index) => index,
            Err(e) => {
                self.instances_error = Some(format!("{e:#}"));
                return;
            }
        };
        let name = if index.name.is_empty() {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        } else {
            index.name.clone()
        };
        self.new_instance_pack_summary = if index.summary.is_empty() {
            None
        } else {
            Some(index.summary)
        };
        self.new_instance_name = name.clone();
        // The auto-filled root reserves the same numbered suffix that
        // `create()` will derive at registration time.
        let dir = crate::util::home::instances_dir(&self.home_dir)
            .join(self.instance_store.unique_name(&name));
        self.new_instance_dir = dir.to_string_lossy().to_string();
        self.new_instance_pack_file = path.to_string_lossy().to_string();
    }

    /// Open a file dialog for a `.mrpack` archive, starting in Downloads.
    pub(crate) fn browse_modpack_file(&mut self) {
        let lang = self.settings.language;
        let mut dialog = rfd::FileDialog::new().add_filter(tr(lang, "Modrinth modpack"), &["mrpack"]);
        if let Some(dir) = dirs::download_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.pick_file() else {
            return; // user cancelled
        };
        self.set_modpack_file(&path);
    }

    /// Validate the dialog inputs and run the modpack import in the
    /// background; the completion job registers the instance and deletes
    /// the archive on success.
    pub(crate) fn start_modpack_install(&mut self) {
        let lang = self.settings.language;
        if self.modpack_installing {
            return; // one import at a time
        }
        self.instances_error = None;

        let name = self.new_instance_name.trim().to_string();
        let dir = self.new_instance_dir.trim().to_string();
        let pack_file = self.new_instance_pack_file.trim().to_string();
        if name.is_empty() {
            self.instances_error = Some(tr(lang, "instance name is empty").to_string());
            return;
        }
        if dir.is_empty() {
            self.instances_error = Some(tr(lang, "instance game directory is empty").to_string());
            return;
        }
        let pack_path = std::path::PathBuf::from(&pack_file);
        if pack_file.is_empty() || !pack_path.is_file() {
            self.instances_error = Some(tr(lang, "select a .mrpack archive first").to_string());
            return;
        }

        self.modpack_installing = true;
        {
            let mut p = self
                .modpack_progress
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            p.label = tr_fmt(lang, "Installing modpack '{0}'…", &[&name]);
            p.frac = Some(0.0);
        }

        let game_root = std::path::PathBuf::from(&dir);
        let progress_line = self.modpack_progress.clone();
        let dir_for_apply = dir.clone();
        let name_for_apply = name.clone();
        let pack_for_apply = pack_file.clone();

        self.spawn_job(
            move || {
                let agent = crate::util::net::agent();
                let result = crate::content::modpack::install(
                    &pack_path,
                    &game_root,
                    &agent,
                    lang,
                    &mut |msg: &str, frac: Option<f32>| {
                        let mut p = progress_line
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        p.label = msg.to_string();
                        p.frac = frac;
                    },
                );
                {
                    let mut p = progress_line
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    p.frac = Some(1.0);
                }
                result.map_err(|e| format!("{e:#}"))
            },
            move |app, result: Result<crate::content::modpack::InstallOutcome, String>| {
                let lang = app.settings.language;
                app.modpack_installing = false;
                {
                    let mut p = app
                        .modpack_progress
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    p.label.clear();
                    p.frac = None;
                }
                match result {
                    Ok(outcome) => {
                        match app.instance_store.create(&name_for_apply, &dir_for_apply, lang) {
                            Ok(final_name) => {
                                if let Err(e) = app.instance_store.save(&app.home_dir, lang) {
                                    let text = format!("{e:#}");
                                    app.instances_error = Some(text.clone());
                                    app.notify_error("INSTANCES", text);
                                    return;
                                }
                                // The archive is a hand-off artifact: remove it
                                // only after a fully successful import.
                                if let Err(e) = std::fs::remove_file(&pack_for_apply) {
                                    app.notify_error(
                                        "MODPACK",
                                        tr_fmt(
                                            lang,
                                            "failed to delete the modpack archive: {0}",
                                            &[&e.to_string()],
                                        ),
                                    );
                                }
                                // Wire the new instance into the launcher: the
                                // Versions tab lists the active instance's game
                                // dir, so selecting the instance plus the
                                // modpack's version makes the tab show it as
                                // installed and Play launches exactly it.
                                app.launch_instance = final_name.clone();
                                app.settings.selected_version = outcome.version_id.clone();
                                app.save_settings();
                                app.reload_versions();
                                let msg = tr_fmt(
                                    lang,
                                    "Modpack '{0}' installed into instance '{1}'",
                                    &[&outcome.pack_name, &final_name],
                                );
                                app.log_console(format!("[RustLauncher] {msg}"));
                                app.notify_info(msg);
                                app.new_instance_name.clear();
                                app.new_instance_dir.clear();
                                app.new_instance_pack_file.clear();
                                app.new_instance_pack_summary = None;
                                app.new_instance_modpack = false;
                            }
                            Err(e) => {
                                let text = format!("{e:#}");
                                app.instances_error = Some(text);
                            }
                        }
                    }
                    Err(e) => {
                        app.instances_error = Some(e.clone());
                        app.notify_error("MODPACK", e);
                    }
                }
            },
        );
    }
}
