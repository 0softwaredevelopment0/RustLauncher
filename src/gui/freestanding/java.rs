//! Java runtime background tasks: the machine scan behind the Settings
//! picker and the download-and-select flow of the "Download Java" section.

use super::super::state::App;
use crate::lang::tr_fmt;
use crate::services::settings::JavaSelection;

impl App {
    /// Scan the machine for installed Java runtimes in the background.
    pub(crate) fn start_java_scan(&mut self) {
        self.java_scan_loading = true;
        self.java_scanned = true;
        let runtimes = self.home_dir.join("runtimes");
        self.spawn_job(
            move || crate::game::java::locator::detect_installed_javas(Some(&runtimes)),
            |app, list| {
                app.java_installed = list;
                app.java_scan_loading = false;
            },
        );
    }

    /// Combo label for a configured Java path: the detected
    /// `Vendor version` when the path is in the inventory, else the path.
    pub(crate) fn java_label_for(&self, path: &str) -> String {
        if let Some(j) = self
            .java_installed
            .iter()
            .find(|j| j.path.display().to_string() == path)
        {
            return j.label();
        }
        path.to_string()
    }

    /// Download + unpack a runtime in the background and select it when it
    /// lands. Uses the edition/major currently chosen in the download UI.
    pub(crate) fn start_java_download(&mut self, sub: String) {
        let edition = self.java_dl_edition.clone();
        let major = self.java_dl_major;
        self.java_downloading = Some((edition.clone(), major, sub.clone()));
        let runtimes = self.home_dir.join("runtimes");
        let lang = self.settings.language;
        {
            let mut p = self
                .java_progress
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            p.label.clear();
            p.frac = None;
        }
        let progress = self.java_progress.clone();
        self.spawn_job(
            move || {
                let result = crate::game::java::download::install_runtime(
                    &crate::util::net::agent(),
                    lang,
                    &edition,
                    major,
                    &sub,
                    &runtimes,
                    &|msg, frac| {
                        let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                        p.label = msg.to_string();
                        p.frac = frac;
                    },
                )
                .map(|p| p.display().to_string())
                .map_err(|e| e.to_string());
                {
                    let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                    p.frac = Some(1.0);
                }
                result
            },
            |app, result| {
                app.java_downloading = None;
                match result {
                    Ok(java_path) => {
                        app.settings.java_mode = JavaSelection::Installed(java_path.clone());
                        let _ = app.settings.save(&app.home_dir);
                        // Re-scan so the combo shows the new runtime.
                        app.start_java_scan();
                        let label = app.java_label_for(&java_path);
                        app.notify_info(tr_fmt(
                            app.settings.language,
                            "Java runtime installed: {0}",
                            &[&label],
                        ));
                    }
                    Err(e) => {
                        app.notify_error("java-download-failed", &e);
                    }
                }
            },
        );
    }
}
