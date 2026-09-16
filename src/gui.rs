//! The eframe/egui GUI — the successor of the 4300-line JavaFX controller,
//! covering the same screens: play, console, version catalog, servers,
//! accounts, skins, news, settings, profiles and diagnostics.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};

use crate::accounts::AccountStore;
use crate::auth;
use crate::diagnostics;
use crate::home::{self};
use crate::launcher::{self, LaunchPlan};
use crate::logs::SessionLog;
use crate::news::{self, NewsItem};
use crate::notifications::{Toast, ToastKind, Toasts};
use crate::profiles;
use crate::servers::{self, ServerStore};
use crate::settings::Settings;
use crate::skins;
use crate::updater::{self, Manifest};
use crate::version::{self, Version};
use crate::version_json::VersionJson;

/// The maximum number of console lines kept in memory.
const CONSOLE_CAP: usize = 8000;

/// A deferred UI mutation produced by a background thread.
type Job = Box<dyn FnOnce(&mut App) + Send>;

/// Cached loader builds for one (loader, mc-version) pair.
type LoaderBuildsCache =
    BTreeMap<(updater::Loader, String), Option<Result<Vec<updater::LoaderBuild>, String>>>;

pub struct App {
    home_dir: PathBuf,
    pub settings: Settings,
    pub accounts: AccountStore,
    pub servers: ServerStore,
    pub versions: Vec<Version>,
    pub screen: Screen,

    // Play screen.
    pub username_input: String,
    pub play_status: String,
    pub launch_error: Option<String>,
    /// A Stop/Kill confirmation dialog is open, guarding this action.
    terminate_confirm: Option<TerminateKind>,
    /// Live state of the "Don't ask again" checkbox while the dialog is open.
    terminate_dont_ask: bool,

    // Game process.
    pub console: Arc<Mutex<Vec<String>>>,
    pub console_seq: usize,
    pub game_running: Arc<AtomicBool>,
    game_log: Arc<Mutex<Option<SessionLog>>>,
    /// PID of the running game's java process, shared with the launch thread.
    game_pid: Arc<Mutex<Option<u32>>>,

    // Version list (merged local + remote).
    pub manifest: Option<Result<Manifest, String>>,
    pub manifest_loading: bool,
    pub version_filter: VersionFilter,
    pub version_search: String,
    pub install_progress: Arc<Mutex<String>>,
    pub installing: Arc<AtomicBool>,
    /// Mod loader selected in the Versions tab.
    pub loader_pick: updater::Loader,
    /// Builds cached per loader for the release selected by search/filter.
    pub loader_builds: LoaderBuildsCache,
    pub loader_builds_loading: bool,
    /// The loader build chosen in the combo box for the current target.
    pub loader_selected: Option<String>,

    // Servers.
    pub server_status: BTreeMap<usize, String>,
    pub new_server_name: String,
    pub new_server_addr: String,
    pub servers_dat_status: String,

    // Accounts.
    pub account_error: Option<String>,

    // Skins.
    pub skin_names: Vec<String>,
    pub selected_skin: Option<String>,
    pub skin_texture: Option<(String, egui::TextureHandle)>,
    pub skin_status: String,

    // News.
    pub news: Option<Result<Vec<NewsItem>, String>>,

    // Diagnostics.
    pub diag_results: Option<Vec<diagnostics::CheckResult>>,
    pub diag_running: bool,

    // Profiles.
    pub profile_index: profiles::ProfileIndex,
    pub profile_error: Option<String>,

    // Background completions.
    jobs: Arc<Mutex<Vec<Job>>>,
    /// Handle used by background threads to wake the UI when a job finishes.
    ctx: egui::Context,

    // Toast notifications (bottom-left).
    toasts: Toasts,
    /// Last frame instant, for advancing toast aging.
    last_frame: Option<std::time::Instant>,
    /// Launcher error log (logs/launcher-N.log), written at startup and on
    /// every launcher error so toasts can show its tail.
    launcher_log: Option<SessionLog>,
}

/// Which destructive action the confirmation dialog is guarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminateKind {
    Stop,
    Kill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    General,
    Console,
    Versions,
    Servers,
    Accounts,
    Skins,
    News,
    Settings,
    Diagnostics,
}

/// One row of the merged version list: locally installed versions (vanilla,
/// Fabric, whatever lives in the game directory) plus every version from the
/// Mojang manifest.
#[derive(Debug, Clone)]
pub struct VersionRow {
    pub name: String,
    /// `release` / `snapshot` / `old_alpha` / `old_beta`, or `local` when the
    /// version exists only on disk (modded installs missing from the manifest).
    pub kind: String,
    pub installed: bool,
    pub is_latest_release: bool,
    /// Present when the version can be (re-)installed from the manifest.
    pub remote: Option<updater::ManifestVersion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionFilter {
    All,
    Release,
    Snapshot,
    Old,
    Installed,
}

impl VersionFilter {
    fn label(self) -> &'static str {
        match self {
            VersionFilter::All => "All",
            VersionFilter::Release => "Releases",
            VersionFilter::Snapshot => "Snapshots",
            VersionFilter::Old => "Old",
            VersionFilter::Installed => "Installed",
        }
    }

    fn matches(self, row: &VersionRow) -> bool {
        match self {
            VersionFilter::All => true,
            VersionFilter::Release => row.kind == "release",
            VersionFilter::Snapshot => row.kind == "snapshot",
            VersionFilter::Old => matches!(row.kind.as_str(), "old_alpha" | "old_beta"),
            VersionFilter::Installed => row.installed,
        }
    }
}

