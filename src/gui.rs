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
use crate::auth::AccountKind;
use crate::content;
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

/// Which platform a content tab browses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentPlatform {
    Modrinth,
}

fn platform_slug(platform: ContentPlatform) -> &'static str {
    match platform {
        ContentPlatform::Modrinth => "modrinth",
    }
}

/// Licenses commonly offered as a filter on Modrinth.
const COMMON_LICENSES: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "GPL-3.0",
    "LGPL-3.0",
    "BSD-3-Clause",
    "MPL-2.0",
    "CC0-1.0",
    "ARR",
];

/// Category chips offered per content kind (Modrinth tags). Loaders are
/// handled by the dedicated loader filter and not repeated here.
fn categories_for(kind: content::ContentKind) -> &'static [&'static str] {
    match kind {
        content::ContentKind::Mod => &[
            "adventure",
            "optimization",
            "gameplay",
            "storage",
            "food",
            "furniture",
            "library",
            "magic",
            "mobs",
            "technology",
            "transportation",
            "utility",
            "worldgen",
        ],
        content::ContentKind::ResourcePack => &[
            "8x-",
            "16x",
            "32x",
            "64x",
            "128x",
            "256x",
            "512x+",
            "simplistic",
            "realistic",
            "themed",
            "vanilla-like",
            "fonts",
            "gui",
            "medieval",
        ],
        content::ContentKind::DataPack => {
            &["adventure", "gameplay", "technology", "utility", "worldgen"]
        }
        content::ContentKind::Shader => &[
            "cartoon",
            "cursed",
            "fantasy",
            "realistic",
            "semi-realistic",
            "vanilla-like",
            "potato",
            "low",
            "medium",
            "high",
            "screenshot",
        ],
        content::ContentKind::Plugin => &[
            "chat",
            "dev-tools",
            "economy",
            "gameplay",
            "management",
            "mechanics",
            "protection",
            "utility",
        ],
        content::ContentKind::Modpack => &[
            "adventure",
            "challenging",
            "combat",
            "expert",
            "fps",
            "hrm",
            "light",
            "multiplayer",
            "optimization",
            "quests",
            "skyblock",
            "small",
            "technology",
        ],
        _ => &[],
    }
}

/// Strip the most common markdown noise so project bodies read cleanly in
/// the plain-text description view.
fn strip_markdown(line: &str) -> String {
    let mut out = line.replace("**", "").replace("*", "").replace("`", "");
    // Collapse markdown links [text](url) to text.
    while let Some(start) = out.find('[') {
        let Some(end_rel) = out[start..].find("](") else {
            break;
        };
        let end = start + end_rel;
        let Some(close) = out[end..].find(')') else {
            break;
        };
        let text = out[start + 1..end].to_string();
        out = format!("{}{}{}", &out[..start], text, &out[end + close + 1..]);
    }
    out
}

/// Whether the content kind is loader-specific (mods are; packs are not).
fn kind_uses_loader(kind: content::ContentKind) -> bool {
    kind == content::ContentKind::Mod
}

/// How the version list is ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VersionSort {
    /// Newest first (by manifest order = Mojang's release order).
    #[default]
    Newest,
    /// By version number descending (numeric-aware).
    Number,
    /// By release date from the manifest, newest first.
    ReleaseDate,
    /// Grouped by loader type, then alphabetically.
    LoaderType,
    /// A → Z.
    Alphabetical,
    /// Z → A.
    AlphabeticalReverse,
}

impl VersionSort {
    fn label(self) -> &'static str {
        match self {
            VersionSort::Newest => "Newest",
            VersionSort::Number => "By number",
            VersionSort::ReleaseDate => "By release date",
            VersionSort::LoaderType => "By loader type",
            VersionSort::Alphabetical => "A → Z",
            VersionSort::AlphabeticalReverse => "Z → A",
        }
    }
}

/// Sort the version rows in place according to `sort`.
pub fn sort_version_rows(rows: &mut [VersionRow], sort: VersionSort) {
    match sort {
        VersionSort::Newest => {
            // The merge keeps manifest order (newest first); stable-sort is
            // a no-op for manifest rows and moves local rows after them.
            rows.sort_by(|a, b| {
                let al = a.loader.is_some() as u8;
                let bl = b.loader.is_some() as u8;
                al.cmp(&bl)
            });
        }
        VersionSort::Number => {
            rows.sort_by(|a, b| cmp_version_desc(&a.name, &b.name));
        }
        VersionSort::ReleaseDate => {
            rows.sort_by(|a, b| {
                let ad = a
                    .remote
                    .as_ref()
                    .map(|r| r.releaseTime.as_str())
                    .unwrap_or("");
                let bd = b
                    .remote
                    .as_ref()
                    .map(|r| r.releaseTime.as_str())
                    .unwrap_or("");
                bd.cmp(ad) // newest first
            });
        }
        VersionSort::LoaderType => {
            rows.sort_by(|a, b| {
                let al = loader_rank(a);
                let bl = loader_rank(b);
                al.cmp(&bl)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
        }
        VersionSort::Alphabetical => {
            rows.sort_by_key(|r| r.name.to_lowercase());
        }
        VersionSort::AlphabeticalReverse => {
            rows.sort_by_key(|r| std::cmp::Reverse(r.name.to_lowercase()));
        }
    }
}

/// Group rank for `By loader type`: Mojang rows first, then by loader.
fn loader_rank(row: &VersionRow) -> (u8, String) {
    match row.loader {
        None => (0, String::new()),
        Some(l) => (1, l.slug().to_string()),
    }
}

/// Numeric-aware descending comparison (`1.21.10` > `1.21.9`).
fn cmp_version_desc(a: &str, b: &str) -> std::cmp::Ordering {
    cmp_version_parts_asc(a, b).reverse()
}

fn cmp_version_parts_asc(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ia = a.split(['.', '-']);
    let mut ib = b.split(['.', '-']);
    loop {
        match (ia.next(), ib.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let xn = x.parse::<u64>().ok();
                let yn = y.parse::<u64>().ok();
                let ord = match (xn, yn) {
                    (Some(xn), Some(yn)) => xn.cmp(&yn),
                    _ => x.to_lowercase().cmp(&y.to_lowercase()),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// UI state of one mod-platform tab.
#[derive(Default)]
struct ContentUi {
    kind: content::ContentKind,
    search: String,
    loader_filter: Option<updater::Loader>,
    /// Category tags chosen for the current kind (OR-combined).
    category_filter: Vec<String>,
    /// License short name filter (Modrinth).
    license_filter: Option<String>,
    /// Search sort order.
    sort: content::SortIndex,
    /// Search results, loaded lazily.
    results: Option<Result<Vec<content::ContentItem>, String>>,
    loading: bool,
    /// The project whose detail view is open.
    open_project: Option<String>,
    files: BTreeMap<String, Option<Result<Vec<content::ContentFile>, String>>>,
    files_loading: bool,
    /// The loaded project page (description body, links).
    detail: BTreeMap<String, Option<Result<content::ProjectDetail, String>>>,
    /// Selected inner tab of the open project (0=Description, 1=Changelog,
    /// 2=Versions).
    detail_tab: usize,
}

impl ContentUi {
    fn kind_slot(&mut self) -> &mut content::ContentKind {
        &mut self.kind
    }

    fn search_slot(&mut self) -> &mut String {
        &mut self.search
    }

    fn loader_slot(&mut self) -> &mut Option<updater::Loader> {
        &mut self.loader_filter
    }

    /// An owned read-only snapshot of the render-relevant state; lets the
    /// egui closures read it while `self` is borrowed for spawn_job.
    fn snapshot(&self) -> ContentSnapshot {
        ContentSnapshot {
            kind: self.kind,
            sort: self.sort,
            category_filter: self.category_filter.clone(),
            license_filter: self.license_filter.clone(),
            results: self.results.as_ref().map(|r| match r {
                Ok(items) => Ok(items.clone()),
                Err(e) => Err(e.clone()),
            }),
            open_project: self.open_project.clone(),
            files: clone_files(&self.files),
            detail: clone_details(&self.detail),
            detail_tab: self.detail_tab,
            files_loading: self.files_loading,
        }
    }
}

fn clone_files(
    files: &BTreeMap<String, Option<Result<Vec<content::ContentFile>, String>>>,
) -> BTreeMap<String, Option<Result<Vec<content::ContentFile>, String>>> {
    files
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                v.as_ref().map(|r| match r {
                    Ok(files) => Ok(files.clone()),
                    Err(e) => Err(e.clone()),
                }),
            )
        })
        .collect()
}

fn clone_details(
    details: &BTreeMap<String, Option<Result<content::ProjectDetail, String>>>,
) -> BTreeMap<String, Option<Result<content::ProjectDetail, String>>> {
    details
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                v.as_ref().map(|r| match r {
                    Ok(d) => Ok(d.clone()),
                    Err(e) => Err(e.clone()),
                }),
            )
        })
        .collect()
}

