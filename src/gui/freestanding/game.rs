//! The game launch pipeline on the GUI side: validating the launch,
//! running the process on a worker thread (with console streaming),
//! stopping/killing running games and the Stop/Kill confirmation dialog.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};

use super::super::state::{App, ConsoleBuffer, RunningGame, Screen, TerminateKind};
use super::super::toast_ui::push_line;
use crate::gui::toast_ui::draw_warning_triangle;
use crate::account::auth;
use crate::util::home;
use crate::game::launcher;
use crate::game::launcher::LaunchPlan;
use crate::util::logs::SessionLog;
use crate::services::settings::{self, Settings};
use crate::game::version::Version;
use crate::game::version_json::VersionJson;
use crate::lang::{tr, tr_fmt};

impl App {
    pub(crate) fn start_game(&mut self) {
        self.launch_error = None;
        let lang = self.settings.language;

        let Some(instance) = self.instance_store.get(&self.launch_instance).cloned() else {
            self.launch_error = Some(
                tr(
                    lang,
                    "No instance selected. Create one on the Instances tab first.",
                )
                .into(),
            );
            self.screen = Screen::Instances;
            return;
        };
        // One live process per instance: launching twice would corrupt the
        // instance's saves/session data.
        if self.instance_running(&instance.name) {
            self.play_status = tr_fmt(lang, "{0} is already running", &[&instance.name]);
            return;
        }
        let game_dir = PathBuf::from(instance.game_dir.trim());

        if game_dir.to_string_lossy().trim().is_empty() {
            self.launch_error = Some(
                tr(
                    lang,
                    "The instance has no game directory. Edit it on the Instances tab.",
                )
                .into(),
            );
            return;
        }
        if !game_dir.is_dir() {
            self.launch_error = Some(tr_fmt(
                lang,
                "Instance game directory does not exist: {0}",
                &[&game_dir.display().to_string()],
            ));
            return;
        }

        // JVM flags (with the heap flags) are mandatory.
        let flags: Vec<String> = self
            .settings
            .java_args
            .split_whitespace()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if let Err(e) = crate::game::jvm::validate_jvm_args(&flags, lang) {
            self.launch_error = Some(e);
            self.screen = Screen::Settings;
            return;
        }

        // Custom Java mode must have an actual path.
        if self.settings.java_mode.is_empty_path() {
            self.launch_error = Some(
                tr(
                    lang,
                    "Custom Java is selected but the Java path is empty. Pick a java executable \
                     in Settings or switch back to Default.",
                )
                .into(),
            );
            self.screen = Screen::Settings;
            return;
        }

        let account = match self.current_account() {
            Some(account) => account,
            None => {
                self.launch_error = Some(
                    tr(
                        lang,
                        "No account selected. Add one on the Accounts screen (or type a name there).",
                    )
                    .into(),
                );
                return;
            }
        };
        let Some(version) = self.selected_version().cloned() else {
            self.launch_error = Some(
                tr(
                    lang,
                    "No version selected. Install one on the Catalog screen or scan your game dir.",
                )
                .into(),
            );
            return;
        };

        let settings = self.settings.clone();
        let console = Arc::new(Mutex::new(ConsoleBuffer::new(settings.console_log_mode)));
        let running = Arc::new(AtomicBool::new(false));
        let pid = Arc::new(Mutex::new(None));
        let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
        let log: Arc<Mutex<Option<SessionLog>>> = Arc::new(Mutex::new(None));
        let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let status = Arc::new(Mutex::new(String::new()));
        // A finished entry of the same instance is replaced by the new launch.
        self.running_games
            .retain(|g| g.instance != instance.name || g.running.load(Ordering::SeqCst));
        self.running_games.push(RunningGame {
            instance: instance.name.clone(),
            version: version.name.clone(),
            console: console.clone(),
            running: running.clone(),
            pid: pid.clone(),
            stdin: stdin.clone(),
            error: error.clone(),
            status: status.clone(),
        });
        // Keep the legacy single-game fields pointing at the newest launch so
        // the General tab status and Console follow the latest game.
        self.game_pid = pid.clone();
        self.game_log = log.clone();
        self.console = console.clone();
        self.console_seq = 0;

        let file_log = settings.file_log;
        let home_dir = self.home_dir.clone();

        running.store(true, Ordering::SeqCst);
        self.screen = Screen::Console;
        self.play_status = tr_fmt(
            lang,
            "Launching {0} ({1}) …",
            &[&version.name, &instance.name],
        );
        self.notify_info(tr_fmt(
            lang,
            "Starting {0} ({1}) …",
            &[&version.name, &instance.name],
        ));

        self.spawn_job(
            move || {
                let result = run_game_process(
                    &game_dir, &version, &account, &settings, console, file_log, &home_dir, log,
                    pid, stdin,
                );
                running.store(false, Ordering::SeqCst);
                if let Err(e) = &result {
                    *error.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{e:#}"));
                }
                if let Ok(code) = &result {
                    let lang = settings.language;
                    *status.lock().unwrap_or_else(|e| e.into_inner()) = if *code == 0 {
                        tr(lang, "exited normally").to_string()
                    } else {
                        tr_fmt(lang, "exited with code {0}", &[&code.to_string()])
                    };
                }
                result
            },
            move |app, result: Result<i32>| {
                let instance = instance.name.clone();
                let lang = app.settings.language;
                match result {
                    Ok(code) => {
                        app.play_status = if code == 0 {
                            tr_fmt(lang, "{0} exited normally", &[&instance])
                        } else {
                            tr_fmt(
                                lang,
                                "{0} exited with code {1}",
                                &[&instance, &code.to_string()],
                            )
                        };
                        if code == 0 {
                            app.notify_info(tr_fmt(lang, "{0} stopped", &[&instance]));
                        } else {
                            app.notify_error(
                                "GAME-EXIT",
                                tr_fmt(
                                    lang,
                                    "{0} exited with code {1}",
                                    &[&instance, &code.to_string()],
                                ),
                            );
                        }
                    }
                    Err(e) => {
                        app.play_status.clear();
                        app.launch_error = Some(format!("{e:#}"));
                        app.notify_error("LAUNCH", format!("{e:#}"));
                    }
                }
            },
        );
    }

    pub(crate) fn stop_game(&mut self) {
        // Legacy single-game Stop: act on the most recently started game.
        let pid = *self.game_pid.lock().unwrap_or_else(|e| e.into_inner());
        self.stop_pid(pid);
        self.play_status = tr(self.settings.language, "Stop requested").into();
    }

    /// Politely stop a specific running game by PID (posts WM_CLOSE; the JVM
    /// runs shutdown hooks).
    pub(crate) fn stop_instance_pid(&mut self, index: usize) {
        let Some(game) = self.running_games.get(index) else {
            return;
        };
        let pid = *game.pid.lock().unwrap_or_else(|e| e.into_inner());
        self.stop_pid(pid);
    }

    pub(crate) fn stop_pid(&mut self, pid: Option<u32>) {
        let lang = self.settings.language;
        if let Some(pid) = pid {
            match std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string()])
                .output()
            {
                Ok(_) => {
                    self.log_console(format!(
                        "[RustLauncher] {}",
                        tr_fmt(lang, "Stop requested (PID {0})", &[&pid.to_string()])
                    ));
                    self.notify_info(tr(lang, "Stopping game…"));
                }
                Err(e) => {
                    self.log_console(format!(
                        "[RustLauncher] {}",
                        tr_fmt(lang, "Stop failed: {0}", &[&e.to_string()])
                    ));
                    self.notify_error("STOP", format!("{e:#}"));
                }
            }
        } else {
            self.log_console(format!(
                "[RustLauncher] {}",
                tr(lang, "Stop: no running game process")
            ));
        }
    }

    /// Politely stop every running game (launcher exit with the
    /// "close games on exit" setting): posts WM_CLOSE to each game PID so
    /// the JVM runs its shutdown hooks — never a force kill. Fire-and-forget
    /// taskkill processes outlive the launcher; no toasts or console writes
    /// because the launcher window is already closing.
    pub(crate) fn stop_all_games(&mut self) {
        for game in &self.running_games {
            if !game.running.load(Ordering::SeqCst) {
                continue;
            }
            let pid = *game.pid.lock().unwrap_or_else(|e| e.into_inner());
            let Some(pid) = pid else {
                continue;
            };
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string()])
                .spawn();
        }
    }

    /// The Stop/Kill confirmation dialog: material warning triangle,
    /// a "don't ask again" checkbox, and Confirm / Cancel buttons.
    pub(crate) fn show_terminate_confirmation(&mut self, ctx: &egui::Context, kind: TerminateKind) {
        let lang = self.settings.language;
        let (title, body) = match kind {
            TerminateKind::Stop => (
                tr(lang, "Stop the game?").to_string(),
                tr(
                    lang,
                    "The game will be asked to close. It usually exits within a few seconds, but unsaved progress may be lost.",
                )
                .to_string(),
            ),
            TerminateKind::Kill => (
                tr(lang, "Force kill the game?").to_string(),
                tr(
                    lang,
                    "The game process tree will be terminated immediately. Unsaved progress will be lost.",
                )
                .to_string(),
            ),
        };
        let screen = ctx.screen_rect();

        // Dim everything behind the dialog. Layer stack (egui): Background
        // < Panels < Middle < Foreground < Tooltip, so the dim area in
        // Middle covers the panels but sits below the Foreground dialog —
        // it never swallows clicks meant for the dialog itself.
        egui::Area::new(egui::Id::new("terminate_confirm_dim"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.left_top())
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
                if resp.clicked() {
                    self.terminate_confirm = None;
                }
            });

        // The dialog itself sits above the dim layer. The checkbox state
        // lives on App so it survives across frames while open (it is
        // reset when the dialog opens, see the Stop/Kill click handlers).
        let kill_confirmed = kind == TerminateKind::Kill;
        egui::Window::new(egui::RichText::new(tr(lang, "Confirm")).strong())
            .id(egui::Id::new("terminate_confirm_dialog"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            // Hard clamp: min = max = default, so the dialog physically
            // cannot stretch regardless of what egui remembers or measures.
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
                        self.terminate_confirm = None;
                        if self.terminate_dont_ask {
                            match kind {
                                TerminateKind::Stop => self.settings.confirm_stop = false,
                                TerminateKind::Kill => self.settings.confirm_kill = false,
                            }
                            self.save_settings();
                        }
                        if kill_confirmed {
                            self.kill_game();
                        } else {
                            self.stop_game();
                        }
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        self.terminate_confirm = None;
                    }
                });
            });
    }

    /// Force-kill the game process tree (taskkill /T /F) — the last resort.
    pub(crate) fn kill_game(&mut self) {
        let pid = *self.game_pid.lock().unwrap_or_else(|e| e.into_inner());
        self.kill_pid(pid);
        self.play_status = tr(self.settings.language, "Kill issued").into();
    }

    /// Force-kill a specific running game (Instances tab).
    pub(crate) fn kill_instance_pid(&mut self, index: usize) {
        let Some(game) = self.running_games.get(index) else {
            return;
        };
        let pid = *game.pid.lock().unwrap_or_else(|e| e.into_inner());
        self.kill_pid(pid);
    }

    pub(crate) fn kill_pid(&mut self, pid: Option<u32>) {
        let lang = self.settings.language;
        if let Some(pid) = pid {
            self.log_console(format!(
                "[RustLauncher] {}",
                tr_fmt(
                    lang,
                    "Kill: terminating PID {0} and its child processes",
                    &[&pid.to_string()]
                )
            ));
            match std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .output()
            {
                Ok(out) => {
                    if out.status.success() {
                        self.log_console(format!(
                            "[RustLauncher] {}",
                            tr(lang, "Game process tree terminated")
                        ));
                        self.notify_info(tr(lang, "Game killed"));
                    } else {
                        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                        self.log_console(format!(
                            "[RustLauncher] {}",
                            tr_fmt(lang, "taskkill failed: {0}", &[&stderr])
                        ));
                        self.notify_error("KILL", stderr);
                    }
                }
                Err(e) => {
                    self.log_console(format!(
                        "[RustLauncher] {}",
                        tr_fmt(lang, "Kill failed: {0}", &[&e.to_string()])
                    ));
                    self.notify_error("KILL", format!("{e:#}"));
                }
            }
        } else {
            self.log_console(format!(
                "[RustLauncher] {}",
                tr(lang, "Kill: no running game process")
            ));
        }
    }
}

