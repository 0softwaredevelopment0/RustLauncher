//! GUI state: the `App` struct, shared type aliases, screens, running-game
//! registry items, content-tab state and the merged version-list model.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};

use crate::accounts::AccountStore;
use crate::auth::AccountKind;
use crate::content;
use crate::diagnostics;
use crate::icons;
use crate::instances;
use crate::logs::SessionLog;
use crate::news::NewsItem;
use crate::notifications::Toasts;
use crate::servers::ServerStore;
use crate::settings::Settings;
use crate::updater::{self, Manifest};
use crate::version::Version;

/// The maximum number of console lines kept in memory.
pub(crate) const CONSOLE_CAP: usize = 8000;

/// A deferred UI mutation produced by a background thread.
pub(crate) type Job = Box<dyn FnOnce(&mut App) + Send>;

/// Cached loader builds for one (loader, mc-version) pair.
pub(crate) type LoaderBuildsCache =
    BTreeMap<(updater::Loader, String), Option<Result<Vec<updater::LoaderBuild>, String>>>;

/// Which platform a content tab browses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentPlatform {
    Modrinth,
}

/// Licenses commonly offered as a filter on Modrinth.
pub(crate) const COMMON_LICENSES: &[&str] = &[
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
pub(crate) fn categories_for(kind: content::ContentKind) -> &'static [&'static str] {
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
pub(crate) fn strip_markdown(line: &str) -> String {
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
pub(crate) fn kind_uses_loader(kind: content::ContentKind) -> bool {
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
    pub(crate) fn label(self, lang: crate::lang::Language) -> String {
        use crate::lang::{tr, tr_fmt};
        match self {
            VersionSort::Newest => tr(lang, "Newest").to_string(),
            VersionSort::Number => tr_fmt(lang, "Number {0}", &[icons::ARROW_DOWNWARD]),
            VersionSort::ReleaseDate => tr(lang, "Release date").to_string(),
            VersionSort::LoaderType => tr(lang, "Loader type").to_string(),
            VersionSort::Alphabetical => tr_fmt(lang, "A {0} Z", &[icons::ARROW_FORWARD]),
            VersionSort::AlphabeticalReverse => tr_fmt(lang, "Z {0} A", &[icons::ARROW_FORWARD]),
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
pub(crate) fn loader_rank(row: &VersionRow) -> (u8, String) {
    match row.loader {
        None => (0, String::new()),
        Some(l) => (1, l.slug().to_string()),
    }
}

/// Numeric-aware descending comparison (`1.21.10` > `1.21.9`).
pub(crate) fn cmp_version_desc(a: &str, b: &str) -> std::cmp::Ordering {
    cmp_version_parts_asc(a, b).reverse()
}

pub(crate) fn cmp_version_parts_asc(a: &str, b: &str) -> std::cmp::Ordering {
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
pub(crate) struct ContentUi {
    pub(crate) kind: content::ContentKind,
    pub(crate) search: String,
    pub(crate) loader_filter: Option<updater::Loader>,
    /// Category tags chosen for the current kind (OR-combined).
    pub(crate) category_filter: Vec<String>,
    /// License short name filter (Modrinth).
    pub(crate) license_filter: Option<String>,
    /// Search sort order.
    pub(crate) sort: content::SortIndex,
    /// Search results, loaded lazily.
    pub(crate) results: Option<Result<Vec<content::ContentItem>, String>>,
    pub(crate) loading: bool,
    /// The project whose detail view is open.
    pub(crate) open_project: Option<String>,
    pub(crate) files: BTreeMap<String, Option<Result<Vec<content::ContentFile>, String>>>,
    pub(crate) files_loading: bool,
    /// The loaded project page (description body, links).
    pub(crate) detail: BTreeMap<String, Option<Result<content::ProjectDetail, String>>>,
    /// Version sort on the project page.
    pub(crate) version_sort: content::VersionSort,
    /// MC-version filter on the project page (empty = any).
    pub(crate) version_mc_filter: String,
    /// Version-type filter on the project page (release/beta/alpha).
    pub(crate) version_type_filter: Vec<String>,
}

impl ContentUi {
    pub(crate) fn kind_slot(&mut self) -> &mut content::ContentKind {
        &mut self.kind
    }

    pub(crate) fn search_slot(&mut self) -> &mut String {
        &mut self.search
    }

    pub(crate) fn loader_slot(&mut self) -> &mut Option<updater::Loader> {
        &mut self.loader_filter
    }

    pub(crate) fn version_sort_slot(&mut self) -> &mut content::VersionSort {
        &mut self.version_sort
    }

    pub(crate) fn version_mc_slot(&mut self) -> &mut String {
        &mut self.version_mc_filter
    }

    /// An owned read-only snapshot of the render-relevant state; lets the
    /// egui closures read it while `self` is borrowed for spawn_job.
    pub(crate) fn snapshot(&self) -> ContentSnapshot {
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
            files_loading: self.files_loading,
        }
    }
}

pub(crate) fn clone_files(
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

pub(crate) fn clone_details(
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
pub(crate) struct ContentSnapshot {
    pub(crate) kind: content::ContentKind,
    pub(crate) sort: content::SortIndex,
    pub(crate) category_filter: Vec<String>,
    #[allow(dead_code)] // read through the live tab, not the snapshot
    pub(crate) license_filter: Option<String>,
    pub(crate) results: Option<Result<Vec<content::ContentItem>, String>>,
    pub(crate) open_project: Option<String>,
    pub(crate) files: BTreeMap<String, Option<Result<Vec<content::ContentFile>, String>>>,
    pub(crate) detail: BTreeMap<String, Option<Result<content::ProjectDetail, String>>>,
    pub(crate) files_loading: bool,
}

// ---------------------------------------------------------------------------
// Icon cache: async PNG downloads turned into egui textures.
// ---------------------------------------------------------------------------

/// Cache of project icons keyed by URL. A URL maps to `Loading` while the
/// background fetch is in flight, then to the decoded texture or `Failed`.
#[derive(Default)]
pub struct IconCache {
    entries: BTreeMap<String, IconState>,
}

pub(crate) enum IconState {
    #[allow(dead_code)]
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

impl IconCache {
    /// Returns the cached texture for `url`, if it is decoded.
    pub fn get(&self, url: &str) -> Option<&egui::TextureHandle> {
        match self.entries.get(url) {
            Some(IconState::Ready(tex)) => Some(tex),
            _ => None,
        }
    }

    /// Whether a fetch for this URL is done or already in flight.
    pub(crate) fn known(&self, url: &str) -> bool {
        self.entries.contains_key(url) || url.is_empty()
    }

    pub(crate) fn insert(&mut self, url: String, state: IconState) {
        self.entries.insert(url, state);
    }
}

/// Download and decode an icon into raw RGBA + dimensions.
pub(crate) fn fetch_icon_rgba(
    url: &str,
    lang: crate::lang::Language,
) -> Result<(Vec<u8>, u32, u32)> {
    let bytes = crate::net::get_bytes(&crate::net::agent(), url, lang)?;
    let img = image::load_from_memory(&bytes)
        .context(crate::lang::tr(lang, "failed to decode the icon image"))?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Ok((rgba.into_raw(), w, h))
}

pub struct App {
    pub(crate) home_dir: PathBuf,
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
    pub(crate) terminate_confirm: Option<TerminateKind>,
    /// Live state of the "Don't ask again" checkbox while the dialog is open.
    pub(crate) terminate_dont_ask: bool,

    // Game process (the single-instance legacy fields are kept for the
    // most-recently-started game: General tab status and Console selection).
    pub console: Arc<Mutex<Vec<String>>>,
    pub console_seq: usize,
    /// Text typed in the Console command line, waiting to be sent.
    pub console_input: String,
    /// One registry entry per running game; several can run at once.
    pub running_games: Vec<RunningGame>,
    pub(crate) game_log: Arc<Mutex<Option<SessionLog>>>,
    /// PID of the most recently started game (legacy single-instance field).
    pub(crate) game_pid: Arc<Mutex<Option<u32>>>,

    // Instances tab.
    pub instance_store: instances::InstanceStore,
    pub instances_error: Option<String>,
    pub new_instance_name: String,
    pub new_instance_dir: String,
    /// Instance name awaiting deletion confirmation.
    pub instance_delete_pending: Option<String>,
    /// Instance selected on the General tab for the Play button.
    pub launch_instance: String,
    /// A Stop/Kill confirmation for a specific running instance, keyed by the
    /// game's PID handle (not a registry index — entries can shift while the
    /// dialog is open, and an index would then terminate the wrong game).
    pub(crate) instance_terminate_pending: Option<(Arc<Mutex<Option<u32>>>, TerminateKind)>,

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
    pub(crate) modrinth: ContentUi,
    pub(crate) content_downloading: bool,
    pub(crate) content_progress: Arc<Mutex<String>>,
    /// The MC version filter for content searches (empty = any).
    pub content_mc_filter: String,
    /// Shared texture cache of project icons (keyed by icon URL).
    pub(crate) icon_cache: IconCache,

    // Servers.
    pub server_status: BTreeMap<usize, crate::servers::ServerStatus>,
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
    /// Index of the expanded news detail view; `None` = card grid.
    pub news_selected: Option<usize>,

    // Java picker (Settings tab).
    /// Cached inventory of installed runtimes (filled by the first scan).
    pub(crate) java_installed: Vec<crate::java_locator::InstalledJava>,
    pub(crate) java_scan_loading: bool,
    /// Whether the first inventory scan already ran.
    pub(crate) java_scanned: bool,
    /// Cached sub-version lists per (edition id, major).
    pub(crate) java_versions:
        BTreeMap<(String, u32), Result<Vec<crate::java_download::SubVersion>, String>>,
    /// Which (edition, major) lists are currently being fetched.
    pub(crate) java_versions_loading: BTreeSet<(String, u32)>,
    /// Edition id chosen in the download section.
    pub java_dl_edition: String,
    /// Major version chosen in the download section.
    pub java_dl_major: u32,
    /// Sub-version id chosen in the download section.
    pub java_dl_sub: Option<String>,
    /// A runtime download in flight: (edition id, major, sub label).
    pub(crate) java_downloading: Option<(String, u32, String)>,
    /// Whether the Download Java section is expanded.
    pub java_download_open: bool,

    // Diagnostics.
    pub diag_results: Option<Vec<diagnostics::CheckResult>>,
    pub diag_running: bool,

    // Background completions.
    pub(crate) jobs: Arc<Mutex<Vec<Job>>>,
    /// Handle used by background threads to wake the UI when a job finishes.
    pub(crate) ctx: egui::Context,

    // Toast notifications (bottom-left).
    pub(crate) toasts: Toasts,
    /// Last frame instant, for advancing toast aging.
    pub(crate) last_frame: Option<std::time::Instant>,
    /// Launcher error log (logs/launcher-N.log), written at startup and on
    /// every launcher error so toasts can show its tail.
    pub(crate) launcher_log: Option<SessionLog>,

    /// Reset-to-defaults confirmation dialog is open.
    pub settings_reset_pending: bool,
    /// Cached background photo texture: the path it was decoded from plus
    /// the texture (`None` = the photo failed to load, fall back to flat
    /// color). Reloaded only when the path changes.
    pub(crate) bg_texture: Option<(String, Option<egui::TextureHandle>)>,
}

/// Which destructive action the confirmation dialog is guarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminateKind {
    Stop,
    Kill,
}

/// A game process started from an instance. Several can be alive at the same
/// time — one entry per launch, each with its own console buffer and PID.
pub struct RunningGame {
    /// The instance this game was started from.
    pub instance: String,
    /// Version launched.
    pub version: String,
    /// Live console of this particular game.
    pub console: Arc<Mutex<Vec<String>>>,
    /// True while the game process is alive (cleared by the worker thread).
    pub running: Arc<AtomicBool>,
    /// The game's java PID for Stop/Kill.
    pub pid: Arc<Mutex<Option<u32>>>,
    /// The game's stdin, for sending chat lines / commands from the Console.
    pub stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
    /// Last launch error, surfaced on the Instances tab.
    pub error: Arc<Mutex<Option<String>>>,
    /// Exit status text set by the worker when the game ends.
    pub status: Arc<Mutex<String>>,
}

/// State of an in-flight Microsoft device-code sign-in.
#[derive(Debug, Clone)]
pub(crate) struct MsLoginState {
    pub(crate) device_code: String,
    pub(crate) user_code: String,
    pub(crate) error: Option<String>,
    /// Set when the user pressed Cancel (consumed by the UI on the next frame).
    pub(crate) cancelled: bool,
}

/// Launcher file locations for background jobs (the executable directory is
/// portable; the DB lives next to the .exe).
pub(crate) struct LauncherPaths;

impl LauncherPaths {
    pub(crate) fn probe() -> PathBuf {
        crate::home::launcher_home().unwrap_or_else(|_| std::env::temp_dir())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    General,
    Console,
    Instances,
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
    pub(crate) fn label(self, lang: crate::lang::Language) -> &'static str {
        use crate::lang::tr;
        match self {
            VersionFilter::All => tr(lang, "All"),
            VersionFilter::Mojang => tr(lang, "Mojang"),
            VersionFilter::Loaders => tr(lang, "Loaders"),
            VersionFilter::Release => tr(lang, "Releases"),
            VersionFilter::Snapshot => tr(lang, "Snapshots"),
            VersionFilter::Old => tr(lang, "Old"),
            VersionFilter::Installed => tr(lang, "Installed"),
        }
    }

    pub(crate) fn matches(self, row: &VersionRow) -> bool {
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