/// The owned snapshot [`ContentUi::snapshot`] hands to the render pass.
struct ContentSnapshot {
    kind: content::ContentKind,
    sort: content::SortIndex,
    category_filter: Vec<String>,
    #[allow(dead_code)] // read through the live tab, not the snapshot
    license_filter: Option<String>,
    results: Option<Result<Vec<content::ContentItem>, String>>,
    open_project: Option<String>,
    files: BTreeMap<String, Option<Result<Vec<content::ContentFile>, String>>>,
    detail: BTreeMap<String, Option<Result<content::ProjectDetail, String>>>,
    detail_tab: usize,
    files_loading: bool,
}

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
    /// Filter by specific mod loader (Any / Fabric / Forge / Quilt / NeoForge).
    pub version_loader_filter: Option<updater::Loader>,
    /// The order of the version list.
    pub version_sort: VersionSort,
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
    /// The Mojang release chosen directly in the loader installer section.
    pub loader_mc_pick: Option<String>,
    /// Whether the Filters section at the bottom of the Versions tab is open.
    #[allow(dead_code)] // kept for the planned restore of the collapsible section
    pub filters_open: bool,

    // Mod-platform tab (Modrinth).
    modrinth: ContentUi,
    content_downloading: bool,
    content_progress: Arc<Mutex<String>>,
    /// The MC version filter for content searches (empty = any).
    pub content_mc_filter: String,

    // Servers.
    pub server_status: BTreeMap<usize, String>,
    pub new_server_name: String,
    pub new_server_addr: String,
    pub servers_dat_status: String,

    // Accounts.
    pub account_error: Option<String>,
    /// Kind selected in the "Add account" section.
    pub new_account_kind: AccountKind,
    /// Offline-account creation password.
    pub new_account_password: String,
    /// Ely.by login inputs.
    pub online_login_input: String,
    pub online_password_input: String,
    /// In-flight Microsoft device-code sign-in (account creation).
    pub ms_login: Option<MsLoginState>,
    /// In-flight Microsoft re-login (account removal proof).
    pub ms_removal: Option<MsLoginState>,
    /// A background auth/DB job is running (spinner on the Accounts screen).
    pub account_busy: bool,
    /// A Microsoft device-code poll job is in flight.
    pub ms_polling: bool,
    /// Account name awaiting removal confirmation.
    pub account_remove_pending: Option<String>,
    pub account_remove_password: String,
    pub account_remove_login: String,
    pub account_remove_error: Option<String>,

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

/// State of an in-flight Microsoft device-code sign-in.
#[derive(Debug, Clone)]
pub(crate) struct MsLoginState {
    device_code: String,
    user_code: String,
    error: Option<String>,
    /// Set when the user pressed Cancel (consumed by the UI on the next frame).
    cancelled: bool,
}

/// Launcher file locations for background jobs (the executable directory is
/// portable; the DB lives next to the .exe).
struct LauncherPaths;

impl LauncherPaths {
    fn probe() -> PathBuf {
        crate::home::launcher_home().unwrap_or_else(|_| std::env::temp_dir())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    General,
    Console,
    Versions,
    Servers,
    Accounts,
    Skins,
    Modrinth,
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
    /// Set when the row is a mod-loader version (fabric-/quilt-/neoforge-/forge-).
    pub loader: Option<updater::Loader>,
}

/// Detect a mod-loader version by its id: the launcher names loader installs
/// `fabric-loader-<b>-<mc>`, `quilt-loader-…`, `neoforge-<mc>-<b>`,
/// `forge-<mc>-<b>`; other launchers use the same prefixes.
pub fn loader_of(version_name: &str) -> Option<updater::Loader> {
    let name = version_name.to_ascii_lowercase();
    if name.starts_with("fabric") || name.contains("fabric-loader") {
        Some(updater::Loader::Fabric)
    } else if name.starts_with("quilt") || name.contains("quilt-loader") {
        Some(updater::Loader::Quilt)
    } else if name.starts_with("neoforge") {
        Some(updater::Loader::NeoForge)
    } else if name.starts_with("forge") {
        Some(updater::Loader::Forge)
    } else {
        None
    }
}

/// Extract the base Minecraft version from a (possibly loader-prefixed)
/// version id: `fabric-loader-0.19.5-1.21.4` -> `1.21.4`,
/// `neoforge-1.21.4-21.4.157` -> `1.21.4`, `1.20.1` -> `1.20.1`.
pub fn base_mc_of(version_name: &str) -> String {
    let lower = version_name.to_ascii_lowercase();
    // Find the `1.x` segment that starts a Minecraft version: it must be
    // preceded by a dash (or the string start), otherwise `0.21.0` would
    // match the `1.0` inside it.
    fn find_mc_segment(s: &str) -> Option<usize> {
        let bytes = s.as_bytes();
        (0..s.len()).find(|&i| {
            bytes[i] == b'1'
                && i + 1 < bytes.len()
                && bytes[i + 1] == b'.'
                && (i == 0 || bytes[i - 1] == b'-')
        })
    }
    for prefix in ["fabric-loader-", "quilt-loader-"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if let Some(pos) = find_mc_segment(rest) {
                return rest[pos..].to_string();
            }
        }
    }
    for prefix in ["neoforge-", "forge-"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if let Some(pos) = find_mc_segment(rest) {
                // Cut at the loader build: `1.21.4-21.4.157` -> `1.21.4`.
                let tail = &rest[pos..];
                if let Some(dash) = tail.find('-') {
                    return tail[..dash].to_string();
                }
                return tail.to_string();
            }
        }
    }
    version_name.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionFilter {
    All,
    Mojang,
    Loaders,
    Release,
    Snapshot,
    Old,
    Installed,
}

