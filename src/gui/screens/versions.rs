//! The Versions screen: the merged local/remote version list with filters,
//! search and sorting, plus the mod-loader installer and the vanilla
//! version install jobs.

use std::sync::atomic::Ordering;

use crate::gui::state::{
    App, VersionFilter, VersionRow, VersionSort, merge_versions, sort_version_rows,
};
use crate::gui::toast_ui::draw_search_icon;
use crate::lang::{tr, tr_fmt};
use crate::updater;

impl App {
    pub(crate) fn ui_versions(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        ui.heading(tr(lang, "Versions"));
        ui.add_space(4.0);

        if self.manifest.is_none() && !self.manifest_loading {
            self.manifest_loading = true;
            self.spawn_job(
                move || {
                    updater::fetch_manifest(&crate::util::net::agent(), lang).map_err(|e| e.to_string())
                },
                |app, result| {
                    app.manifest = Some(result);
                    app.manifest_loading = false;
                },
            );
        }

        // ── Filters section (top level, above the list) ──
        ui.strong(tr(lang, "Filters"));
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
                ui.selectable_value(&mut self.version_filter, filter, filter.label(lang));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.weak(tr(lang, "Loader:"));
            ui.selectable_value(&mut self.version_loader_filter, None, tr(lang, "Any"));
            for l in updater::Loader::ALL {
                ui.selectable_value(&mut self.version_loader_filter, Some(l), l.label());
            }
        });
        ui.horizontal(|ui| {
            draw_search_icon(ui, 16.0, ui.visuals().text_color());
            ui.add(
                egui::TextEdit::singleline(&mut self.version_search)
                    .hint_text(tr(lang, "Search…"))
                    .desired_width(200.0),
            );
            if ui.button(tr(lang, "Rescan")).clicked() {
                self.reload_versions();
            }
            if ui.button(tr(lang, "Refresh manifest")).clicked() {
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
            crate::gui::state::ui_progress_line(ui, &progress, lang);
            ui.separator();
        }
        egui::CollapsingHeader::new(tr(lang, "Install a mod loader"))
            .id_salt("loader_installer")
            .default_open(false)
            .show(ui, |ui| {
                self.ui_loader_row(ui, installing);
            });
        ui.separator();

        if let Some(Err(e)) = &self.manifest {
            ui.colored_label(
                egui::Color32::YELLOW,
                tr_fmt(
                    lang,
                    "Manifest unavailable ({0}) — showing local versions only",
                    &[&e.to_string()],
                ),
            );
        }

        // ── Sort bar (same level as the list) ──
        ui.horizontal(|ui| {
            ui.weak(tr(lang, "Sort:"));
            egui::ComboBox::from_id_salt("version_sort")
                .selected_text(self.version_sort.label(lang))
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
                        ui.selectable_value(&mut self.version_sort, s, s.label(lang));
                    }
                });
            let total = self.versions.len();
            ui.weak(tr_fmt(lang, "{0} installed", &[&total.to_string()]));
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

        ui.weak(tr_fmt(
            lang,
            "selected: {0}",
            &[if self.settings.selected_version.is_empty() {
                tr(lang, "— none —")
            } else {
                &self.settings.selected_version
            }],
        ));

        // The version list takes all remaining space.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in shown.iter() {
                    self.version_row_ui(ui, row, installing);
                }
                if shown.is_empty() {
                    ui.weak(tr(lang, "No versions match the current filter."));
                }
            });
    }

    /// Render one row of the version list (Select / Install controls).
    pub(crate) fn version_row_ui(&mut self, ui: &mut egui::Ui, row: &VersionRow, installing: bool) {
        let lang = self.settings.language;
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
                ui.weak(format!("[{}]", tr(lang, row.kind.as_str())));
            }
            if row.is_latest_release {
                ui.weak(tr(lang, "(latest release)"));
            }
            let is_selected = self.settings.selected_version == row.name;
            if is_selected {
                ui.colored_label(egui::Color32::LIGHT_GREEN, tr(lang, "selected"));
            }
            if row.installed {
                if is_selected {
                    ui.weak(tr(lang, "current"));
                } else if ui.button(tr(lang, "Select")).clicked() {
                    self.settings.selected_version = row.name.clone();
                    self.save_settings();
                }
            } else if ui
                .add_enabled(!installing, egui::Button::new(tr(lang, "Install")))
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
        let lang = self.settings.language;
        self.spawn_job(
            move || {
                let agent = crate::util::net::agent();
                updater::fetch_loader_builds(&agent, loader, &mc_task, lang)
                    .map_err(|e| e.to_string())
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
        {
            let mut p = self
                .install_progress
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            p.label.clear();
            p.frac = None;
        }
        let game_dir = self.active_game_dir();
        let progress = self.install_progress.clone();
        let installing = self.installing.clone();
        let mc_display = mc.clone();
        let lang = self.settings.language;

        self.spawn_job(
            move || {
                let agent = crate::util::net::agent();
                let result = updater::install_loader(
                    &agent,
                    &game_dir,
                    loader,
                    &mc,
                    &build,
                    lang,
                    &mut |msg, frac| {
                        let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                        p.label = msg.to_string();
                        p.frac = frac;
                    },
                );
                installing.store(false, Ordering::SeqCst);
                result.map_err(|e| e.to_string())
            },
            move |app, result| {
                let mc = mc_display;
                let lang = app.settings.language;
                match &result {
                    Ok(outcome) => {
                        app.settings.selected_version = outcome.version_id.clone();
                        app.save_settings();
                        app.log_console(format!(
                            "[RustLauncher] {}",
                            tr_fmt(
                                lang,
                                "installed {0} ({1} new files)",
                                &[&outcome.version_id, &outcome.downloaded_files.to_string()]
                            )
                        ));
                        app.notify_info(tr_fmt(
                            lang,
                            "{0} {1} on {2} is ready to play",
                            &[loader.label(), &outcome.version_id, &mc],
                        ));
                    }
                    Err(e) => {
                        app.notify_error(
                            "LOADER-INSTALL",
                            tr_fmt(
                                lang,
                                "{0} install failed: {1}",
                                &[loader.label(), &e.to_string()],
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
        let lang = self.settings.language;
        ui.strong(tr(lang, "Install a mod loader"));
        ui.add_space(2.0);

        // The game root must be configured; the installer writes into
        // `<game dir>/versions/…`.
        let game_dir = self.active_game_dir();
        let game_dir_ok = game_dir.exists();
        if !game_dir_ok {
            ui.colored_label(
                egui::Color32::YELLOW,
                tr_fmt(
                    lang,
                    "Root game directory is not set — configure it in Settings; installs would go to {0}",
                    &[&game_dir.display().to_string()],
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
            ui.weak(tr(
                lang,
                "Load the Mojang manifest (Refresh manifest) to pick a version.",
            ));
            return;
        }
        let mc_selected = self
            .loader_mc_pick
            .clone()
            .unwrap_or_else(|| releases[0].clone());
        egui::ComboBox::from_label(tr(lang, "Minecraft"))
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
                ui.weak(tr_fmt(
                    lang,
                    "Loading {0} builds…",
                    &[self.loader_pick.label()],
                ));
            }
            Some(None) => {
                ui.weak(tr_fmt(
                    lang,
                    "{0} is not available for {1}",
                    &[self.loader_pick.label(), &mc],
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
                                tr_fmt(lang, "{0} (beta)", &[&b.version])
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
                            egui::Button::new(tr_fmt(
                                lang,
                                "Install {0} {1} on {2}",
                                &[self.loader_pick.label(), &build.version, &mc],
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
        {
            let mut p = self
                .install_progress
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            p.label.clear();
            p.frac = None;
        }
        let game_dir = self.active_game_dir();
        let progress = self.install_progress.clone();
        let installing = self.installing.clone();
        let lang = self.settings.language;

        self.spawn_job(
            move || {
                let agent = crate::util::net::agent();
                let result =
                    updater::install_version(&agent, &game_dir, &version, lang, &mut |msg, frac| {
                        let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                        p.label = msg.to_string();
                        p.frac = frac;
                    });
                installing.store(false, Ordering::SeqCst);
                result.map_err(|e| e.to_string())
            },
            |app, result| {
                let lang = app.settings.language;
                match &result {
                    Ok(outcome) => {
                        app.settings.selected_version = outcome.version_id.clone();
                        app.save_settings();
                        app.log_console(format!(
                            "[RustLauncher] {}",
                            tr_fmt(
                                lang,
                                "installed {0} ({1} new files)",
                                &[&outcome.version_id, &outcome.downloaded_files.to_string()]
                            )
                        ));
                    }
                    Err(e) => app.log_console(format!(
                        "[RustLauncher] {}",
                        tr_fmt(lang, "install failed: {0}", &[&e.to_string()])
                    )),
                }
                app.reload_versions();
                app.play_status.clear();
            },
        );
    }
}
