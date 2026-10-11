//! The App's non-screen logic, grouped by concern:
//!
//! - `mod.rs` (here) — construction, background job plumbing, shared accessors;
//! - `console` — console buffer plumbing and exports;
//! - `java` — Java runtime scan/download background tasks;
//! - `modpack` — the `.mrpack` import flow of the New instance dialog;
//! - `persistence` — saving settings/accounts/servers and refresh helpers;
//! - `game` — the launch pipeline (plan, process spawn, stop/kill) and the
//!   Stop/Kill confirmation dialog;
//! - `toasts` — launcher logging and the toast notification stack;
//! - `visuals` — theme application and background painting.

mod console;
mod game;
mod java;
mod modpack;
mod persistence;
mod toasts;
mod visuals;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::account::AccountStore;
use crate::account::AccountKind;
use crate::gui::theme::fonts;
use crate::util::home;
use crate::game::instances;
use crate::game::version;
use crate::services::notifications::Toasts;
use crate::services::servers::ServerStore;
use crate::services::settings::Settings;
use crate::updater;

use super::state::{
    App, ConsoleBuffer, ContentUi, IconCache, Job, ProgressLine, Screen, VersionFilter, VersionSort,
};

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        fonts::install(&ctx);
        let home_dir = home::launcher_home().unwrap_or_else(|_| std::env::temp_dir());
        let _ = home::ensure(&home_dir);
        let settings = Settings::load(&home_dir);
        let accounts = AccountStore::load(&home::accounts_file(&home_dir));
        let game_dir = game::resolve_game_dir(&settings);
        let servers = ServerStore::load_or_import(&home_dir, &game_dir);
        let versions = version::list_versions(&game_dir).unwrap_or_default();
        let instance_store = instances::InstanceStore::load(&home_dir);
        let console = Arc::new(Mutex::new(ConsoleBuffer::new(settings.console_log_mode)));

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
            console,
            console_seq: 0,
            console_input: String::new(),
            running_games: Vec::new(),
            game_log: Arc::new(Mutex::new(None)),
            game_pid: Arc::new(Mutex::new(None)),
            instance_store,
            instances_error: None,
            new_instance_name: String::new(),
            new_instance_dir: String::new(),
            instance_delete_pending: None,
            new_instance_modpack: false,
            new_instance_pack_file: String::new(),
            new_instance_pack_summary: None,
            modpack_installing: false,
            modpack_progress: Arc::new(Mutex::new(ProgressLine::default())),
            launch_instance: String::new(),
            instance_terminate_pending: None,
            manifest: None,
            manifest_loading: false,
            version_filter: VersionFilter::All,
            version_loader_filter: None,
            version_sort: VersionSort::Newest,
            version_search: String::new(),
            install_progress: Arc::new(Mutex::new(ProgressLine::default())),
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
            content_progress: Arc::new(Mutex::new(ProgressLine::default())),
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
            java_installed: Vec::new(),
            java_scan_loading: false,
            java_scanned: false,
            java_versions: BTreeMap::new(),
            java_versions_loading: BTreeSet::new(),
            java_dl_edition: "temurin".to_string(),
            java_dl_major: 21,
            java_dl_sub: None,
            java_downloading: None,
            java_progress: Arc::new(Mutex::new(ProgressLine::default())),
            java_download_open: false,
            diag_results: Vec::new(),
            diag_pending: Vec::new(),
            diag_running: false,
            jobs: Arc::new(Mutex::new(Vec::new())),
            ctx,
            toasts: Toasts::default(),
            last_frame: None,
            launcher_log: None,
            settings_reset_pending: false,
            bg_texture: None,
        };
        app.accounts.select_saved(&app.settings.username);
        app.refresh_skins();
        app.select_saved_skin();
        app.fetch_news();
        app.start_launcher_log();
        app
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

    /// Handle files dropped onto the window: only while the Instances tab
    /// is open in modpack import mode; the first dropped `.mrpack` fills
    /// the file field. Everything else is ignored so other screens are
    /// unaffected.
    pub(crate) fn handle_dropped_files(&mut self, files: Vec<egui::DroppedFile>) {
        if self.screen != Screen::Instances || !self.new_instance_modpack {
            return;
        }
        let pack = files.into_iter().find_map(|f| {
            let path = f.path?;
            path
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".mrpack")
                .then_some(path)
        });
        if let Some(path) = pack {
            self.set_modpack_file(&path);
        }
    }

    pub(crate) fn selected_version(&self) -> Option<&version::Version> {
        // No silent fallback to the first version: launching a version the
        // user never picked would be a nasty surprise.
        self.versions
            .iter()
            .find(|v| v.name == self.settings.selected_version)
    }

    pub(crate) fn current_account(&self) -> Option<crate::account::auth::Account> {
        self.accounts.current().map(|rec| crate::account::auth::Account {
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

    /// Any game process is alive.
    pub(crate) fn any_game_running(&self) -> bool {
        self.running_games
            .iter()
            .any(|g| g.running.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// Whether the given instance currently has a live game process.
    pub(crate) fn instance_running(&self, name: &str) -> bool {
        self.running_games
            .iter()
            .any(|g| g.instance == name && g.running.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// The game directory the launcher UI operates on: the selected
    /// instance's directory when one is chosen, otherwise the global
    /// Settings directory.
    pub(crate) fn active_game_dir(&self) -> std::path::PathBuf {
        if let Some(inst) = self.instance_store.get(&self.launch_instance) {
            let dir = std::path::PathBuf::from(inst.game_dir.trim());
            if !dir.as_os_str().is_empty() {
                return dir;
            }
        }
        game::resolve_game_dir(&self.settings)
    }

    /// Apply results queued by background workers (see [`App::spawn_job`]).
    pub(crate) fn apply_pending_jobs(&mut self) {
        let jobs: Vec<Job> =
            std::mem::take(&mut *self.jobs.lock().unwrap_or_else(|e| e.into_inner()));
        for job in jobs {
            job(self);
        }
    }
}