impl VersionFilter {
    fn label(self) -> &'static str {
        match self {
            VersionFilter::All => "All",
            VersionFilter::Mojang => "Mojang",
            VersionFilter::Loaders => "Loaders",
            VersionFilter::Release => "Releases",
            VersionFilter::Snapshot => "Snapshots",
            VersionFilter::Old => "Old",
            VersionFilter::Installed => "Installed",
        }
    }

    fn matches(self, row: &VersionRow) -> bool {
        match self {
            VersionFilter::All => true,
            VersionFilter::Mojang => row.loader.is_none(),
            VersionFilter::Loaders => row.loader.is_some(),
            VersionFilter::Release => row.loader.is_none() && row.kind == "release",
            VersionFilter::Snapshot => row.loader.is_none() && row.kind == "snapshot",
            VersionFilter::Old => {
                row.loader.is_none() && matches!(row.kind.as_str(), "old_alpha" | "old_beta")
            }
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
        let loader = loader_of(&v.name);
        // Loader installs live in their own group; keep manifest kinds only
        // for Mojang rows.
        let kind = if loader.is_some() {
            "loader".to_string()
        } else {
            "local".to_string()
        };
        rows.push(VersionRow {
            name: v.name.clone(),
            kind,
            installed: true,
            is_latest_release: loader.is_none() && v.name == latest,
            remote: None,
            loader,
        });
    }
    if let Some(manifest) = manifest {
        for mv in &manifest.versions {
            if let Some(&i) = index_by_name.get(&mv.id) {
                // Keep the loader grouping for loader rows.
                if rows[i].loader.is_none() {
                    rows[i].kind = mv.kind.clone();
                }
                rows[i].remote = Some(mv.clone());
            } else {
                index_by_name.insert(mv.id.clone(), rows.len());
                rows.push(VersionRow {
                    name: mv.id.clone(),
                    kind: mv.kind.clone(),
                    installed: false,
                    is_latest_release: mv.id == latest,
                    remote: Some(mv.clone()),
                    loader: None,
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
        app.accounts.select_saved(&app.settings.username);
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

    /// Persist the current account selection (the store itself writes to the
    /// SQLite DB on every mutation; only the selected name lives in settings).
    fn save_accounts(&mut self) {
        self.settings.username = self
            .accounts
            .current()
            .map(|a| a.username.clone())
            .unwrap_or_default();
        self.save_settings();
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
            // Padding inside the frame; used to compute the real text width.
            let inner_pad = 16.0;
            // Width available for text between the icon column and the ✕.
            let text_width = 360.0_f32 - inner_pad - 26.0 - 24.0;

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
                    // Hard-wrap the title into at most 2 lines with an
                    // ellipsis on the overflow — a very long single-line
                    // error message must never widen the card.
                    ui.set_min_width(text_width);
                    ui.set_max_width(text_width);
                    let title_lines = collapse_detail(&toast.title, 2);
                    for line in &title_lines {
                        ui.add(
                            egui::Label::new(egui::RichText::new(line).strong())
                                .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                    }
                    if let Some(detail) = &toast.detail {
                        let mono = egui::FontId::monospace(10.0);
                        for line in collapse_detail(detail, 3) {
                            let shown = ellipsize_line(ui, &line, text_width, &mono);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(shown).monospace().small().weak(),
                                )
                                .wrap_mode(egui::TextWrapMode::Truncate),
                            );
                        }
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
                    let galley = ui.painter().layout(
                        log.to_string(),
                        egui::FontId::monospace(10.0),
                        egui::Color32::from_rgb(0xC8, 0xC8, 0xC8),
                        (log_rect.width() - 8.0).max(40.0),
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

/// Collapse a multi-line detail to at most `max_lines`: everything past
/// the cap is replaced by a single "…" line, so the collapsed toast stays
/// a fixed size no matter how long the message is.
fn collapse_detail(detail: &str, max_lines: usize) -> Vec<String> {
    let lines: Vec<&str> = detail.lines().collect();
    if lines.len() <= max_lines {
        return lines.iter().map(|s| s.to_string()).collect();
    }
    let mut out: Vec<String> = lines[..max_lines.saturating_sub(1)]
        .iter()
        .map(|s| s.to_string())
        .collect();
    out.push("…".to_string());
    out
}

/// Trim one line so it fits `max_width` in the given font, appending "…".
fn ellipsize_line(ui: &egui::Ui, line: &str, max_width: f32, font: &egui::FontId) -> String {
    let color = ui.visuals().text_color();
    if ui
        .painter()
        .layout_no_wrap(line.to_string(), font.clone(), color)
        .size()
        .x
        <= max_width
    {
        return line.to_string();
    }
    // Binary search the longest prefix that fits, then append the ellipsis.
    let bytes = line.as_bytes();
    let mut lo = 0usize;
    let mut hi = bytes.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if !line.is_char_boundary(mid) {
            hi = mid - 1;
            continue;
        }
        let w = ui
            .painter()
            .layout_no_wrap(line[..mid].to_string(), font.clone(), color)
            .size()
            .x;
        if w <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut end = lo;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    // Make room for the ellipsis itself.
    let ell = "…";
    let ell_w = ui
        .painter()
        .layout_no_wrap(ell.to_string(), font.clone(), color)
        .size()
        .x;
    while end > 0 {
        let cand = &line[..end];
        let w = ui
            .painter()
            .layout_no_wrap(format!("{cand}{ell}"), font.clone(), color)
            .size()
            .x;
        if w <= max_width {
            return format!("{cand}{ell}");
        }
        end -= 1;
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        let _ = ell_w;
    }
    ell.to_string()
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

/// A material-style magnifying-glass icon (search), stroked in the text
/// color so it sits naturally in front of a text input.
fn draw_search_icon(ui: &mut egui::Ui, height: f32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(height, height), egui::Sense::hover());
    let p = ui.painter_at(rect);

    // Lens circle, offset toward the top-left.
    let lens_r = height * 0.30;
    let lens_c = egui::pos2(rect.left() + height * 0.40, rect.top() + height * 0.40);
    // The handle starts on the lens rim toward the bottom-right.
    let handle_dir = std::f32::consts::SQRT_2 / 2.0;
    let handle_start = egui::pos2(
        lens_c.x + lens_r * handle_dir,
        lens_c.y + lens_r * handle_dir,
    );
    let handle_end = egui::pos2(rect.right() - height * 0.10, rect.bottom() - height * 0.10);

    p.circle_stroke(lens_c, lens_r, egui::Stroke::new(height * 0.10, color));
    p.line_segment(
        [handle_start, handle_end],
        egui::Stroke::new(height * 0.10, color),
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
                (Screen::Modrinth, "⬢  Modrinth"),
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

        if self.account_remove_pending.is_some() {
            self.show_account_removal(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.screen {
            Screen::General => self.ui_general(ui),
            Screen::Console => self.ui_console(ui),
            Screen::Versions => self.ui_versions(ui),
            Screen::Servers => self.ui_servers(ui),
            Screen::Accounts => self.ui_accounts(ui),
            Screen::Skins => self.ui_skins(ui, ctx),
            Screen::Modrinth => self.ui_content(ui, ContentPlatform::Modrinth),
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

        // ── Filters section (top level, above the list) ──
        ui.strong("Filters");
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
                ui.selectable_value(&mut self.version_filter, filter, filter.label());
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.weak("Loader:");
            ui.selectable_value(&mut self.version_loader_filter, None, "Any");
            for l in updater::Loader::ALL {
                ui.selectable_value(&mut self.version_loader_filter, Some(l), l.label());
            }
        });
        ui.horizontal(|ui| {
            draw_search_icon(ui, 16.0, ui.visuals().text_color());
            ui.add(
                egui::TextEdit::singleline(&mut self.version_search)
                    .hint_text("Search…")
                    .desired_width(200.0),
            );
            if ui.button("Rescan").clicked() {
                self.reload_versions();
            }
            if ui.button("Refresh manifest").clicked() {
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
            ui.label(&progress);
            ui.add(egui::ProgressBar::new(1.0).animate(true).text("working…"));
            ui.separator();
        }
        egui::CollapsingHeader::new("Install a mod loader")
            .id_salt("loader_installer")
            .default_open(false)
            .show(ui, |ui| {
                self.ui_loader_row(ui, installing);
            });
        ui.separator();

        if let Some(Err(e)) = &self.manifest {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("Manifest unavailable ({e}) — showing local versions only"),
            );
        }

        // ── Sort bar (same level as the list) ──
        ui.horizontal(|ui| {
            ui.weak("Sort:");
            egui::ComboBox::from_id_salt("version_sort")
                .selected_text(self.version_sort.label())
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
                        ui.selectable_value(&mut self.version_sort, s, s.label());
                    }
                });
            let total = self.versions.len();
            ui.weak(format!("{} installed", total));
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

        ui.weak(format!(
            "selected: {}",
            if self.settings.selected_version.is_empty() {
                "— none —"
            } else {
                &self.settings.selected_version
            }
        ));

        // The version list takes all remaining space.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in shown.iter() {
                    self.version_row_ui(ui, row, installing);
                }
                if shown.is_empty() {
                    ui.weak("No versions match the current filter.");
                }
            });
    }

    /// Render one row of the version list (Select / Install controls).
    fn version_row_ui(&mut self, ui: &mut egui::Ui, row: &VersionRow, installing: bool) {
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
                ui.weak(format!("[{}]", row.kind));
            }
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
        ui.strong("Install a mod loader");
        ui.add_space(2.0);

        // The game root must be configured; the installer writes into
        // `<game dir>/versions/…`.
        let game_dir = resolve_game_dir(&self.settings);
        let game_dir_ok = !self.settings.game_directory.trim().is_empty() || game_dir.exists();
        if !game_dir_ok {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!(
                    "Root game directory is not set — configure it in Settings; installs would go to {}",
                    game_dir.display()
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
            ui.weak("Load the Mojang manifest (Refresh manifest) to pick a version.");
            return;
        }
        let mc_selected = self
            .loader_mc_pick
            .clone()
            .unwrap_or_else(|| releases[0].clone());
        egui::ComboBox::from_label("Minecraft")
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
                            !installing && game_dir_ok,
                            egui::Button::new(format!(
                                "Install {} {} on {mc}",
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

    /// The Accounts screen: three account kinds with their own creation and
    /// removal proofs — Offline (nickname + password, Argon2id in the DB),
    /// Ely.by (username + password against the Yggdrasil authserver) and
    /// Mojang/Microsoft (OAuth device-code flow).
    fn ui_accounts(&mut self, ui: &mut egui::Ui) {
        ui.heading("Accounts");
        ui.add_space(4.0);

        // ── Add account ──────────────────────────────────────────
        ui.group(|ui| {
            ui.strong("Add account");
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                for kind in auth::AccountKind::ALL {
                    ui.selectable_value(&mut self.new_account_kind, *kind, kind.label());
                }
            });
            match self.new_account_kind {
                AccountKind::Offline => {
                    ui.horizontal(|ui| {
                        ui.label("Nickname");
                        let name = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.username_input)
                                    .hint_text("3-16 chars")
                                    .desired_width(140.0),
                            )
                            .lost_focus();
                        ui.label("Password");
                        let pass = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.new_account_password)
                                    .password(true)
                                    .hint_text("required")
                                    .desired_width(140.0),
                            )
                            .lost_focus();
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.button("Create").clicked() || ((name || pass) && enter))
                            && !self.account_busy
                        {
                            let name = self.username_input.trim().to_string();
                            let password = self.new_account_password.clone();
                            self.account_busy = true;
                            self.spawn_job(
                                move || {
                                    // Argon2id hashing is intentionally slow;
                                    // keep it off the UI thread. The insert goes
                                    // through the DB so any store can do it; the
                                    // live in-memory store is refreshed afterwards.
                                    let mut store = AccountStore::load(&LauncherPaths::probe());
                                    store.add_offline(&name, &password)
                                },
                                move |app, result| {
                                    app.account_busy = false;
                                    app.finish_account_change(result);
                                },
                            );
                        }
                    });
                    ui.weak(
                        "The password is stored as an Argon2id hash in the launcher's database.",
                    );
                }
                AccountKind::ElyBy => {
                    ui.horizontal(|ui| {
                        ui.label("Ely.by email / login");
                        let user = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.online_login_input)
                                    .hint_text("you@example.com")
                                    .desired_width(180.0),
                            )
                            .lost_focus();
                        ui.label("Password");
                        let pass = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.online_password_input)
                                    .password(true)
                                    .desired_width(140.0),
                            )
                            .lost_focus();
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.button("Log in").clicked() || ((user || pass) && enter))
                            && !self.account_busy
                        {
                            let user = self.online_login_input.trim().to_string();
                            let password = self.online_password_input.clone();
                            self.account_busy = true;
                            self.spawn_job(
                                move || auth::login_elyby(&user, &password),
                                move |app, result| {
                                    app.account_busy = false;
                                    app.finish_online_login(result);
                                },
                            );
                        }
                    });
                    ui.weak(
                        "Logs in against authserver.ely.by; the session token is stored in the DB.",
                    );
                }
                AccountKind::Mojang => {
                    ui.label("Sign in with a Microsoft account that owns Minecraft: Java Edition.");
                    if self.ms_login.is_none()
                        && !self.account_busy
                        && ui.button("Start Microsoft sign-in").clicked()
                    {
                        self.account_busy = true;
                        self.spawn_job(
                            move || {
                                auth::microsoft_begin(&crate::net::agent())
                                    .map_err(|e| e.to_string())
                            },
                            move |app, result| match result {
                                Ok((device_code, user_code, _expires, _interval)) => {
                                    app.account_busy = false;
                                    app.ms_login = Some(MsLoginState {
                                        device_code,
                                        user_code: user_code.clone(),
                                        error: None,
                                        cancelled: false,
                                    });
                                    app.notify_info(format!(
                                        "Enter the code {user_code} at microsoft.com/link"
                                    ));
                                }
                                Err(e) => {
                                    app.account_busy = false;
                                    app.notify_error("MS-BEGIN", e);
                                }
                            },
                        );
                    }
                    let ms_state = self.ms_login.take();
                    if let Some(mut state) = ms_state {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("Go to");
                                ui.hyperlink("https://www.microsoft.com/link");
                                ui.add_space(8.0);
                                ui.label("and enter code:");
                                ui.label(
                                    egui::RichText::new(&state.user_code)
                                        .strong()
                                        .monospace()
                                        .size(18.0),
                                );
                                if ui.button("Copy").clicked() {
                                    ui.ctx().copy_text(state.user_code.clone());
                                }
                            });
                            ui.weak("Waiting for you to finish in the browser…");
                            if let Some(err) = &state.error {
                                ui.colored_label(egui::Color32::LIGHT_RED, err);
                            }
                            if ui.button("Cancel").clicked() {
                                state.cancelled = true;
                            }
                        });

                        if state.cancelled {
                            self.ms_login = None;
                        } else {
                            // Put the state back; the poll job below updates it.
                            self.ms_login = Some(state);

                            // Poll in the background; the result arrives via a job.
                            if !self.account_busy && !self.ms_polling {
                                let device_code = self
                                    .ms_login
                                    .as_ref()
                                    .map(|s| s.device_code.clone())
                                    .unwrap_or_default();
                                self.ms_polling = true;
                                self.spawn_job(
                                    move || {
                                        auth::microsoft_poll(&crate::net::agent(), &device_code)
                                            .map_err(|e| e.to_string())
                                    },
                                    move |app, result| {
                                        app.ms_polling = false;
                                        match result {
                                            Ok(Some(login)) => {
                                                app.ms_login = None;
                                                app.ms_polling = false;
                                                // Add to the live store directly: the
                                                // HTTP work is done, the insert is a
                                                // fast local SQLite write.
                                                let result = app.accounts.add_mojang(
                                                    &login.username,
                                                    &login.uuid,
                                                    &login.access_token,
                                                    &login.refresh_token,
                                                );
                                                app.finish_account_change(result);
                                            }
                                            Ok(None) => {} // still waiting; poll again next frames
                                            Err(e) => {
                                                if let Some(state) = &mut app.ms_login {
                                                    state.error = Some(e);
                                                }
                                            }
                                        }
                                    },
                                );
                            }
                        }
                    }
                }
            }
            if self.account_busy {
                ui.spinner();
                ui.weak("working…");
            }
            if let Some(error) = &self.account_error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        });

        ui.add_space(6.0);

        // ── Account list ─────────────────────────────────────────
        ui.strong("Your accounts");
        ui.add_space(2.0);
        let rows: Vec<(AccountKind, String)> = self
            .accounts
            .accounts
            .iter()
            .map(|a| (a.kind, a.username.clone()))
            .collect();
        for (kind, name) in &rows {
            ui.horizontal(|ui| {
                let selected = self.accounts.current.as_deref() == Some(name.as_str());
                if ui.radio(selected, name).clicked() {
                    self.accounts.select(name);
                    self.save_accounts();
                }
                ui.weak(kind.label());
                if ui.small_button("Remove").clicked() {
                    // Removal always asks for proof: the password for offline
                    // accounts, a fresh login for online ones.
                    self.account_remove_pending = Some(name.clone());
                    self.account_remove_password.clear();
                    self.account_remove_error = None;
                }
            });
        }
        if rows.is_empty() {
            ui.weak("No accounts yet — create one above.");
        }
    }

    /// Common tail of every successful account mutation: refresh selection
    /// state, clear inputs, drop the error.
    fn finish_account_change(&mut self, result: Result<String>) {
        match result {
            Ok(name) => {
                self.username_input.clear();
                self.new_account_password.clear();
                self.account_error = None;
                // Background jobs add the account to a throwaway store loaded
                // from the DB; reload the in-memory copy so the new account
                // actually shows up in the list and can be selected.
                self.accounts = AccountStore::load(&self.home_dir);
                self.accounts.select(&name);
                self.save_accounts();
                self.notify_info(format!("Account {name} added"));
            }
            Err(e) => self.account_error = Some(e.to_string()),
        }
    }

    /// Store an online login result (Ely.by or Microsoft) as an account.
    /// Runs on the UI thread: the HTTP work already happened in the job; the
    /// store write is a fast local SQLite insert.
    fn finish_online_login(&mut self, result: Result<auth::Account>) {
        match result {
            Ok(account) => {
                self.online_password_input.clear();
                // Mutate the live store directly so the new account is in the
                // list immediately (and persisted by the store itself).
                let result = match account.kind {
                    AccountKind::ElyBy => self.accounts.add_elyby(
                        &account.username,
                        &account.uuid,
                        &account.access_token,
                    ),
                    AccountKind::Mojang => self.accounts.add_mojang(
                        &account.username,
                        &account.uuid,
                        &account.access_token,
                        "",
                    ),
                    AccountKind::Offline => unreachable!(),
                };
                self.finish_account_change(result);
            }
            Err(e) => {
                self.account_error = Some(e.to_string());
            }
        }
    }

    /// The removal dialog: password proof for offline accounts, re-login for
    /// online ones. Mirrors the Stop/Kill confirmation styling.
    fn show_account_removal(&mut self, ctx: &egui::Context) {
        let Some(name) = self.account_remove_pending.clone() else {
            return;
        };
        let rec = self.accounts.accounts.iter().find(|a| a.username == name);
        let kind = rec.map(|r| r.kind);
        let screen = ctx.screen_rect();

        egui::Area::new(egui::Id::new("account_remove_dim"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.left_top())
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
                if resp.clicked() {
                    self.account_remove_pending = None;
                }
            });

        egui::Area::new(egui::Id::new("account_remove_dialog"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::window(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        draw_warning_triangle(ui, 36.0);
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(format!("Remove {name}?"))
                                    .strong()
                                    .size(16.0),
                            );
                            match kind {
                                Some(AccountKind::Offline) => {
                                    ui.label("Enter the account password to confirm removal.");
                                }
                                Some(AccountKind::ElyBy) => {
                                    ui.label("Sign in to Ely.by again to confirm removal.");
                                }
                                Some(AccountKind::Mojang) => {
                                    ui.label("Sign in with Microsoft again to confirm removal.");
                                }
                                None => {}
                            }
                        });
                    });
                    ui.add_space(10.0);

                    let confirmed = match kind {
                        Some(AccountKind::Offline) => {
                            ui.horizontal(|ui| {
                                ui.label("Password");
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut self.account_remove_password)
                                        .password(true)
                                        .desired_width(200.0),
                                );
                                let enter =
                                    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                enter
                                    && self
                                        .accounts
                                        .verify_offline_password(&name, self.account_remove_password.trim())
                            });
                            if let Some(err) = &self.account_remove_error {
                                ui.colored_label(egui::Color32::LIGHT_RED, err);
                            }
                            let ok = !self.account_remove_password.trim().is_empty()
                                && self
                                    .accounts
                                    .verify_offline_password(&name, self.account_remove_password.trim());
                            let mut clicked = false;
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                clicked = ui
                                    .add_enabled(
                                        ok,
                                        egui::Button::new(egui::RichText::new("Confirm").strong()),
                                    )
                                    .clicked();
                            });
                            ok && clicked
                        }
                        Some(AccountKind::ElyBy) => {
                            ui.horizontal(|ui| {
                                ui.label("Ely.by login");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.account_remove_login)
                                        .desired_width(180.0),
                                );
                                ui.label("Password");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.account_remove_password)
                                        .password(true)
                                        .desired_width(140.0),
                                );
                            });
                            if let Some(err) = &self.account_remove_error {
                                ui.colored_label(egui::Color32::LIGHT_RED, err);
                            }
                            let ready = !self.account_remove_login.trim().is_empty()
                                && !self.account_remove_password.is_empty();
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui
                                    .add_enabled(
                                        ready,
                                        egui::Button::new(egui::RichText::new("Confirm").strong()),
                                    )
                                    .clicked()
                                {
                                    self.account_remove_error = None;
                                    let user = self.account_remove_login.trim().to_string();
                                    let password = self.account_remove_password.clone();
                                    let name = name.clone();
                                    self.spawn_job(
                                        move || auth::login_elyby(&user, &password),
                                        move |app, res| match res {
                                            Ok(acc) => {
                                                if acc.username == name {
                                                    app.remove_account_confirmed(&name);
                                                } else {
                                                    app.account_remove_error = Some(
                                                        "that login belongs to another account".into(),
                                                    );
                                                }
                                            }
                                            Err(e) => app.account_remove_error = Some(e.to_string()),
                                        },
                                    );
                                }
                            });
                            false
                        }
                        Some(AccountKind::Mojang) => {
                            ui.label("A Microsoft sign-in window will open. Complete it to remove this account.");
                            if ui.button("Start Microsoft sign-in").clicked() {
                                let name = name.clone();
                                self.account_busy = true;
                                self.spawn_job(
                                    move || {
                                        auth::microsoft_begin(&crate::net::agent())
                                            .map_err(|e| e.to_string())
                                    },
                                    move |app, result| match result {
                                        Ok((device_code, user_code, _, _)) => {
                                            app.account_busy = false;
                                            app.account_remove_pending = Some(name.clone());
                                            app.ms_removal = Some(MsLoginState {
                                                device_code,
                                                user_code: user_code.clone(),
                                                error: None,
                                                cancelled: false,
                                            });
                                            app.notify_info(format!(
                                                "Enter the code {user_code} at microsoft.com/link"
                                            ));
                                        }
                                        Err(e) => {
                                            app.account_busy = false;
                                            app.account_remove_error = Some(e);
                                        }
                                    },
                                );
                            }
                            if let Some(err) = &self.account_remove_error {
                                ui.colored_label(egui::Color32::LIGHT_RED, err);
                            }
                            false
                        }
                        None => false,
                    };
                    if confirmed {
                        self.remove_account_confirmed(&name);
                    }

                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            self.account_remove_pending = None;
                            self.ms_removal = None;
                        }
                    });
                });
            });

        // The Microsoft re-login for removal shares the device-code poller.
        if self.ms_removal.is_some() && !self.account_busy && !self.ms_polling {
            let device_code = self
                .ms_removal
                .as_ref()
                .map(|s| s.device_code.clone())
                .unwrap_or_default();
            let name = name.clone();
            self.ms_polling = true;
            self.spawn_job(
                move || {
                    auth::microsoft_poll(&crate::net::agent(), &device_code)
                        .map_err(|e| e.to_string())
                },
                move |app, result| {
                    app.ms_polling = false;
                    match result {
                        Ok(Some(login)) => {
                            if login.username == name {
                                app.ms_removal = None;
                                app.remove_account_confirmed(&name);
                            } else {
                                if let Some(state) = &mut app.ms_removal {
                                    state.error =
                                        Some("that Microsoft account is not this account".into());
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            if let Some(state) = &mut app.ms_removal {
                                state.error = Some(e);
                            }
                        }
                    }
                },
            );
        }
    }

    /// Remove the account (proof already collected). Falls back to the first
    /// remaining account when the removed one was selected.
    fn remove_account_confirmed(&mut self, name: &str) {
        let _ = self.accounts.remove(name);
        // Re-sync with the DB so the in-memory list matches what is stored.
        self.accounts = AccountStore::load(&self.home_dir);
        self.account_remove_pending = None;
        self.account_remove_password.clear();
        self.account_remove_login.clear();
        self.account_remove_error = None;
        self.save_accounts();
        self.notify_info(format!("Account {name} removed"));
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

    /// The Modrinth tab: search content, pick a file, install it into the
    /// game directory. Tab state is accessed via `self.tab(platform)` to keep
    /// borrow conflicts out of the render closures.
    fn ui_content(&mut self, ui: &mut egui::Ui, platform: ContentPlatform) {
        ui.heading("Modrinth");
        ui.add_space(4.0);

        // Content kind tabs: mods are the default, the rest follow the
        // Modrinth project types.
        ui.horizontal_wrapped(|ui| {
            for kind in [
                content::ContentKind::Mod,
                content::ContentKind::ResourcePack,
                content::ContentKind::DataPack,
                content::ContentKind::Shader,
                content::ContentKind::Modpack,
                content::ContentKind::Plugin,
                content::ContentKind::Server,
                content::ContentKind::World,
            ] {
                ui.selectable_value(self.tab(platform).kind_slot(), kind, kind.label());
            }
        });
        ui.horizontal(|ui| {
            draw_search_icon(ui, 16.0, ui.visuals().text_color());
            let response = ui.add(
                egui::TextEdit::singleline(self.tab(platform).search_slot())
                    .hint_text("Search…")
                    .desired_width(260.0),
            );
            let target_mc = base_mc_of(&self.settings.selected_version);
            ui.weak(format!(
                "MC: {}",
                if target_mc.is_empty() {
                    "—"
                } else {
                    &target_mc
                }
            ));
            // Loader filter (mods only; Modrinth has loader facets).
            if kind_uses_loader(self.tab(platform).kind) {
                ui.separator();
                let loader_filter = self.tab(platform).loader_filter;
                ui.selectable_value(self.tab(platform).loader_slot(), None, "any loader");
                for l in updater::Loader::ALL {
                    ui.selectable_value(self.tab(platform).loader_slot(), Some(l), l.label());
                }
                let _ = loader_filter;
            }
            let search_pressed = ui.button("Search").clicked()
                || response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if search_pressed {
                let tab = self.tab(platform);
                tab.results = None;
                tab.open_project = None;
                tab.files.clear();
            }
        });

        let installing = self.installing.load(Ordering::SeqCst) || self.content_downloading;
        let progress = self
            .content_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if installing && !progress.is_empty() {
            ui.label(&progress);
        }
        ui.separator();

        // Lazily run the search.
        if self.tab(platform).results.is_none() && !self.tab(platform).loading {
            let kind = self.tab(platform).kind;
            let query = self.tab(platform).search.clone();
            let loader = self
                .tab(platform)
                .loader_filter
                .map(|l| l.slug().to_string());
            let mc = self.content_mc_filter.clone();
            let categories: Vec<String> = self.tab(platform).category_filter.clone();
            let license = self.tab(platform).license_filter.clone();
            let sort = self.tab(platform).sort;
            self.tab(platform).loading = true;
            self.spawn_job(
                move || {
                    content::search_modrinth(
                        &crate::net::agent(),
                        kind,
                        &query,
                        &mc,
                        loader.as_deref(),
                        &categories,
                        license.as_deref(),
                        sort,
                        30,
                    )
                    .map_err(|e| e.to_string())
                },
                move |app, result| {
                    let tab = app.tab(platform);
                    tab.results = Some(result);
                    tab.loading = false;
                },
            );
        }

        // Read-only snapshot of the state needed to render the results.
        let state = self.tab(platform).snapshot();
        let Some(results) = &state.results else {
            ui.weak("Type a query and press Search.");
            return;
        };
        let items = match results {
            Err(e) => {
                ui.colored_label(egui::Color32::YELLOW, e.to_string());
                return;
            }
            Ok(items) if items.is_empty() => {
                ui.weak("Nothing found.");
                return;
            }
            Ok(items) => items,
        };

        // Filter bar above the results: MC version, categories, license and
        // sort (Modrinth facets; CurseForge gets MC only).
        ui.horizontal_wrapped(|ui| {
            ui.weak("MC:");
            let mc_w = ui.available_width() * 0.13;
            ui.add(
                egui::TextEdit::singleline(&mut self.content_mc_filter)
                    .hint_text("any")
                    .desired_width(mc_w),
            );
            if platform == ContentPlatform::Modrinth {
                ui.separator();
                ui.weak("Sort:");
                egui::ComboBox::from_id_salt("content_sort")
                    .selected_text(state.sort.label())
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for s in [
                            content::SortIndex::Relevance,
                            content::SortIndex::Downloads,
                            content::SortIndex::Follows,
                            content::SortIndex::Newest,
                            content::SortIndex::Updated,
                        ] {
                            ui.selectable_value(&mut self.tab(platform).sort, s, s.label());
                        }
                    });
                if ui.button("Apply").clicked() {
                    self.tab(platform).results = None; // re-run the search
                }
                ui.separator();
                ui.weak("License:");
                egui::ComboBox::from_id_salt("content_license")
                    .selected_text(
                        self.tab(platform)
                            .license_filter
                            .clone()
                            .unwrap_or_else(|| "any".into()),
                    )
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.tab(platform).license_filter, None, "any");
                        for lic in COMMON_LICENSES {
                            ui.selectable_value(
                                &mut self.tab(platform).license_filter,
                                Some(lic.to_string()),
                                *lic,
                            );
                        }
                    });
                // Category chips for the current kind.
                let cats = categories_for(state.kind);
                if !cats.is_empty() {
                    ui.separator();
                    for cat in cats {
                        let selected = state.category_filter.iter().any(|c| c == cat);
                        if ui.selectable_label(selected, *cat).clicked() {
                            let tab = self.tab(platform);
                            if selected {
                                tab.category_filter.retain(|c| c != cat);
                            } else {
                                tab.category_filter.push(cat.to_string());
                            }
                            tab.results = None;
                        }
                    }
                }
                if ui.button("Clear").clicked() {
                    let tab = self.tab(platform);
                    tab.category_filter.clear();
                    tab.license_filter = None;
                    tab.results = None;
                }
            }
        });
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for item in items {
                let open = state.open_project.as_deref() == Some(item.id.as_str());
                // Card header: icon + title + author + stats.
                ui.horizontal(|ui| {
                    let header = egui::CollapsingHeader::new(egui::RichText::new(format!(
                        "{}   ·   ↓ {}  ·  ♥ {}  ·  [{}]",
                        item.title, item.downloads, item.follows, item.license
                    )))
                    .id_salt((platform_slug(platform), item.id.as_str()))
                    .default_open(open)
                    .show(ui, |ui| {
                        ui.weak(format!("by {}", item.author));
                        ui.label(&item.description);
                        if !item.categories.is_empty() {
                            ui.weak(item.categories.join(" · "));
                        }

                        // The project detail tabs (Modrinth only: body page).
                        if platform == ContentPlatform::Modrinth {
                            self.content_detail_tabs(ui, platform, &item.id, &state);
                        }

                        // Versions + download buttons.
                        ui.strong("Versions");
                        if !state.files.contains_key(&item.id) && !state.files_loading {
                            let id = item.id.clone();
                            let id_task = item.id.clone();
                            let mc = self.content_mc_filter.trim().to_string();
                            let loader = self
                                .tab(platform)
                                .loader_filter
                                .map(|l| l.slug().to_string());
                            self.tab(platform).files_loading = true;
                            self.spawn_job(
                                move || {
                                    content::modrinth_versions(
                                        &crate::net::agent(),
                                        &id_task,
                                        &mc,
                                        loader.as_deref(),
                                    )
                                    .map_err(|e| e.to_string())
                                },
                                move |app, result| {
                                    let tab = app.tab(platform);
                                    tab.files.insert(id, Some(result));
                                    tab.files_loading = false;
                                },
                            );
                        }
                        match state.files.get(&item.id) {
                            None => {
                                ui.weak("Loading versions…");
                            }
                            Some(None) => {
                                ui.weak("No versions for this MC version.");
                            }
                            Some(Some(Err(e))) => {
                                ui.colored_label(egui::Color32::YELLOW, e.to_string());
                            }
                            Some(Some(Ok(files))) => {
                                for file in files.iter().take(20) {
                                    ui.horizontal(|ui| {
                                        ui.monospace(&file.name);
                                        ui.weak(format!(
                                            "{:.1} MB",
                                            file.size as f32 / 1_048_576.0
                                        ));
                                        let label = if installing { "…" } else { "Download" };
                                        if ui
                                            .add_enabled(!installing, egui::Button::new(label))
                                            .clicked()
                                        {
                                            self.download_content(
                                                state.kind,
                                                item.clone(),
                                                file.clone(),
                                            );
                                        }
                                    });
                                }
                            }
                        }
                    });
                    if header.header_response.clicked() {
                        let tab = self.tab(platform);
                        tab.open_project = if open { None } else { Some(item.id.clone()) };
                    }
                });
                ui.separator();
            }
        });
    }

    /// The inner tabs of an open project: Description / Changelog (from the
    /// latest version) / project page body.
    fn content_detail_tabs(
        &mut self,
        ui: &mut egui::Ui,
        platform: ContentPlatform,
        project_id: &str,
        state: &ContentSnapshot,
    ) {
        // Lazily fetch the project page.
        if !state.detail.contains_key(project_id) {
            let id_task = project_id.to_string();
            let id_key = project_id.to_string();
            self.spawn_job(
                move || {
                    content::modrinth_project(&crate::net::agent(), &id_task)
                        .map_err(|e| e.to_string())
                },
                move |app, result| {
                    app.tab(platform).detail.insert(id_key, Some(result));
                },
            );
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            for (i, label) in ["Description", "Links"].iter().enumerate() {
                if ui.selectable_label(state.detail_tab == i, *label).clicked() {
                    self.tab(platform).detail_tab = i;
                }
            }
        });
        match state.detail.get(project_id) {
            None => {
                ui.weak("Loading project page…");
            }
            Some(None) => {}
            Some(Some(Err(e))) => {
                ui.colored_label(egui::Color32::YELLOW, e.to_string());
            }
            Some(Some(Ok(detail))) => match state.detail_tab {
                1 => {
                    ui.horizontal_wrapped(|ui| {
                        if !detail.source_url.is_empty() {
                            ui.hyperlink_to("Source", &detail.source_url);
                        }
                        if !detail.issues_url.is_empty() {
                            ui.hyperlink_to("Issues", &detail.issues_url);
                        }
                        if !detail.wiki_url.is_empty() {
                            ui.hyperlink_to("Wiki", &detail.wiki_url);
                        }
                    });
                    ui.weak(format!(
                        "Updated: {} · {} game versions",
                        detail.date_updated,
                        detail.game_versions.len()
                    ));
                }
                _ => {
                    // The markdown body, rendered as plain text paragraphs.
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for line in detail.body.lines() {
                                let line = line.trim();
                                if line.is_empty() {
                                    ui.add_space(2.0);
                                } else if line.starts_with("#") {
                                    ui.strong(line.trim_start_matches('#').trim());
                                } else {
                                    ui.label(strip_markdown(line));
                                }
                            }
                        });
                }
            },
        }
    }
    fn tab(&mut self, platform: ContentPlatform) -> &mut ContentUi {
        match platform {
            ContentPlatform::Modrinth => &mut self.modrinth,
        }
    }

    /// Download one content file in the background.
    fn download_content(
        &mut self,
        kind: content::ContentKind,
        item: content::ContentItem,
        file: content::ContentFile,
    ) {
        self.content_downloading = true;
        *self
            .content_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = format!("downloading {}…", file.file_name);
        let game_dir = resolve_game_dir(&self.settings);
        // Data packs, shaders, plugins and server jars land in the user's
        // Downloads folder; mods and resource packs go into the game dir.
        let game_dir = if kind.goes_to_downloads() {
            dirs::download_dir().unwrap_or(game_dir)
        } else {
            game_dir
        };
        let progress = self.content_progress.clone();

        self.spawn_job(
            move || {
                let agent = crate::net::agent();
                let result = content::download_file(&agent, &game_dir, kind, &file);
                progress.lock().unwrap_or_else(|e| e.into_inner()).clear();
                result.map_err(|e| e.to_string())
            },
            move |app, result| {
                app.content_downloading = false;
                match result {
                    Ok(path) => {
                        let msg = format!("{} installed to {}", item.title, path.display());
                        app.log_console(format!("[RustLauncher] {msg}"));
                        app.notify_info(msg);
                    }
                    Err(e) => {
                        let code = "MODRINTH";
                        app.notify_error(code, e);
                    }
                }
            },
        );
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

    #[test]
    fn collapse_detail_caps_lines_with_ellipsis() {
        // Within the cap: unchanged.
        let short = "line1\nline2\nline3";
        assert_eq!(collapse_detail(short, 3), vec!["line1", "line2", "line3"]);
        // Past the cap: the overflow becomes a single "…" line.
        let long = "a\nb\nc\nd\ne";
        assert_eq!(collapse_detail(long, 3), vec!["a", "b", "…"]);
        // A single very long line stays one line (truncation is visual).
        let one_long = "x".repeat(500);
        assert_eq!(collapse_detail(&one_long, 3), vec![one_long]);
    }

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
        assert_eq!(rows[0].kind, "loader");
        assert_eq!(rows[0].loader, Some(updater::Loader::Fabric));
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
        assert_eq!(rows[0].kind, "loader");
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
                loader: Some(updater::Loader::Fabric),
            },
            VersionRow {
                name: "1.21.4".into(),
                kind: "release".into(),
                installed: true,
                is_latest_release: false,
                remote: None,
                loader: None,
            },
            VersionRow {
                name: "25w14craftmine".into(),
                kind: "snapshot".into(),
                installed: false,
                is_latest_release: false,
                remote: None,
                loader: None,
            },
            VersionRow {
                name: "a1.2.5".into(),
                kind: "old_alpha".into(),
                installed: false,
                is_latest_release: false,
                remote: None,
                loader: None,
            },
        ];
        let rows = &rows[..];
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::All.matches(r))
                .count(),
            4
        );
        // The Mojang/Loaders split follows the detected loader.
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Mojang.matches(r))
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["1.21.4", "25w14craftmine", "a1.2.5"]
        );
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Loaders.matches(r))
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["fabric-1.20.1"]
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

    #[test]
    fn loader_detection_covers_all_prefixes() {
        assert_eq!(
            loader_of("fabric-loader-0.19.5-1.21.4"),
            Some(updater::Loader::Fabric)
        );
        assert_eq!(
            loader_of("quilt-loader-0.21.0-1.20.1"),
            Some(updater::Loader::Quilt)
        );
        assert_eq!(
            loader_of("neoforge-1.21.4-21.4.157"),
            Some(updater::Loader::NeoForge)
        );
        assert_eq!(
            loader_of("forge-1.20.1-47.4.10"),
            Some(updater::Loader::Forge)
        );
        // Case-insensitive + loose fabric/forge names from other launchers.
        assert_eq!(loader_of("Fabric-1.20.1"), Some(updater::Loader::Fabric));
        assert_eq!(loader_of("1.21.4"), None);
        assert_eq!(loader_of("25w14craftmine"), None);
    }

    #[test]
    fn base_mc_extraction_strips_loader_prefixes() {
        assert_eq!(base_mc_of("fabric-loader-0.19.5-1.21.4"), "1.21.4");
        assert_eq!(base_mc_of("quilt-loader-0.21.0-1.20.1"), "1.20.1");
        assert_eq!(base_mc_of("neoforge-1.21.4-21.4.157"), "1.21.4");
        assert_eq!(base_mc_of("forge-1.20.1-47.4.10"), "1.20.1");
        assert_eq!(base_mc_of("1.20.1"), "1.20.1");
        assert_eq!(base_mc_of("25w14craftmine"), "25w14craftmine");
    }

    #[test]
    fn merge_groups_loaders_after_mojang_rows() {
        let local = vec![
            local_version("forge-1.20.1-47.4.10"),
            local_version("1.20.1"),
            local_version("fabric-loader-0.19.5-1.21.4"),
            local_version("1.21.4"),
        ];
        let rows = merge_versions(&local, None);
        // Local order is preserved; every loader row carries its loader tag.
        assert_eq!(rows[0].loader, Some(updater::Loader::Forge));
        assert_eq!(rows[1].loader, None);
        assert_eq!(rows[2].loader, Some(updater::Loader::Fabric));
        assert_eq!(rows[3].loader, None);
        assert_eq!(
            rows.iter()
                .filter(|r| VersionFilter::Loaders.matches(r))
                .count(),
            2
        );
    }

    #[test]
    fn version_sort_orders() {
        let make = |name: &str, date: &str| VersionRow {
            name: name.into(),
            kind: "release".into(),
            installed: false,
            is_latest_release: false,
            remote: Some(updater::ManifestVersion {
                id: name.into(),
                kind: "release".into(),
                url: String::new(),
                releaseTime: date.into(),
            }),
            loader: loader_of(name),
        };
        let mut rows = vec![
            make("1.20", "2023-06-07"),
            make("fabric-loader-0.15.0-1.21", ""),
            make("1.21.4", "2024-12-03"),
            make("1.21.10", "2025-06-01"),
        ];

        // Alphabetical.
        sort_version_rows(&mut rows, VersionSort::Alphabetical);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names[0], "1.20");

        // A→Z starts with the digit-prefixed names in numeric order.
        sort_version_rows(&mut rows, VersionSort::AlphabeticalReverse);
        assert_eq!(rows.last().unwrap().name, "1.20");

        // Numeric-aware by number: 1.21.10 > 1.21.4 > 1.21... > 1.20.
        sort_version_rows(&mut rows, VersionSort::Number);
        assert_eq!(rows[0].name, "fabric-loader-0.15.0-1.21");
        assert_eq!(rows[1].name, "1.21.10");
        assert_eq!(rows[2].name, "1.21.4");
        assert_eq!(rows[3].name, "1.20");

        // By release date (newest first, rows without dates go last).
        sort_version_rows(&mut rows, VersionSort::ReleaseDate);
        assert_eq!(rows[0].name, "1.21.10");
        assert_eq!(rows[1].name, "1.21.4");
        assert_eq!(rows[2].name, "1.20");

        // By loader type: Mojang rows first, then loaders alphabetically
        // (fabric < forge).
        sort_version_rows(&mut rows, VersionSort::LoaderType);
        assert_eq!(rows[0].loader, None);
        assert_eq!(rows[1].loader, None);
        assert_eq!(rows[2].loader, None);
        assert_eq!(rows[3].loader, Some(updater::Loader::Fabric));
    }

    #[test]
    fn version_number_compare_is_numeric() {
        use std::cmp::Ordering;
        assert_eq!(cmp_version_parts_asc("1.21.9", "1.21.10"), Ordering::Less);
        assert_eq!(cmp_version_parts_asc("1.20", "1.20.1"), Ordering::Less);
        assert_eq!(cmp_version_parts_asc("1.21.4", "1.21.4"), Ordering::Equal);
        assert_eq!(cmp_version_parts_asc("a1.2", "1.2"), Ordering::Greater);
    }

    #[test]
    fn version_filter_by_loader_matches_exactly() {
        let mut rows = vec![
            VersionRow {
                name: "1.21.4".into(),
                kind: "release".into(),
                installed: false,
                is_latest_release: false,
                remote: None,
                loader: None,
            },
            VersionRow {
                name: "fabric-loader-0.19.5-1.21.4".into(),
                kind: "loader".into(),
                installed: true,
                is_latest_release: false,
                remote: None,
                loader: Some(updater::Loader::Fabric),
            },
        ];
        rows.retain(|r| r.loader == Some(updater::Loader::Fabric));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "fabric-loader-0.19.5-1.21.4");
    }
}
