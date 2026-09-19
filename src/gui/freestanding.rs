//! Free functions shared across the GUI modules: merge/sort/filter of the
//! version list, version-name helpers, launch/stop/kill process plumbing,
//! toast text shaping and painter-drawn material icons.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};

use crate::accounts::AccountStore;
use crate::auth::{self, AccountKind};
use crate::home;
use crate::icons;
use crate::instances;
use crate::launcher;
use crate::launcher::LaunchPlan;
use crate::logs::SessionLog;
use crate::news;
use crate::notifications::{Toast, ToastKind, Toasts};
use crate::servers::ServerStore;
use crate::settings::Settings;
use crate::skins;
use crate::updater::{self};
use crate::version::{self, Version};
use crate::version_json::VersionJson;

use super::state::{
    App, ContentUi, IconCache, Job, RunningGame, Screen, TerminateKind, VersionFilter, VersionSort,
    CONSOLE_CAP,
};
use super::toast_ui::push_line;
use super::toast_ui::{draw_warning_triangle, tail_lines, toast_body};

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        icons::install(&ctx);
        let home_dir = home::launcher_home().unwrap_or_else(|_| std::env::temp_dir());
        let _ = home::ensure(&home_dir);
        let settings = Settings::load(&home_dir);
        let accounts = AccountStore::load(&home::accounts_file(&home_dir));
        let game_dir = resolve_game_dir(&settings);
        let servers = ServerStore::load_or_import(&home_dir, &game_dir);
        let versions = version::list_versions(&game_dir).unwrap_or_default();
        let instance_store = instances::InstanceStore::load(&home_dir);

        let mut app = App {
            home_dir,
            username_input: settings.username.clone(),
            settings,
            accounts,
            servers,
            versions,
            screen: Screen::General,
            play_status: String::new(),
            launch_error: None,
            terminate_confirm: None,
            terminate_dont_ask: false,
            console: Arc::new(Mutex::new(Vec::new())),
            console_seq: 0,
            running_games: Vec::new(),
            game_log: Arc::new(Mutex::new(None)),
            game_pid: Arc::new(Mutex::new(None)),
            instance_store,
            instances_error: None,
            new_instance_name: String::new(),
            new_instance_dir: String::new(),
            instance_delete_pending: None,
            launch_instance: String::new(),
            instance_terminate_pending: None,
            manifest: None,
            manifest_loading: false,
            version_filter: VersionFilter::All,
            version_loader_filter: None,
            version_sort: VersionSort::Newest,
            version_search: String::new(),
            install_progress: Arc::new(Mutex::new(String::new())),
            installing: Arc::new(AtomicBool::new(false)),
            loader_pick: updater::Loader::Fabric,
            loader_builds: BTreeMap::new(),
            loader_builds_loading: false,
            loader_selected: None,
            loader_mc_pick: None,
            filters_open: true,
            modrinth: ContentUi::default(),
            icon_cache: IconCache::default(),
            content_downloading: false,
            content_progress: Arc::new(Mutex::new(String::new())),
            content_mc_filter: String::new(),
            server_status: BTreeMap::new(),
            new_server_name: String::new(),
            new_server_addr: String::new(),
            servers_dat_status: String::new(),
            account_error: None,
            new_account_kind: AccountKind::Offline,
            new_account_password: String::new(),
            online_login_input: String::new(),
            online_password_input: String::new(),
            ms_login: None,
            ms_removal: None,
            account_busy: false,
            ms_polling: false,
            account_remove_pending: None,
            account_remove_password: String::new(),
            account_remove_login: String::new(),
            account_remove_error: None,
            skin_names: Vec::new(),
            selected_skin: None,
            skin_texture: None,
            skin_status: String::new(),
            news: None,
            news_selected: None,
            diag_results: None,
            diag_running: false,
            jobs: Arc::new(Mutex::new(Vec::new())),
            ctx,
            toasts: Toasts::default(),
            last_frame: None,
            launcher_log: None,
        };
        app.accounts.select_saved(&app.settings.username);
        app.refresh_skins();
        app.select_saved_skin();
        app.fetch_news();
        app.start_launcher_log();
        app
    }

    // ── helpers ────────────────────────────────────────────────

    pub(crate) fn log_console(&self, line: impl Into<String>) {
        let mut console = self.console.lock().unwrap_or_else(|e| e.into_inner());
        console.push(line.into());
        let excess = console.len().saturating_sub(CONSOLE_CAP);
        if excess > 0 {
            console.drain(..excess);
        }
    }

    pub(crate) fn spawn_job<T, F>(&mut self, task: impl FnOnce() -> T + Send + 'static, apply: F)
    where
        T: Send + 'static,
        F: FnOnce(&mut App, T) + Send + 'static,
    {
        let jobs = self.jobs.clone();
        let ctx = self.ctx.clone();
        // Fire-and-forget: the worker never blocks the UI thread. It queues
        // the result and asks egui to repaint so `apply_pending_jobs` picks
        // it up on the next frame.
        std::thread::spawn(move || {
            let value = task();
            let job: Job = Box::new(move |app: &mut App| apply(app, value));
            jobs.lock().unwrap_or_else(|e| e.into_inner()).push(job);
            ctx.request_repaint();
        });
    }

    /// Ask for a destination and write the whole console buffer there.
    pub(crate) fn export_console(&mut self) {
        let default_name = format!(
            "console-{}.log",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter("Log files", &["log", "txt"])
            .save_file()
        else {
            return; // user cancelled
        };
        let text = {
            let buf = self.console.lock().unwrap_or_else(|e| e.into_inner());
            buf.join("\n")
        };
        match std::fs::write(&path, text) {
            Ok(()) => {
                let msg = format!("Console exported to {}", path.display());
                self.log_console(msg.clone());
                self.notify_info(msg);
            }
            Err(e) => {
                self.notify_error("EXPORT", format!("console export failed: {e:#}"));
            }
        }
    }

    pub(crate) fn save_settings(&self) {
        let _ = self.settings.save(&self.home_dir);
    }

    /// Persist the current account selection (the store itself writes to the
    /// SQLite DB on every mutation; only the selected name lives in settings).
    pub(crate) fn save_accounts(&mut self) {
        self.settings.username = self
            .accounts
            .current()
            .map(|a| a.username.clone())
            .unwrap_or_default();
        self.save_settings();
    }

    pub(crate) fn save_servers(&mut self) {
        self.versions = version::list_versions(&self.active_game_dir()).unwrap_or_default();
        let _ = self.servers.save(&home::servers_file(&self.home_dir));
    }

    pub(crate) fn reload_versions(&mut self) {
        self.versions = version::list_versions(&self.active_game_dir()).unwrap_or_default();
    }

    pub(crate) fn refresh_skins(&mut self) {
        self.skin_names = skins::list_skins(&home::skins_dir(&self.home_dir))
            .iter()
            .filter_map(|p| p.file_stem().and_then(|n| n.to_str()))
            .map(str::to_string)
            .collect();
    }

    pub(crate) fn select_saved_skin(&mut self) {
        if self.selected_skin.is_none() {
            // Show the skin of the current account, if downloaded.
            if let Some(account) = self.accounts.current() {
                let path =
                    home::skins_dir(&self.home_dir).join(format!("{}.png", account.username));
                if path.is_file() {
                    self.selected_skin = Some(account.username.clone());
                }
            }
        }
    }

    pub(crate) fn fetch_news(&mut self) {
        let url = self.settings.news_url.trim().to_string();
        self.spawn_job(
            move || {
                let agent = crate::net::agent();
                news::fetch(&agent, &url)
            },
            |app, result| {
                app.news = Some(result.map_err(|e| e.to_string()));
            },
        );
    }

    pub(crate) fn selected_version(&self) -> Option<&Version> {
        // No silent fallback to the first version: launching a version the
        // user never picked would be a nasty surprise.
        self.versions
            .iter()
            .find(|v| v.name == self.settings.selected_version)
    }

    pub(crate) fn current_account(&self) -> Option<auth::Account> {
        self.accounts.current().map(|rec| auth::Account {
            username: rec.username.clone(),
            uuid: rec.uuid.clone(),
            access_token: if rec.access_token.is_empty() {
                // Offline convention: the game accepts the UUID.
                rec.uuid.clone()
            } else {
                rec.access_token.clone()
            },
            kind: rec.kind,
        })
    }

    // ── game launch ────────────────────────────────────────────

    /// Any game process is alive.
    pub(crate) fn any_game_running(&self) -> bool {
        self.running_games
            .iter()
            .any(|g| g.running.load(Ordering::SeqCst))
    }

    /// Whether the given instance currently has a live game process.
    pub(crate) fn instance_running(&self, name: &str) -> bool {
        self.running_games
            .iter()
            .any(|g| g.instance == name && g.running.load(Ordering::SeqCst))
    }

    /// The game directory the launcher UI operates on: the selected
    /// instance's directory when one is chosen, otherwise the global
    /// Settings directory.
    pub(crate) fn active_game_dir(&self) -> PathBuf {
        if let Some(inst) = self.instance_store.get(&self.launch_instance) {
            let dir = PathBuf::from(inst.game_dir.trim());
            if !dir.as_os_str().is_empty() {
                return dir;
            }
        }
        resolve_game_dir(&self.settings)
    }

    pub(crate) fn start_game(&mut self) {
        self.launch_error = None;

        let Some(instance) = self.instance_store.get(&self.launch_instance).cloned() else {
            self.launch_error =
                Some("No instance selected. Create one on the Instances tab first.".into());
            self.screen = Screen::Instances;
            return;
        };
        // One live process per instance: launching twice would corrupt the
        // instance's saves/session data.
        if self.instance_running(&instance.name) {
            self.play_status = format!("{} is already running", instance.name);
            return;
        }
        let game_dir = PathBuf::from(instance.game_dir.trim());

        if game_dir.to_string_lossy().trim().is_empty() {
            self.launch_error =
                Some("The instance has no game directory. Edit it on the Instances tab.".into());
            return;
        }
        if !game_dir.is_dir() {
            self.launch_error = Some(format!(
                "Instance game directory does not exist: {}",
                game_dir.display()
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
        if let Err(e) = crate::jvm::validate_jvm_args(&flags) {
            self.launch_error = Some(e);
            self.screen = Screen::Settings;
            return;
        }

        // Custom Java mode must have an actual path.
        if self.settings.use_custom_java && self.settings.java_path.trim().is_empty() {
            self.launch_error = Some(
                "Custom Java is selected but the Java path is empty. Pick a java executable \
                 in Settings or switch back to Default."
                    .into(),
            );
            self.screen = Screen::Settings;
            return;
        }

        let account = match self.current_account() {
            Some(account) => account,
            None => {
                self.launch_error = Some(
                    "No account selected. Add one on the Accounts screen (or type a name there)."
                        .into(),
                );
                return;
            }
        };
        let Some(version) = self.selected_version().cloned() else {
            self.launch_error = Some(
                "No version selected. Install one on the Catalog screen or scan your game dir."
                    .into(),
            );
            return;
        };

        let settings = self.settings.clone();
        let console = Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(AtomicBool::new(false));
        let pid = Arc::new(Mutex::new(None));
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
            error: error.clone(),
            status: status.clone(),
        });
        // Keep the legacy single-game fields pointing at the newest launch so
        // the General tab status and Console follow the latest game.
        self.game_pid = pid.clone();
        self.game_log = log.clone();
        self.console = console.clone();
        self.console_seq = 0;

        let save_log = settings.save_console_log;
        let home_dir = self.home_dir.clone();

        running.store(true, Ordering::SeqCst);
        self.screen = Screen::Console;
        self.play_status = format!("Launching {} ({}) …", version.name, instance.name);
        self.notify_info(format!("Starting {} ({}) …", version.name, instance.name));

        self.spawn_job(
            move || {
                let result = run_game_process(
                    &game_dir, &version, &account, &settings, console, save_log, &home_dir, log,
                    pid,
                );
                running.store(false, Ordering::SeqCst);
                if let Err(e) = &result {
                    *error.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{e:#}"));
                }
                if let Ok(code) = &result {
                    *status.lock().unwrap_or_else(|e| e.into_inner()) = if *code == 0 {
                        "exited normally".to_string()
                    } else {
                        format!("exited with code {code}")
                    };
                }
                result
            },
            move |app, result: Result<i32>| {
                let instance = instance.name.clone();
                match result {
                    Ok(code) => {
                        app.play_status = if code == 0 {
                            format!("{} exited normally", instance)
                        } else {
                            format!("{} exited with code {code}", instance)
                        };
                        if code == 0 {
                            app.notify_info(format!("{} stopped", instance));
                        } else {
                            app.notify_error(
                                "GAME-EXIT",
                                format!("{} exited with code {code}", instance),
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

    // ── launcher error log & toasts ──────────────────────

    /// Open `logs/launcher-N.log` for the whole app lifetime; every launcher
    /// error is written there so error toasts can show its tail.
    pub(crate) fn start_launcher_log(&mut self) {
        match SessionLog::start(&home::logs_dir(&self.home_dir), "launcher") {
            Ok(log) => {
                log.write(&format!(
                    "[Log] RustLauncher {} started",
                    env!("CARGO_PKG_VERSION")
                ));
                self.launcher_log = Some(log);
            }
            Err(e) => eprintln!("[RustLauncher] launcher log failed: {e:#}"),
        }
    }

    /// Record a launcher error: to the launcher log, console and as a toast.
    pub(crate) fn notify_error(&mut self, code: &str, message: impl std::fmt::Display) {
        let line = format!("[ERROR {code}] {message}");
        if let Some(log) = &self.launcher_log {
            log.write(&line);
        }
        self.log_console(line.clone());

        // The collapsed toast shows the last 3 lines; a click expands the
        // full (capped) log, animated.
        let log_path = self.launcher_log.as_ref().map(|l| l.path.clone());
        let detail = log_path.as_deref().and_then(|p| tail_lines(p, 3));
        let full_log = log_path
            .as_deref()
            .and_then(|p| tail_lines(p, crate::notifications::TOAST_MAX_LOG_LINES));

        self.toasts.push(Toast::error(
            "An error occurred",
            Some(code.to_string()),
            detail,
            full_log,
        ));
    }

    /// Push an informational toast (game started/stopped etc.).
    pub(crate) fn notify_info(&mut self, title: impl Into<String>) {
        self.toasts.push(Toast::info(title));
    }

    /// Advance toast aging; called once per frame.
    pub(crate) fn tick_toasts(&mut self) {
        let now = std::time::Instant::now();
        let dt = self
            .last_frame
            .take()
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.0);
        self.last_frame = Some(now);
        self.toasts.tick(dt);
    }

    /// Render the toast stack anchored to the bottom-left corner of the
    /// screen. Each toast is its own Area anchored at LEFT_BOTTOM, offset
    /// upward by the measured heights of the toasts above it, so the stack
    /// always hugs the corner regardless of screen size.
    pub(crate) fn show_toasts(&mut self, ctx: &egui::Context) {
        // Deferred actions: mutating self inside the Area closure fights the
        // outer borrow, so collect and apply them after the UI pass.
        enum Action {
            Pin(usize),
            Close(usize),
            Toggle(usize),
        }
        let mut actions: Vec<Action> = Vec::new();

        const MARGIN: f32 = 12.0;
        const GAP: f32 = 8.0;

        // Newest on top: iterate the queue in reverse, stacking each toast
        // below the previous one.
        let mut y_offset: f32 = 0.0;
        for i in (0..self.toasts.items().len()).rev() {
            let kind = self.toasts.items()[i].kind;
            let slide = self.toasts.items()[i].slide_offset();
            let anchor_y = -MARGIN - y_offset;
            let (close_pressed, body_clicked, height) =
                self.toast_area(ctx, i, egui::vec2(MARGIN + slide, anchor_y));
            if close_pressed {
                actions.push(Action::Close(i));
            } else if body_clicked {
                actions.push(Action::Pin(i));
                if kind == ToastKind::Error {
                    actions.push(Action::Toggle(i));
                }
            }
            // Stack the next (older) toast below this one; fall back to an
            // estimate until the first render has measured the real height.
            let h = if height > 0.0 { height } else { 70.0 };
            y_offset += h + GAP;
        }

        for action in actions {
            match action {
                Action::Close(i) => self.toasts.remove(i),
                Action::Pin(i) => {
                    if let Some(t) = self.toasts.items_mut().get_mut(i) {
                        t.pin();
                    }
                }
                Action::Toggle(i) => {
                    if let Some(t) = self.toasts.items_mut().get_mut(i) {
                        t.toggle_expanded();
                    }
                }
            }
        }
    }

    /// Render one toast in its own bottom-left-anchored Area; returns
    /// `(close_clicked, body_clicked, measured_height)`.
    pub(crate) fn toast_area(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        anchor_offset: egui::Vec2,
    ) -> (bool, bool, f32) {
        let toast = &self.toasts.items()[index];
        let alpha = toast.visual_alpha();
        let kind = toast.kind;
        let expanded = toast.expanded();
        let expand_progress = toast.expand_progress();
        let full_log = toast.full_log().map(str::to_string);
        let id = egui::Id::new(("toast", index));

        let (close_clicked, body_clicked, size) = egui::Area::new(id)
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::LEFT_BOTTOM, anchor_offset)
            .show(ctx, |ui| {
                ui.multiply_opacity(alpha);
                toast_body(
                    ui,
                    toast,
                    kind,
                    index,
                    expanded,
                    expand_progress,
                    full_log.as_deref(),
                )
            })
            .inner;

        // Record the measured height for the next frame's stacking.
        if let Some(t) = self.toasts.items_mut().get_mut(index) {
            t.set_height(size.y);
        }
        (close_clicked, body_clicked, size.y)
    }

    pub(crate) fn stop_game(&mut self) {
        // Legacy single-game Stop: act on the most recently started game.
        let pid = *self.game_pid.lock().unwrap_or_else(|e| e.into_inner());
        self.stop_pid(pid);
        self.play_status = "Stop requested".into();
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
        if let Some(pid) = pid {
            match std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string()])
                .output()
            {
                Ok(_) => {
                    self.log_console(format!("[RustLauncher] Stop requested (PID {pid})"));
                    self.notify_info("Stopping game…");
                }
                Err(e) => {
                    self.log_console(format!("[RustLauncher] Stop failed: {e}"));
                    self.notify_error("STOP", format!("{e:#}"));
                }
            }
        } else {
            self.log_console("[RustLauncher] Stop: no running game process");
        }
    }

    /// The Stop/Kill confirmation dialog: material warning triangle,
    /// a "don't ask again" checkbox, and Confirm / Cancel buttons.
    pub(crate) fn show_terminate_confirmation(&mut self, ctx: &egui::Context, kind: TerminateKind) {
        let (title, body) = match kind {
            TerminateKind::Stop => (
                "Stop the game?",
                "The game will be asked to close. It usually exits within a few seconds, but unsaved progress may be lost.",
            ),
            TerminateKind::Kill => (
                "Force kill the game?",
                "The game process tree will be terminated immediately. Unsaved progress will be lost.",
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
        egui::Window::new(egui::RichText::new("Confirm").strong())
            .id(egui::Id::new("terminate_confirm_dialog"))
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
                ui.checkbox(&mut self.terminate_dont_ask, "Don't ask again");

                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(egui::RichText::new("Confirm").strong()))
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
                    if ui.button("Cancel").clicked() {
                        self.terminate_confirm = None;
                    }
                });
            });
    }

    /// Force-kill the game process tree (taskkill /T /F) — the last resort.
    pub(crate) fn kill_game(&mut self) {
        let pid = *self.game_pid.lock().unwrap_or_else(|e| e.into_inner());
        self.kill_pid(pid);
        self.play_status = "Kill issued".into();
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
        if let Some(pid) = pid {
            self.log_console(format!(
                "[RustLauncher] Kill: terminating PID {pid} and its child processes"
            ));
            match std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .output()
            {
                Ok(out) => {
                    if out.status.success() {
                        self.log_console("[RustLauncher] Game process tree terminated");
                        self.notify_info("Game killed");
                    } else {
                        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                        self.log_console(format!("[RustLauncher] taskkill failed: {stderr}"));
                        self.notify_error("KILL", stderr);
                    }
                }
                Err(e) => {
                    self.log_console(format!("[RustLauncher] Kill failed: {e}"));
                    self.notify_error("KILL", format!("{e:#}"));
                }
            }
        } else {
            self.log_console("[RustLauncher] Kill: no running game process");
        }
    }

    // ── per-frame plumbing ─────────────────────────────────────

    pub(crate) fn apply_pending_jobs(&mut self) {
        let jobs: Vec<Job> =
            std::mem::take(&mut *self.jobs.lock().unwrap_or_else(|e| e.into_inner()));
        for job in jobs {
            job(self);
        }
    }

    pub(crate) fn sync_visuals(&self, ctx: &egui::Context) {
        ctx.set_visuals(if self.settings.dark_theme {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
    }
}

pub(crate) fn resolve_game_dir(settings: &Settings) -> PathBuf {
    if settings.game_directory.trim().is_empty() {
        version::default_game_dir().unwrap_or_else(|_| PathBuf::from("."))
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
    console: Arc<Mutex<Vec<String>>>,
    save_log: bool,
    home_dir: &std::path::Path,
    game_log: Arc<Mutex<Option<SessionLog>>>,
    game_pid: Arc<Mutex<Option<u32>>>,
) -> Result<i32> {
    let json = VersionJson::load(&version.json)?;
    let plan = launcher::build_launch_plan(
        game_dir,
        &version.name,
        &json,
        &version.jar,
        account,
        &settings.java_args,
        if settings.use_custom_java && !settings.java_path.trim().is_empty() {
            Some(settings.java_path.trim())
        } else {
            None
        },
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
    )?;

    if save_log {
        match SessionLog::start(&home::logs_dir(home_dir), "game") {
            Ok(log) => {
                *game_log.lock().unwrap_or_else(|e| e.into_inner()) = Some(log);
            }
            Err(e) => {
                push_line(&console, format!("[RustLauncher] log file failed: {e:#}"));
            }
        }
    }

    push_line(
        &console,
        format!(
            "[RustLauncher] Launching {} with {}",
            version.name,
            plan.java.display()
        ),
    );

    let code = spawn_and_stream(&plan, console, game_pid)?;
    *game_log.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(code)
}
/// Run the plan, streaming output lines into the console buffer.
pub(crate) fn spawn_and_stream(
    plan: &LaunchPlan,
    console: Arc<Mutex<Vec<String>>>,
    game_pid: Arc<Mutex<Option<u32>>>,
) -> Result<i32> {
    use std::io::BufRead;
    use std::process::{Command, Stdio};

    let mut child = Command::new(&plan.java)
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {}", plan.java.display()))?;

    // Publish the PID so Stop/Kill can act on it.
    *game_pid.lock().unwrap_or_else(|e| e.into_inner()) = Some(child.id());

    let stdout = child.stdout.take().context("no stdout")?;
    let stderr = child.stderr.take().context("no stderr")?;

    let drain = |stream: Box<dyn std::io::Read + Send>| {
        let console = console.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stream)
                .lines()
                .map_while(Result::ok)
            {
                push_line(&console, line);
            }
        });
    };
    drain(Box::new(stdout));
    drain(Box::new(stderr));

    let status = child.wait()?;
    Ok(status.code().unwrap_or(-1))
}
