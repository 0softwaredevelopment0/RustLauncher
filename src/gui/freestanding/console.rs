//! Console plumbing: pushing lines, propagating the capture-time log mode,
//! sending stdin commands to the followed game and exporting the buffer.

use std::sync::Arc;

use super::super::state::App;
use crate::lang::{tr, tr_fmt};

impl App {
    pub(crate) fn log_console(&self, line: impl Into<String>) {
        self.console
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(line.into());
    }

    /// Propagate the current `console_log_mode` into every live console
    /// buffer. The mode lives in the buffer itself (worker threads push
    /// without access to the settings), so this runs once per frame and
    /// catches combo changes, settings resets and new launches alike.
    pub(crate) fn sync_console_modes(&mut self) {
        let mode = self.settings.console_log_mode;
        self.console
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_mode(mode);
        for game in &self.running_games {
            game.console
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_mode(mode);
        }
    }

    /// Whether the console is currently following a live game (so the
    /// command line can send to it).
    pub(crate) fn console_follows_running(&self) -> bool {
        self.running_games
            .iter()
            .any(|g| g.running.load(std::sync::atomic::Ordering::SeqCst) && Arc::ptr_eq(&g.console, &self.console))
    }

    /// Send one line to the followed game's stdin (a chat line or a command)
    /// and echo it into the console. Returns false when no live game is
    /// being followed.
    pub(crate) fn send_console_line(&mut self, line: &str) -> bool {
        let stdin = self
            .running_games
            .iter()
            .find(|g| g.running.load(std::sync::atomic::Ordering::SeqCst) && Arc::ptr_eq(&g.console, &self.console))
            .map(|g| g.stdin.clone());
        let Some(stdin) = stdin else {
            return false;
        };
        {
            use std::io::Write;
            let mut guard = stdin.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = guard.as_mut() {
                let _ = writeln!(s, "{line}");
                let _ = s.flush();
            }
        }
        self.log_console(format!("> {line}"));
        true
    }

    /// Ask for a destination and write the whole console buffer there.
    pub(crate) fn export_console(&mut self) {
        let lang = self.settings.language;
        let default_name = format!(
            "console-{}.log",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter(tr(lang, "Log files"), &["log", "txt"])
            .save_file()
        else {
            return; // user cancelled
        };
        let text = {
            let buf = self.console.lock().unwrap_or_else(|e| e.into_inner());
            buf.lines().join("\n")
        };
        match std::fs::write(&path, text) {
            Ok(()) => {
                let msg = tr_fmt(
                    lang,
                    "Console exported to {0}",
                    &[&path.display().to_string()],
                );
                self.log_console(msg.clone());
                self.notify_info(msg);
            }
            Err(e) => {
                self.notify_error(
                    "EXPORT",
                    tr_fmt(lang, "console export failed: {0}", &[&format!("{e:#}")]),
                );
            }
        }
    }

    /// Ask for a destination and write the finished diagnostics report
    /// there (the Save to file button on the Diagnostics tab).
    pub(crate) fn export_diagnostics(&mut self) {
        if !self.diag_pending.is_empty() {
            return; // not finished yet — the button is disabled anyway
        }
        let lang = self.settings.language;
        let default_name = format!(
            "diagnostics-{}.txt",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter(tr(lang, "Text files"), &["txt", "log"])
            .save_file()
        else {
            return; // user cancelled
        };
        let text = crate::services::diagnostics::report_text(&self.diag_results);
        match std::fs::write(&path, text) {
            Ok(()) => {
                let msg = tr_fmt(
                    lang,
                    "Diagnostics report saved to {0}",
                    &[&path.display().to_string()],
                );
                self.log_console(msg.clone());
                self.notify_info(msg);
            }
            Err(e) => {
                self.notify_error(
                    "DIAG-EXPORT",
                    tr_fmt(lang, "diagnostics export failed: {0}", &[&format!("{e:#}")]),
                );
            }
        }
    }
}