pub(crate) fn resolve_game_dir(settings: &Settings) -> PathBuf {
    if settings.game_directory.trim().is_empty() {
        crate::game::version::default_game_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        PathBuf::from(settings.game_directory.trim())
    }
}

/// The blocking part of a game launch, executed on a worker thread.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_game_process(
    game_dir: &std::path::Path,
    version: &Version,
    account: &auth::Account,
    settings: &Settings,
    console: Arc<Mutex<ConsoleBuffer>>,
    file_log: settings::FileLogMode,
    home_dir: &std::path::Path,
    game_log: Arc<Mutex<Option<SessionLog>>>,
    game_pid: Arc<Mutex<Option<u32>>>,
    game_stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
) -> Result<i32> {
    let json = VersionJson::load(&version.json)?;
    // Download libraries the version json (and its parent chain) declares
    // but that are missing on disk: the first launch of a Fabric/Quilt
    // profile or a hand-copied version must fetch them before the JVM
    // starts, otherwise the modloader exits with "could not find the game".
    let agent = crate::util::net::agent();
    let lib_console = console.clone();
    let mut lib_progress =
        |msg: &str, _frac: Option<f32>| push_line(&lib_console, format!("[RustLauncher] {msg}"));
    crate::updater::ensure_version_libraries(&agent, game_dir, &json, settings.language, &mut lib_progress)?;
    let plan = launcher::build_launch_plan(
        game_dir,
        &version.name,
        &json,
        &version.jar,
        account,
        &settings.java_args,
        settings.java_mode.path(),
        if settings.auto_connect && !settings.connect_server_ip.trim().is_empty() {
            Some(settings.connect_server_ip.trim())
        } else {
            None
        },
        if settings.use_custom_resolution {
            Some((settings.game_width, settings.game_height))
        } else {
            None
        },
        settings.language,
    )?;

    if file_log != settings::FileLogMode::Nothing {
        match SessionLog::start(&home::logs_dir(home_dir), "game") {
            Ok(log) => {
                *game_log.lock().unwrap_or_else(|e| e.into_inner()) = Some(log);
            }
            Err(e) => {
                push_line(
                    &console,
                    format!(
                        "[RustLauncher] {}",
                        tr_fmt(
                            settings.language,
                            "log file failed: {0}",
                            &[&format!("{e:#}")]
                        )
                    ),
                );
            }
        }
    }

    push_line(
        &console,
        format!(
            "[RustLauncher] {}",
            tr_fmt(
                settings.language,
                "Launching {0} with {1}",
                &[&version.name, &plan.java.display().to_string()]
            )
        ),
    );

    let code = spawn_and_stream(
        &plan,
        console,
        game_pid,
        game_log.clone(),
        file_log,
        settings.language,
        game_stdin,
    )?;
    *game_log.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(code)
}
/// Run the plan, streaming output lines into the console buffer.
pub(crate) fn spawn_and_stream(
    plan: &LaunchPlan,
    console: Arc<Mutex<ConsoleBuffer>>,
    game_pid: Arc<Mutex<Option<u32>>>,
    game_log: Arc<Mutex<Option<SessionLog>>>,
    file_log: settings::FileLogMode,
    lang: crate::lang::Language,
    game_stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
) -> Result<i32> {
    use std::io::BufRead;
    use std::process::{Command, Stdio};

    let mut child = Command::new(&plan.java)
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            tr_fmt(
                lang,
                "failed to start {0}",
                &[&plan.java.display().to_string()],
            )
        })?;

    // Publish the PID so Stop/Kill can act on it, and the stdin so the
    // Console command line can send chat lines / commands to the game.
    *game_pid.lock().unwrap_or_else(|e| e.into_inner()) = Some(child.id());
    *game_stdin.lock().unwrap_or_else(|e| e.into_inner()) = child.stdin.take();

    let stdout = child.stdout.take().context(tr(lang, "no stdout"))?;
    let stderr = child.stderr.take().context(tr(lang, "no stderr"))?;

    let drain = |stream: Box<dyn std::io::Read + Send>| {
        let console = console.clone();
        let log_handle = game_log.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stream)
                .lines()
                .map_while(Result::ok)
            {
                if file_log.allows(&line) {
                    if let Some(log) = log_handle
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_ref()
                    {
                        log.write(&line);
                    }
                }
                push_line(&console, line);
            }
        });
    };
    drain(Box::new(stdout));
    drain(Box::new(stderr));

    let status = child.wait()?;
    Ok(status.code().unwrap_or(-1))
}