/// Merge locally discovered versions with the remote manifest. Local versions
/// come first (the game directory order), then remote-only ones; a version
/// present in both places takes the manifest kind and gains an Install entry.
pub fn merge_versions(local: &[Version], manifest: Option<&Manifest>) -> Vec<VersionRow> {
    let latest = manifest
        .and_then(|m| m.latest.get("release").cloned())
        .unwrap_or_default();
    let mut rows: Vec<VersionRow> = Vec::new();
    let mut index_by_name: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();

    for v in local {
        index_by_name.insert(v.name.clone(), rows.len());
        rows.push(VersionRow {
            name: v.name.clone(),
            kind: "local".to_string(),
            installed: true,
            is_latest_release: v.name == latest,
            remote: None,
        });
    }
    if let Some(manifest) = manifest {
        for mv in &manifest.versions {
            if let Some(&i) = index_by_name.get(&mv.id) {
                rows[i].kind = mv.kind.clone();
                rows[i].remote = Some(mv.clone());
            } else {
                index_by_name.insert(mv.id.clone(), rows.len());
                rows.push(VersionRow {
                    name: mv.id.clone(),
                    kind: mv.kind.clone(),
                    installed: false,
                    is_latest_release: mv.id == latest,
                    remote: Some(mv.clone()),
                });
            }
        }
    }
    rows
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        let home_dir = home::launcher_home().unwrap_or_else(|_| std::env::temp_dir());
        let _ = home::ensure(&home_dir);
        let settings = Settings::load(&home_dir);
        let accounts = AccountStore::load(&home::accounts_file(&home_dir));
        let game_dir = resolve_game_dir(&settings);
        let servers = ServerStore::load_or_import(&home_dir, &game_dir);
        let versions = version::list_versions(&game_dir).unwrap_or_default();
        let profile_index = profiles::ProfileIndex::load_or_create(&home::profiles_dir(&home_dir));

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
            game_running: Arc::new(AtomicBool::new(false)),
            game_log: Arc::new(Mutex::new(None)),
            game_pid: Arc::new(Mutex::new(None)),
            manifest: None,
            manifest_loading: false,
            version_filter: VersionFilter::All,
            version_search: String::new(),
            install_progress: Arc::new(Mutex::new(String::new())),
            installing: Arc::new(AtomicBool::new(false)),
            loader_pick: updater::Loader::Fabric,
            loader_builds: BTreeMap::new(),
            loader_builds_loading: false,
            loader_selected: None,
            server_status: BTreeMap::new(),
            new_server_name: String::new(),
            new_server_addr: String::new(),
            servers_dat_status: String::new(),
            account_error: None,
            skin_names: Vec::new(),
            selected_skin: None,
            skin_texture: None,
            skin_status: String::new(),
            news: None,
            diag_results: None,
            diag_running: false,
            profile_index,
            profile_error: None,
            jobs: Arc::new(Mutex::new(Vec::new())),
            ctx,
            toasts: Toasts::default(),
            last_frame: None,
            launcher_log: None,
        };
        app.refresh_skins();
        app.select_saved_skin();
        app.fetch_news();
        app.start_launcher_log();
        app
    }

    // ── helpers ────────────────────────────────────────────────

    fn log_console(&self, line: impl Into<String>) {
        let mut console = self.console.lock().unwrap_or_else(|e| e.into_inner());
        console.push(line.into());
        let excess = console.len().saturating_sub(CONSOLE_CAP);
        if excess > 0 {
            console.drain(..excess);
        }
    }

    fn spawn_job<T, F>(&mut self, task: impl FnOnce() -> T + Send + 'static, apply: F)
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

    fn save_settings(&self) {
        let _ = self.settings.save(&self.home_dir);
    }

    fn save_accounts(&self) {
        let _ = self.accounts.save(&home::accounts_file(&self.home_dir));
    }

    fn save_servers(&mut self) {
        self.versions =
            version::list_versions(&resolve_game_dir(&self.settings)).unwrap_or_default();
        let _ = self.servers.save(&home::servers_file(&self.home_dir));
    }

    fn reload_versions(&mut self) {
        self.versions =
            version::list_versions(&resolve_game_dir(&self.settings)).unwrap_or_default();
    }

    fn refresh_skins(&mut self) {
        self.skin_names = skins::list_skins(&home::skins_dir(&self.home_dir))
            .iter()
            .filter_map(|p| p.file_stem().and_then(|n| n.to_str()))
            .map(str::to_string)
            .collect();
    }

    fn select_saved_skin(&mut self) {
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

    fn fetch_news(&mut self) {
        let url =
            "https://raw.githubusercontent.com/rizer001-Development/launcher-news/main/news.json";
        self.spawn_job(
            move || {
                let agent = crate::net::agent();
                news::fetch(&agent, url)
            },
            |app, result| {
                app.news = Some(result.map_err(|e| e.to_string()));
            },
        );
    }

    fn selected_version(&self) -> Option<&Version> {
        self.versions
            .iter()
            .find(|v| v.name == self.settings.selected_version)
            .or_else(|| self.versions.first())
    }

    fn current_account(&self) -> Option<auth::Account> {
        self.accounts.current().map(|rec| auth::Account {
            username: rec.username.clone(),
            uuid: rec.uuid.clone(),
        })
    }

    // ── game launch ────────────────────────────────────────────

    fn start_game(&mut self) {
        if self.game_running.load(Ordering::SeqCst) {
            self.play_status = "The game is already running".into();
            return;
        }
        self.launch_error = None;

        // The game directory must be configured explicitly.
        if resolve_game_dir(&self.settings)
            .to_string_lossy()
            .trim()
            .is_empty()
        {
            self.launch_error =
                Some("No game directory configured. Set it in Settings first.".into());
            return;
        }
        if !resolve_game_dir(&self.settings).is_dir() {
            self.launch_error = Some(format!(
                "Game directory does not exist: {}",
                resolve_game_dir(&self.settings).display()
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
        let game_dir = resolve_game_dir(&settings);
        let console = self.console.clone();
        let running = self.game_running.clone();
        let game_log = self.game_log.clone();
        let game_pid = self.game_pid.clone();
        let save_log = settings.save_console_log;
        let home_dir = self.home_dir.clone();

        running.store(true, Ordering::SeqCst);
        self.screen = Screen::Console;
        self.play_status = format!("Launching {} …", version.name);
        self.notify_info(format!("Starting {} …", version.name));

        self.spawn_job(
            move || {
                let result = run_game_process(
                    &game_dir, &version, &account, &settings, console, save_log, &home_dir,
                    game_log, game_pid,
                );
                running.store(false, Ordering::SeqCst);
                result
            },
            |app, result| match result {
                Ok(code) => {
                    app.play_status = if code == 0 {
                        "Game exited normally".to_string()
                    } else {
                        format!("Game exited with code {code}")
                    };
                    if code == 0 {
                        app.notify_info("Game stopped");
                    } else {
                        app.notify_error("GAME-EXIT", format!("Game exited with code {code}"));
                    }
                    *app.game_pid.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
                Err(e) => {
                    app.play_status.clear();
                    app.launch_error = Some(format!("{e:#}"));
                    app.notify_error("LAUNCH", format!("{e:#}"));
                    *app.game_pid.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
            },
        );
    }

    // ── launcher error log & toasts ──────────────────────

    /// Open `logs/launcher-N.log` for the whole app lifetime; every launcher
    /// error is written there so error toasts can show its tail.
    fn start_launcher_log(&mut self) {
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
    fn notify_error(&mut self, code: &str, message: impl std::fmt::Display) {
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
    fn notify_info(&mut self, title: impl Into<String>) {
        self.toasts.push(Toast::info(title));
    }

    /// Advance toast aging; called once per frame.
    fn tick_toasts(&mut self) {
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
    fn show_toasts(&mut self, ctx: &egui::Context) {
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
    fn toast_area(
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

    fn stop_game(&mut self) {
        // Ask the game to close politely (like the window's X):
        // taskkill without /F posts WM_CLOSE; the JVM runs shutdown hooks.
        let pid = *self.game_pid.lock().unwrap_or_else(|e| e.into_inner());
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
        self.play_status = "Stop requested".into();
    }

    /// The Stop/Kill confirmation dialog: material warning triangle,
    /// a "don't ask again" checkbox, and Confirm / Cancel buttons.
    fn show_terminate_confirmation(&mut self, ctx: &egui::Context, kind: TerminateKind) {
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
        egui::Area::new(egui::Id::new("terminate_confirm_dialog"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::window(ui.style()).show(ui, |ui| {
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
            });
    }

    /// Force-kill the game process tree (taskkill /T /F) — the last resort.
    fn kill_game(&mut self) {
        let pid = *self.game_pid.lock().unwrap_or_else(|e| e.into_inner());
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
        self.play_status = "Kill issued".into();
    }

    // ── per-frame plumbing ─────────────────────────────────────

    fn apply_pending_jobs(&mut self) {
        let jobs: Vec<Job> =
            std::mem::take(&mut *self.jobs.lock().unwrap_or_else(|e| e.into_inner()));
        for job in jobs {
            job(self);
        }
    }

    fn sync_visuals(&self, ctx: &egui::Context) {
        ctx.set_visuals(if self.settings.dark_theme {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
    }
}

fn resolve_game_dir(settings: &Settings) -> PathBuf {
    if settings.game_directory.trim().is_empty() {
        version::default_game_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        PathBuf::from(settings.game_directory.trim())
    }
}

/// The blocking part of a game launch, executed on a worker thread.
#[allow(clippy::too_many_arguments)]
fn run_game_process(
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
fn spawn_and_stream(
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

/// Draw one toast card; returns `(close_clicked, body_clicked, size)`.
fn toast_body(
    ui: &mut egui::Ui,
    toast: &Toast,
    kind: ToastKind,
    index: usize,
    expanded: bool,
    expand_progress: f32,
    full_log: Option<&str>,
) -> (bool, bool, egui::Vec2) {
    // The ✕ zone's rect, recorded while drawing the header row.
    let cross_rect = std::cell::Cell::new(egui::Rect::NOTHING);

    let frame_rect = egui::Frame::popup(ui.style())
        .fill(match kind {
            ToastKind::Error => egui::Color32::from_rgb(0x3B, 0x2E, 0x2A), // warm dark red-brown
            ToastKind::Info => ui.style().visuals.widgets.inactive.bg_fill,
        })
        .stroke(match kind {
            ToastKind::Error => {
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0xE5, 0x7F, 0x62))
            }
            ToastKind::Info => ui.style().visuals.widgets.inactive.bg_stroke,
        })
        .show(ui, |ui| {
            ui.set_min_width(320.0);
            ui.set_max_width(360.0);

            // The ✕ close zone, as part of the header row. Its clickable
            // response is registered *after* the card-wide body interact
            // (see the tail of this function) so it wins clicks inside it.
            ui.horizontal(|ui| {
                match kind {
                    ToastKind::Error => draw_warning_triangle(ui, 24.0),
                    ToastKind::Info => draw_info_icon(ui, 24.0),
                }
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.set_min_width(240.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&toast.title).strong());
                        if let Some(code) = &toast.code {
                            ui.label(egui::RichText::new(format!("[{code}]")).weak().monospace());
                        }
                    });
                    if let Some(detail) = &toast.detail {
                        ui.label(egui::RichText::new(detail).monospace().small().weak());
                    }
                });

                // Reserve the ✕ space without a click sense here: the ✕
                // click is registered after the body interact below, and
                // egui routes a click to the last registered interact
                // containing the pointer — that is what makes the ✕ win
                // inside its corner.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                cross_rect.set(rect);
            });

            // The expanding log section (error toasts with a log). The
            // height animates 0 → TOAST_MAX_LOG_HEIGHT; the galley is
            // bottom-anchored inside the clip rect so the section visually
            // opens upward from the header.
            if let Some(log) = full_log {
                if expand_progress > 0.001 {
                    let target_h = crate::notifications::TOAST_MAX_LOG_HEIGHT * expand_progress;
                    ui.add_space(6.0 * expand_progress);
                    let (log_rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), target_h),
                        egui::Sense::hover(),
                    );
                    let painter = ui.painter_at(log_rect);
                    painter.rect_filled(log_rect, 2.0, egui::Color32::from_black_alpha(90));
                    let galley = ui.painter().layout_no_wrap(
                        log.to_string(),
                        egui::FontId::monospace(10.0),
                        egui::Color32::from_rgb(0xC8, 0xC8, 0xC8),
                    );
                    // Keep the last log line pinned to the bottom of the
                    // section while it grows.
                    let dy = (galley.size().y - log_rect.height()).max(0.0);
                    painter.galley(
                        egui::pos2(log_rect.left() + 4.0, log_rect.top() - dy),
                        galley,
                        egui::Color32::TRANSPARENT,
                    );
                }

                ui.label(
                    egui::RichText::new(if expanded {
                        "▲ click to collapse"
                    } else {
                        "▼ click to expand log"
                    })
                    .small()
                    .weak(),
                );
            }
        })
        .response
        .rect;

    // Hit-testing order matters: egui routes a click to the *last*
    // registered interact containing the pointer, so the card body goes
    // first and the ✕ corner second — that is what makes the ✕ clickable
    // at all (a cross registered before the body never receives clicks).
    let body = ui.interact(
        frame_rect,
        egui::Id::new(("toast_body", index)),
        egui::Sense::click(),
    );
    let cross_area = cross_rect.get();
    let cross = ui.interact(
        cross_area,
        egui::Id::new(("toast_close", index)),
        egui::Sense::click(),
    );

    // Paint the ✕ here so its hover highlight tracks the live pointer.
    ui.painter_at(cross_area)
        .add(draw_material_cross(cross_area, cross.hovered()));

    // Countdown bar along the bottom edge: drains from full to empty over
    // the hold time and disappears once the toast is locked by a click.
    let frac = toast.hold_frac();
    if frac > 0.0 {
        let track = egui::Rect::from_min_max(
            egui::pos2(frame_rect.left() + 2.0, frame_rect.bottom() - 4.0),
            egui::pos2(frame_rect.right() - 2.0, frame_rect.bottom() - 1.0),
        );
        let p = ui.painter_at(frame_rect);
        p.rect_filled(track, 1.0, egui::Color32::from_black_alpha(80));
        let accent = match kind {
            ToastKind::Error => egui::Color32::from_rgb(0xFF, 0xC1, 0x07), // amber 500
            ToastKind::Info => egui::Color32::from_rgb(0x21, 0x96, 0xF3),  // blue 500
        };
        let fill = egui::Rect::from_min_max(
            track.min,
            egui::pos2(track.left() + track.width() * frac, track.max.y),
        );
        p.rect_filled(fill, 1.0, accent);
    }

    let close_clicked = cross.clicked();
    let body_clicked = body.clicked()
        && !close_clicked
        && body
            .interact_pointer_pos()
            .is_none_or(|pos| !cross_area.contains(pos));
    (close_clicked, body_clicked, frame_rect.size())
}

/// A material-style ✕ cross shape for the given square rect.
fn draw_material_cross(rect: egui::Rect, hovered: bool) -> egui::Shape {
    let color = if hovered {
        egui::Color32::WHITE
    } else {
        egui::Color32::GRAY
    };
    let stroke = egui::Stroke::new(1.6_f32, color);
    let inset = rect.width() * 0.28;
    let a = egui::pos2(rect.left() + inset, rect.top() + inset);
    let b = egui::pos2(rect.right() - inset, rect.bottom() - inset);
    let c = egui::pos2(rect.right() - inset, rect.top() + inset);
    let d = egui::pos2(rect.left() + inset, rect.bottom() - inset);
    egui::Shape::Vec(vec![
        egui::Shape::line_segment([a, b], stroke),
        egui::Shape::line_segment([c, d], stroke),
    ])
}

/// The last `n` lines of a text file, if it can be read.
fn tail_lines(path: &std::path::Path, n: usize) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let start = content.len().saturating_sub(n.saturating_mul(80));
    let window = &content[start.min(content.len())..];
    let lines: Vec<&str> = window
        .lines()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn push_line(console: &Arc<Mutex<Vec<String>>>, line: String) {
    let mut buf = console.lock().unwrap_or_else(|e| e.into_inner());
    buf.push(line);
    let excess = buf.len().saturating_sub(CONSOLE_CAP);
    if excess > 0 {
        buf.drain(..excess);
    }
}

/// Draw a material-style warning triangle (yellow fill, black exclamation
/// mark) of the given height, vertically centered on the current layout.
fn draw_warning_triangle(ui: &mut egui::Ui, height: f32) {
    const YELLOW: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xC1, 0x07); // amber 500
    const BLACK: egui::Color32 = egui::Color32::BLACK;

    let (rect, _) = ui.allocate_exact_size(egui::vec2(height * 1.1, height), egui::Sense::hover());
    let p = ui.painter_at(rect);

    // Triangle: apex at the top-center, base at the bottom.
    let top = egui::pos2(rect.center().x, rect.top());
    let left = egui::pos2(rect.left(), rect.bottom());
    let right = egui::pos2(rect.right(), rect.bottom());
    p.add(egui::Shape::convex_polygon(
        vec![top, left, right],
        YELLOW,
        egui::Stroke::NONE,
    ));

    // Rounded exclamation mark: a stem bar plus a dot.
    let cx = rect.center().x;
    let bar_top = rect.top() + height * 0.34;
    let bar_bottom = rect.top() + height * 0.62;
    let stroke = egui::Stroke::new(height * 0.09, BLACK);
    p.line_segment(
        [egui::pos2(cx, bar_top), egui::pos2(cx, bar_bottom)],
        stroke,
    );
    p.circle_filled(
        egui::pos2(cx, rect.top() + height * 0.78),
        height * 0.06,
        BLACK,
    );
}

/// Draw a material-style info icon: a filled blue circle with a white "i",
/// the counterpart of [`draw_warning_triangle`] for informational toasts.
fn draw_info_icon(ui: &mut egui::Ui, height: f32) {
    const BLUE: egui::Color32 = egui::Color32::from_rgb(0x21, 0x96, 0xF3); // blue 500
    const WHITE: egui::Color32 = egui::Color32::WHITE;

    let (rect, _) = ui.allocate_exact_size(egui::vec2(height, height), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let c = rect.center();

    p.circle_filled(c, height / 2.0, BLUE);

    // The "i": a dot above, a stem below.
    p.circle_filled(
        egui::pos2(c.x, rect.top() + height * 0.30),
        height * 0.055,
        WHITE,
    );
    p.line_segment(
        [
            egui::pos2(c.x, rect.top() + height * 0.46),
            egui::pos2(c.x, rect.top() + height * 0.72),
        ],
        egui::Stroke::new(height * 0.09, WHITE),
    );
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_pending_jobs();
        self.sync_visuals(ctx);
        self.tick_toasts();

        // Repaint while the game runs or a download is in progress so the
        // console and progress labels keep moving without user input.
        let busy =
            self.game_running.load(Ordering::SeqCst) || self.installing.load(Ordering::SeqCst);
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }

        // Toasts animate (slide-out) and must keep repainting while visible.
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(66));
        }

        egui::SidePanel::left("nav").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.heading("RustLauncher");
            ui.add_space(8.0);
            for (screen, label) in [
                (Screen::General, "▶  General"),
                (Screen::Console, "▤  Console"),
                (Screen::Versions, "☰  Versions"),
                (Screen::Servers, "⛶  Servers"),
                (Screen::Accounts, "◉  Accounts"),
                (Screen::Skins, "☺  Skins"),
                (Screen::News, "✉  News"),
                (Screen::Settings, "⚙  Settings"),
                (Screen::Diagnostics, "✚  Diagnostics"),
            ] {
                let selected = self.screen == screen;
                if ui.selectable_label(selected, label).clicked() {
                    self.screen = screen;
                    if screen == Screen::Skins {
                        self.refresh_skins();
                        self.select_saved_skin();
                    }
                }
            }
            ui.separator();
            if let Some(account) = self.accounts.current() {
                ui.label(format!("Player: {}", account.username));
            }
            ui.label(format!("Version: {}", self.settings.selected_version));
            if self.game_running.load(Ordering::SeqCst) {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "Game running");
            }
        });

        self.show_toasts(ctx);

        egui::CentralPanel::default().show(ctx, |ui| match self.screen {
            Screen::General => self.ui_general(ui),
            Screen::Console => self.ui_console(ui),
            Screen::Versions => self.ui_versions(ui),
            Screen::Servers => self.ui_servers(ui),
            Screen::Accounts => self.ui_accounts(ui),
            Screen::Skins => self.ui_skins(ui, ctx),
            Screen::News => self.ui_news(ui),
            Screen::Settings => self.ui_settings(ui),
            Screen::Diagnostics => self.ui_diagnostics(ui),
        });
    }
}

// ── screens ────────────────────────────────────────────────────

impl App {
    fn ui_general(&mut self, ui: &mut egui::Ui) {
        ui.heading("General");
        ui.add_space(6.0);

        egui::Grid::new("play_grid").num_columns(2).show(ui, |ui| {
            ui.label("Account");
            let current = self
                .accounts
                .current()
                .map(|a| a.username.clone())
                .unwrap_or_default();
            ui.label(if current.is_empty() {
                "— none —".to_string()
            } else {
                current
            });
            ui.end_row();

            ui.label("Version");
            let names: Vec<String> = self.versions.iter().map(|v| v.name.clone()).collect();
            egui::ComboBox::from_id_salt("version_combo")
                .selected_text(if self.settings.selected_version.is_empty() {
                    "— select —".to_string()
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
                ui.label("Auto-connect");
                ui.text_edit_singleline(&mut self.settings.connect_server_ip);
                ui.end_row();
            }
        });
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            let play_enabled = !self.game_running.load(Ordering::SeqCst);
            if ui
                .add_enabled(play_enabled, egui::Button::new("▶  Play"))
                .clicked()
            {
                self.start_game();
            }
            if ui.button("Rescan versions").clicked() {
                self.reload_versions();
            }
            let game_running = self.game_running.load(Ordering::SeqCst);
            let stop = egui::Button::new("Stop");
            if ui
                .add_enabled(game_running, stop)
                .on_disabled_hover_text("The game is not running")
                .clicked()
            {
                if self.settings.confirm_stop {
                    self.terminate_dont_ask = false;
                    self.terminate_confirm = Some(TerminateKind::Stop);
                } else {
                    self.stop_game();
                }
            }
            let kill =
                egui::Button::new(egui::RichText::new("☠ Kill").color(egui::Color32::LIGHT_RED));
            if ui
                .add_enabled(game_running, kill)
                .on_disabled_hover_text("The game is not running")
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

    fn ui_console(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Console");
            if ui.button("Clear").clicked() {
                self.console
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clear();
            }
            if ui.button("Copy all").clicked() {
                let text = self
                    .console
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .join("\n");
                ui.output_mut(|o| o.copied_text = text);
            }
        });
        ui.separator();

        let lines: Vec<String> = {
            let buf = self.console.lock().unwrap_or_else(|e| e.into_inner());
            buf.clone()
        };
        let changed = lines.len() != self.console_seq;
        self.console_seq = lines.len();

        egui::ScrollArea::vertical()
            .stick_to_bottom(changed)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    ui.set_min_width(ui.available_width());
                    egui::Grid::new("console_grid")
                        .num_columns(1)
                        .show(ui, |ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                            ui.style_mut()
                                .text_styles
                                .insert(egui::TextStyle::Body, egui::FontId::monospace(12.0));
                            for line in &lines {
                                ui.label(line);
                                ui.end_row();
                            }
                        });
                });
            });
    }

    fn ui_versions(&mut self, ui: &mut egui::Ui) {
        ui.heading("Versions");
        ui.add_space(4.0);

        if self.manifest.is_none() && !self.manifest_loading {
            self.manifest_loading = true;
            self.spawn_job(
                || updater::fetch_manifest(&crate::net::agent()).map_err(|e| e.to_string()),
                |app, result| {
                    app.manifest = Some(result);
                    app.manifest_loading = false;
                },
            );
        }

        // Filters, search and status line.
        ui.horizontal(|ui| {
            for filter in [
                VersionFilter::All,
                VersionFilter::Release,
                VersionFilter::Snapshot,
                VersionFilter::Old,
                VersionFilter::Installed,
            ] {
                ui.selectable_value(&mut self.version_filter, filter, filter.label());
            }
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut self.version_search)
                    .hint_text("Search…")
                    .desired_width(160.0),
            );
            if ui.button("Rescan").clicked() {
                self.reload_versions();
            }
            if ui.button("Refresh manifest").clicked() {
                self.manifest = None;
            }
        });

        let installing = self.installing.load(Ordering::SeqCst);
        let progress = self
            .install_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if installing {
            ui.label(&progress);
            ui.add(egui::ProgressBar::new(1.0).animate(true).text("working…"));
            ui.separator();
        }
        if let Some(Err(e)) = &self.manifest {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("Manifest unavailable ({e}) — showing local versions only"),
            );
        }

        let manifest_opt = match &self.manifest {
            Some(Ok(manifest)) => Some(manifest),
            _ => None,
        };
        let rows = merge_versions(&self.versions, manifest_opt);
        let search = self.version_search.trim().to_lowercase();
        let shown: Vec<&VersionRow> = rows
            .iter()
            .filter(|row| self.version_filter.matches(row))
            .filter(|row| search.is_empty() || row.name.to_lowercase().contains(&search))
            .collect();

        self.ui_loader_row(ui, installing);

        ui.separator();
        ui.label(format!(
            "{} shown · {} installed · selected: {}",
            shown.len(),
            self.versions.len(),
            if self.settings.selected_version.is_empty() {
                "— none —"
            } else {
                &self.settings.selected_version
            }
        ));
        egui::ScrollArea::vertical().show(ui, |ui| {
            for row in shown {
                ui.horizontal(|ui| {
                    if row.installed {
                        ui.label("✔");
                    } else {
                        ui.label(" ");
                    }
                    ui.monospace(&row.name);
                    ui.weak(format!("[{}]", row.kind));
                    if row.is_latest_release {
                        ui.weak("(latest release)");
                    }
                    let is_selected = self.settings.selected_version == row.name;
                    if is_selected {
                        ui.colored_label(egui::Color32::LIGHT_GREEN, "selected");
                    }
                    if row.installed {
                        if is_selected {
                            ui.weak("current");
                        } else if ui.button("Select").clicked() {
                            self.settings.selected_version = row.name.clone();
                            self.save_settings();
                        }
                    } else if ui
                        .add_enabled(!installing, egui::Button::new("Install"))
                        .clicked()
                    {
                        if let Some(remote) = row.remote.clone() {
                            self.install_version(remote);
                        }
                    }
                });
            }
        });
    }

    /// Load (or take from cache) the loader builds for `mc`.
    fn ensure_loader_builds(&mut self, loader: updater::Loader, mc: &str) {
        let key = (loader, mc.to_string());
        if self.loader_builds.contains_key(&key) || self.loader_builds_loading {
            return;
        }
        self.loader_builds_loading = true;
        let mc_task = mc.to_string();
        let mc_key = mc.to_string();
        self.spawn_job(
            move || {
                let agent = crate::net::agent();
                updater::fetch_loader_builds(&agent, loader, &mc_task).map_err(|e| e.to_string())
            },
            move |app, result| {
                app.loader_builds.insert((loader, mc_key), Some(result));
                app.loader_builds_loading = false;
            },
        );
    }

    fn install_loader(&mut self, mc: String, loader: updater::Loader, build: updater::LoaderBuild) {
        self.installing.store(true, Ordering::SeqCst);
        self.install_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let game_dir = resolve_game_dir(&self.settings);
        let progress = self.install_progress.clone();
        let installing = self.installing.clone();
        let mc_display = mc.clone();

        self.spawn_job(
            move || {
                let agent = crate::net::agent();
                let result =
                    updater::install_loader(&agent, &game_dir, loader, &mc, &build, &mut |msg| {
                        *progress.lock().unwrap_or_else(|e| e.into_inner()) = msg.to_string();
                    });
                installing.store(false, Ordering::SeqCst);
                result.map_err(|e| e.to_string())
            },
            move |app, result| {
                let mc = mc_display;
                match &result {
                    Ok(outcome) => {
                        app.settings.selected_version = outcome.version_id.clone();
                        app.save_settings();
                        app.log_console(format!(
                            "[RustLauncher] installed {} ({} new files)",
                            outcome.version_id, outcome.downloaded_files
                        ));
                        app.notify_info(format!(
                            "{} {} on {mc} is ready to play",
                            loader.label(),
                            outcome.version_id
                        ));
                    }
                    Err(e) => {
                        app.notify_error(
                            "LOADER-INSTALL",
                            format!(
                                "{loader_label} install failed: {e}",
                                loader_label = loader.label()
                            ),
                        );
                    }
                }
                app.reload_versions();
                app.play_status.clear();
            },
        );
    }

    fn ui_loader_row(&mut self, ui: &mut egui::Ui, installing: bool) {
        ui.separator();
        ui.strong("Mod loaders");
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            for l in updater::Loader::ALL {
                ui.selectable_value(&mut self.loader_pick, l, l.label());
            }
        });

        // The Minecraft version loaders install onto: prefer the search box
        // (exact id), else the selected version, else the latest release
        // known from the manifest.
        let manifest_release = match &self.manifest {
            Some(Ok(m)) => m.latest.get("release").cloned().unwrap_or_default(),
            _ => String::new(),
        };
        let search = self.version_search.trim().to_string();
        let mc = if !search.is_empty() {
            search
        } else if !self.settings.selected_version.is_empty() {
            self.settings.selected_version.clone()
        } else {
            manifest_release
        };
        if mc.is_empty() {
            ui.weak("Load a manifest (or search a version id) to pick a loader build.");
            return;
        }

        // Only offer builds when the target is an installed or installable
        // release; snapshots have no loader coverage and search hits on
        // snapshot ids would just 404.
        let known_release = self.versions.iter().any(|v| v.name == mc)
            || self
                .manifest
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .is_some_and(|m| m.versions.iter().any(|v| v.id == mc && v.kind == "release"));
        ui.horizontal(|ui| {
            ui.label(format!("Target: {}", mc));
            if !known_release {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "not a known release — loaders may fail",
                );
            }
        });

        self.ensure_loader_builds(self.loader_pick, &mc);
        let key = (self.loader_pick, mc.clone());
        let builds_state = self.loader_builds.get(&key);
        match builds_state {
            None => {
                ui.weak(format!("Loading {} builds…", self.loader_pick.label()));
            }
            Some(None) => {
                ui.weak(format!(
                    "{} is not available for {mc}",
                    self.loader_pick.label()
                ));
            }
            Some(Some(Err(e))) => {
                ui.colored_label(egui::Color32::YELLOW, e.to_string());
            }
            Some(Some(Ok(builds))) => {
                let current = self
                    .loader_builds
                    .get(&key)
                    .and_then(|s| s.as_ref().and_then(|r| r.as_ref().ok()))
                    .map(|b| b.len())
                    .unwrap_or(0);
                let _ = current;
                let selected = self
                    .loader_selected
                    .get_or_insert_with(|| builds[0].version.clone())
                    .clone();
                let _ = selected;
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
                                format!("{} (beta)", b.version)
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
                            !installing,
                            egui::Button::new(format!(
                                "Install {} {}",
                                self.loader_pick.label(),
                                build.version
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

    fn install_version(&mut self, version: updater::ManifestVersion) {
        self.installing.store(true, Ordering::SeqCst);
        self.install_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let game_dir = resolve_game_dir(&self.settings);
        let progress = self.install_progress.clone();
        let installing = self.installing.clone();

        self.spawn_job(
            move || {
                let agent = crate::net::agent();
                let result = updater::install_version(&agent, &game_dir, &version, &mut |msg| {
                    *progress.lock().unwrap_or_else(|e| e.into_inner()) = msg.to_string();
                });
                installing.store(false, Ordering::SeqCst);
                result.map_err(|e| e.to_string())
            },
            |app, result| {
                match &result {
                    Ok(outcome) => {
                        app.settings.selected_version = outcome.version_id.clone();
                        app.save_settings();
                        app.log_console(format!(
                            "[RustLauncher] installed {} ({} new files)",
                            outcome.version_id, outcome.downloaded_files
                        ));
                    }
                    Err(e) => app.log_console(format!("[RustLauncher] install failed: {e}")),
                }
                app.reload_versions();
                app.play_status.clear();
            },
        );
    }

    fn ui_servers(&mut self, ui: &mut egui::Ui) {
        ui.heading("Servers");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.new_server_name);
            ui.label("Address");
            ui.text_edit_singleline(&mut self.new_server_addr);
            if ui.button("Add").clicked() {
                if self.new_server_addr.trim().is_empty() {
                    self.servers_dat_status = "Address is required".into();
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
                        let color = if status == "online" {
                            egui::Color32::LIGHT_GREEN
                        } else {
                            egui::Color32::LIGHT_RED
                        };
                        ui.colored_label(color, status);
                    }
                    if ui.small_button("Check").clicked() {
                        let address = entry.display_ip();
                        self.spawn_job(
                            move || servers::check_status(&address).to_string(),
                            move |app, status| {
                                app.server_status.insert(index, status);
                            },
                        );
                    }
                    if ui.small_button("Delete").clicked() {
                        self.servers.remove(index);
                        self.server_status.clear();
                        self.save_servers();
                    }
                });
            }
            if self.servers.servers.is_empty() {
                ui.weak("No servers yet. Add one above or import servers.dat.");
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Save to servers.dat").clicked() {
                let path = servers::servers_dat_path(&resolve_game_dir(&self.settings));
                let list = self.servers.servers.clone();
                match servers::write_servers_dat(&path, &list) {
                    Ok(()) => {
                        self.servers_dat_status = format!("written to {}", path.display());
                    }
                    Err(e) => self.servers_dat_status = format!("failed: {e:#}"),
                }
            }
            if ui.button("Import from servers.dat").clicked() {
                let path = servers::servers_dat_path(&resolve_game_dir(&self.settings));
                let imported = servers::read_servers_dat(&path);
                if imported.is_empty() {
                    self.servers_dat_status = format!("nothing to import from {}", path.display());
                } else {
                    self.servers.servers = imported;
                    self.save_servers();
                    self.servers_dat_status = "imported".into();
                }
            }
            if !self.servers_dat_status.is_empty() {
                ui.label(&self.servers_dat_status);
            }
        });
    }

    fn ui_accounts(&mut self, ui: &mut egui::Ui) {
        ui.heading("Accounts (offline)");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Nickname");
            let response = ui.text_edit_singleline(&mut self.username_input);
            if (ui.button("Add / select").clicked()
                || response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                && !self.username_input.trim().is_empty()
            {
                match self.accounts.add(self.username_input.trim()) {
                    Ok(name) => {
                        self.username_input = name;
                        self.account_error = None;
                        self.settings.username = self
                            .accounts
                            .current()
                            .map(|a| a.username.clone())
                            .unwrap_or_default();
                        self.save_accounts();
                        self.save_settings();
                    }
                    Err(e) => self.account_error = Some(e.to_string()),
                }
            }
        });
        if let Some(error) = &self.account_error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        ui.separator();

        let names: Vec<String> = self
            .accounts
            .accounts
            .iter()
            .map(|a| a.username.clone())
            .collect();
        for name in &names {
            ui.horizontal(|ui| {
                let selected = self.accounts.current.as_deref() == Some(name.as_str());
                if ui.radio(selected, name).clicked() {
                    self.accounts.select(name);
                    self.settings.username = name.clone();
                    self.save_accounts();
                    self.save_settings();
                }
                if ui.small_button("Remove").clicked() {
                    self.accounts.remove(name);
                    self.settings.username = self
                        .accounts
                        .current()
                        .map(|a| a.username.clone())
                        .unwrap_or_default();
                    self.save_accounts();
                    self.save_settings();
                }
            });
        }
        if names.is_empty() {
            ui.weak("No accounts yet — type a nickname above (3–16 chars).");
        }
    }

    fn ui_skins(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Skins");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let mut username = self
                .accounts
                .current()
                .map(|a| a.username.clone())
                .unwrap_or_default();
            ui.label("Download for");
            ui.add_enabled(
                false,
                egui::TextEdit::singleline(&mut username).desired_width(120.0),
            );
            if ui.button("Download skin").clicked() && !username.is_empty() {
                let dir = home::skins_dir(&self.home_dir);
                let name = username.clone();
                let name2 = username.clone();
                self.skin_status = "Downloading…".into();
                self.spawn_job(
                    move || {
                        skins::download_skin(&crate::net::agent(), &dir, &name)
                            .map_err(|e| e.to_string())
                    },
                    |app, result| match result {
                        Ok(path) => {
                            app.skin_status = format!("saved {}", path.display());
                            app.refresh_skins();
                            app.selected_skin = Some(name2);
                        }
                        Err(e) => app.skin_status = e,
                    },
                );
            }
            if ui.button("Import PNG…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PNG skin", &["png"])
                    .pick_file()
                {
                    let dir = home::skins_dir(&self.home_dir);
                    match skins::import_skin(&dir, &path) {
                        Ok(name) => {
                            self.refresh_skins();
                            self.selected_skin = Some(name);
                            self.skin_status.clear();
                        }
                        Err(e) => self.skin_status = e.to_string(),
                    }
                }
            }
        });
        if !self.skin_status.is_empty() {
            ui.label(&self.skin_status);
        }
        ui.separator();

        ui.horizontal(|ui| {
            egui::ScrollArea::horizontal().show(ui, |ui| {
                for name in self.skin_names.clone() {
                    ui.selectable_value(&mut self.selected_skin, Some(name.clone()), &name);
                }
            });
        });

        if let (Some(name), ctx) = (self.selected_skin.clone(), ctx) {
            let path = home::skins_dir(&self.home_dir).join(format!("{name}.png"));
            let needs_load = match &self.skin_texture {
                Some((cached, _)) => cached != &name,
                None => true,
            };
            if needs_load && path.is_file() {
                if let Err(e) = skins::load_skin(&path).map(|skin| {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [skin.width as usize, skin.height as usize],
                        &skin.rgba,
                    );
                    let texture = ctx.load_texture("skin", image, egui::TextureOptions::NEAREST);
                    self.skin_texture = Some((name.clone(), texture));
                }) {
                    ui.colored_label(egui::Color32::LIGHT_RED, e.to_string());
                }
            }
            if let Some((_, texture)) = &self.skin_texture {
                ui.add_space(8.0);
                ui.label(format!("Skin: {name}"));
                let size = if texture.size()[1] == 32 {
                    egui::vec2(256.0, 128.0)
                } else {
                    egui::vec2(256.0, 256.0)
                };
                ui.add(egui::Image::new(texture).fit_to_exact_size(size));
            }
        } else {
            ui.weak("No skin selected. Download one or import a 64x32/64x64 PNG.");
        }
    }

    fn ui_news(&mut self, ui: &mut egui::Ui) {
        ui.heading("News");
        ui.add_space(4.0);
        if ui.button("Refresh").clicked() {
            self.news = None;
            self.fetch_news();
        }
        ui.separator();
        match &self.news {
            None => {
                ui.label("Loading…");
            }
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("Feed unavailable: {e}"));
                ui.add_space(4.0);
                for item in news::fallback_news() {
                    self.news_item(ui, &item);
                }
            }
            Some(Ok(items)) => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for item in items {
                        self.news_item(ui, item);
                        ui.separator();
                    }
                });
            }
        }
    }

    fn news_item(&self, ui: &mut egui::Ui, item: &NewsItem) {
        ui.strong(&item.title);
        if !item.date.is_empty() {
            ui.weak(&item.date);
        }
        for line in item.content.lines() {
            ui.label(line);
        }
        ui.add_space(2.0);
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(4.0);
        // Java-mode checkboxes mirror `use_custom_java`; only one is checked.
        let mut default_java = !self.settings.use_custom_java;
        let mut custom_java = self.settings.use_custom_java;
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Profiles.
            ui.strong("Profile");
            ui.horizontal(|ui| {
                let current = self.profile_index.current.clone().unwrap_or_default();
                egui::ComboBox::from_id_salt("profile_combo")
                    .selected_text(&current)
                    .show_ui(ui, |ui| {
                        for name in self.profile_index.profiles.clone() {
                            if ui.selectable_label(name == current, &name).clicked() {
                                if let Err(e) = profiles::switch_profile(
                                    &home::profiles_dir(&self.home_dir),
                                    &mut self.profile_index,
                                    &name,
                                    &self.settings,
                                ) {
                                    self.profile_error = Some(e.to_string());
                                } else {
                                    self.settings = profiles::load_profile(
                                        &home::profiles_dir(&self.home_dir),
                                        &name,
                                    );
                                    self.save_settings();
                                }
                            }
                        }
                    });
                if ui.button("New…").clicked() {
                    let mut name = format!("Profile {}", self.profile_index.profiles.len() + 1);
                    // Auto-name; renaming can come later via the file system.
                    while self.profile_index.profiles.contains(&name) {
                        name.push('·');
                    }
                    match profiles::create_profile(
                        &home::profiles_dir(&self.home_dir),
                        &mut self.profile_index,
                        &name,
                    ) {
                        Ok(created) => {
                            self.settings = profiles::load_profile(
                                &home::profiles_dir(&self.home_dir),
                                &created,
                            );
                            self.save_settings();
                        }
                        Err(e) => self.profile_error = Some(e.to_string()),
                    }
                }
                if ui.button("Delete").clicked() {
                    let dir = home::profiles_dir(&self.home_dir);
                    if let Err(e) =
                        profiles::delete_profile(&dir, &mut self.profile_index, &current)
                    {
                        self.profile_error = Some(e.to_string());
                    } else {
                        self.settings = profiles::load_profile(
                            &dir,
                            &self.profile_index.current.clone().unwrap_or_default(),
                        );
                        self.save_settings();
                    }
                }
            });
            if let Some(error) = &self.profile_error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            ui.separator();

            ui.strong("Directories");
            ui.horizontal(|ui| {
                ui.label("Game dir");
                ui.text_edit_singleline(&mut self.settings.game_directory);
                if ui.button("…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.settings.game_directory = dir.to_string_lossy().to_string();
                    }
                }
            });
            ui.add_space(4.0);

            // Java: Default vs Custom, switched with checkboxes.
            ui.strong("Java");
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut default_java, "Default (auto-detect)")
                    .changed()
                    && default_java
                {
                    self.settings.use_custom_java = false;
                }
                if ui.checkbox(&mut custom_java, "Custom path").changed() && custom_java {
                    self.settings.use_custom_java = true;
                }
            });
            ui.add_enabled_ui(self.settings.use_custom_java, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Java executable");
                    ui.add_enabled(
                        true,
                        egui::TextEdit::singleline(&mut self.settings.java_path)
                            .desired_width(360.0),
                    );
                    if ui.button("…").clicked() {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            self.settings.java_path = file.to_string_lossy().to_string();
                        }
                    }
                });
            });
            ui.add_space(4.0);

            // JVM flags: presets + free edit; heap flags are mandatory.
            ui.strong("JVM flags");
            ui.horizontal(|ui| {
                for preset in crate::jvm::PRESETS {
                    let selected = self.settings.java_args == preset.args;
                    if ui
                        .add_enabled(!selected, egui::Button::new(preset.label))
                        .clicked()
                    {
                        self.settings.java_args = preset.args.to_string();
                    }
                }
            });
            ui.add(
                egui::TextEdit::multiline(&mut self.settings.java_args)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            {
                let flags: Vec<String> = self
                    .settings
                    .java_args
                    .split_whitespace()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                match crate::jvm::validate_jvm_args(&flags) {
                    Ok(()) => {
                        ui.colored_label(egui::Color32::LIGHT_GREEN, "✓ heap flags present");
                    }
                    Err(e) => {
                        ui.colored_label(egui::Color32::LIGHT_RED, e);
                    }
                }
            }
            ui.add_space(4.0);

            ui.strong("Game");
            ui.checkbox(
                &mut self.settings.use_custom_resolution,
                "Custom resolution",
            );

            ui.strong("Launcher");
            ui.checkbox(
                &mut self.settings.save_console_log,
                "Save game console to logs/",
            );
            ui.checkbox(&mut self.settings.all_logs, "Verbose console");
            ui.checkbox(&mut self.settings.dark_theme, "Dark theme");
        });
        ui.separator();
        if ui.button("Save settings").clicked() {
            self.save_settings();
            self.reload_versions();
            self.play_status = "Settings saved".into();
        }
    }

    fn ui_diagnostics(&mut self, ui: &mut egui::Ui) {
        ui.heading("Network diagnostics");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.diag_running, egui::Button::new("Run tests"))
                .clicked()
            {
                self.diag_running = true;
                self.spawn_job(
                    || diagnostics::run_all(&crate::net::agent()),
                    |app, results| {
                        app.diag_results = Some(results);
                        app.diag_running = false;
                    },
                );
            }
            if self.diag_running {
                ui.label("Testing…");
            }
        });
        ui.separator();
        match &self.diag_results {
            None => {
                ui.weak("Press Run tests to check DNS, HTTP and TCP connectivity.");
            }
            Some(results) => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for result in results {
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
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_version(name: &str) -> Version {
        Version {
            name: name.to_string(),
            dir: PathBuf::from("."),
            jar: PathBuf::from(format!("{name}.jar")),
            json: PathBuf::from(format!("{name}.json")),
        }
    }

    fn manifest_version(id: &str, kind: &str) -> updater::ManifestVersion {
        updater::ManifestVersion {
            id: id.to_string(),
            kind: kind.to_string(),
            url: format!("https://example.com/{id}.json"),
            releaseTime: String::new(),
        }
    }

    fn manifest(versions: Vec<updater::ManifestVersion>) -> Manifest {
        let mut latest = BTreeMap::new();
        latest.insert(
            "release".to_string(),
            versions
                .iter()
                .find(|v| v.kind == "release")
                .map(|v| v.id.clone())
                .unwrap_or_default(),
        );
        Manifest { latest, versions }
    }

    #[test]
    fn merge_local_first_then_remote_only() {
        let local = vec![local_version("fabric-1.20.1"), local_version("1.21.4")];
        let remote = manifest(vec![
            manifest_version("1.21.4", "release"),
            manifest_version("1.21", "release"),
            manifest_version("25w14craftmine", "snapshot"),
        ]);
        let rows = merge_versions(&local, Some(&remote));

        // Local versions come first, remote-only afterwards.
        assert_eq!(rows[0].name, "fabric-1.20.1");
        assert_eq!(rows[0].kind, "local");
        assert!(rows[0].installed);
        assert!(rows[0].remote.is_none());

        // 1.21.4 exists in both: kind comes from the manifest, installable.
        assert_eq!(rows[1].name, "1.21.4");
        assert_eq!(rows[1].kind, "release");
        assert!(rows[1].installed);
        assert!(rows[1].remote.is_some());
        assert!(rows[1].is_latest_release);

        // Remote-only versions are not installed.
        let one_twenty_one = rows.iter().find(|r| r.name == "1.21").unwrap();
        assert!(!one_twenty_one.installed);
        assert!(one_twenty_one.remote.is_some());
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn merge_without_manifest_shows_local_only() {
        let local = vec![local_version("fabric-1.20.1")];
        let rows = merge_versions(&local, None);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].installed);
        assert_eq!(rows[0].kind, "local");
    }

    #[test]
    fn merge_with_empty_local_shows_remote_only() {
        let remote = manifest(vec![manifest_version("1.21.4", "release")]);
        let rows = merge_versions(&[], Some(&remote));
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].installed);
        assert!(rows[0].remote.is_some());
    }

    #[test]
    fn filters_match_kinds_and_installed() {
        let rows = [
            VersionRow {
                name: "fabric-1.20.1".into(),
                kind: "local".into(),
                installed: true,
                is_latest_release: false,
                remote: None,
            },
            VersionRow {
                name: "1.21.4".into(),
                kind: "release".into(),
                installed: true,
                is_latest_release: false,
                remote: None,
            },
            VersionRow {
                name: "25w14craftmine".into(),
                kind: "snapshot".into(),
                installed: false,
                is_latest_release: false,
                remote: None,
            },
            VersionRow {
                name: "a1.2.5".into(),
                kind: "old_alpha".into(),
                installed: false,
                is_latest_release: false,
                remote: None,
            },
        ];
        let rows = &rows[..];
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::All.matches(r))
                .count(),
            4
        );
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Release.matches(r))
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["1.21.4"]
        );
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Snapshot.matches(r))
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Old.matches(r))
                .count(),
            1
        );
        // "Installed" includes local-only and manifest-installed releases.
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Installed.matches(r))
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["fabric-1.20.1", "1.21.4"]
        );
    }
}
