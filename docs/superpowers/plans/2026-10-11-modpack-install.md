# Modpack installation (.mrpack) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Install Modrinth `.mrpack` modpacks as launchable instances: download the archive via the Content tab into Downloads, then import it from the New instance dialog (file path / dialog / drag & drop), installing MC + loader + all pack files + overrides, deleting the archive on success.

**Architecture:** Linear install pipeline in a new `modpack.rs` engine (parse index → validate → install MC version + pinned loader via existing `updater` paths → download indexed files with SHA-1 skip-if-valid → extract override layers). The GUI runs it on a background thread with an `Arc<Mutex<String>>` progress line and applies the result through the `jobs` queue. CurseForge stays out of scope.

**Tech Stack:** Rust, egui/eframe 0.29, existing crates only (`zip`, `sha1`, `rfd`, `dirs`, `ureq`, `serde`/`serde_json`, `anyhow`).

**Spec:** `docs/superpowers/specs/2026-10-11-modpack-install-design.md`

## Global Constraints

- No new Cargo dependencies.
- Format: `.mrpack` v1 only; `game` must be `minecraft`; `formatVersion` must be `1`.
- Every path from the index and from override entries must be validated: no `..` components, no drive prefix (`X:`), no absolute `/` or `\` start.
- Files with `env.client == "unsupported"` are skipped; missing `env` installs.
- Override layers applied in order `overrides/` → `client-overrides/`; `server-overrides/` never touched.
- Existing files with a correct SHA-1 are never re-downloaded (idempotent retry).
- All new UI/error strings are canonical English keys translated in all 12 language files; `check_translations.ps1` must stay green.
- Material icons from `src/icons.rs` everywhere in new UI.
- The `.mrpack` file is deleted only after a fully successful install.
- One logical change per commit; detailed English commit messages; no attribution footers.

---

### Task 1: Engine — index model, validation, path safety

**Files:**
- Create: `src/modpack.rs`
- Modify: `src/main.rs` (module list, alphabetical: `mod modpack;` between `logs` and `nbt`)

**Interfaces (produced):**
```rust
pub struct MrpackIndex;              // fields: format_version (via rename), game, name, summary, files, dependencies
pub struct MrpackFile;               // path, hashes: BTreeMap<String,String>, env: Option<MrpackEnv>, downloads: Vec<String>, file_size: u64
pub struct MrpackEnv;                // client: Option<String>, server: Option<String>
impl MrpackIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self>;          // serde parse + validate()
    pub fn loader_dep(&self) -> Result<(updater::Loader, String)>; // (loader, pinned build)
    pub fn mc_version(&self) -> Result<&str>;
}
pub fn instance_rel_path(raw: &str) -> Option<std::path::PathBuf>; // None = unsafe/escaping path
```

- [ ] **Step 1: Write failing tests** (in `src/modpack.rs` `#[cfg(test)]`): happy-path index parse (name/files/dependencies incl. unknown dep key tolerated), wrong `game` rejected, wrong `formatVersion` rejected, missing `minecraft` dep rejected, two loader deps rejected, zero known loader deps rejected, each known loader key maps to the right `Loader`, `instance_rel_path` accepts `mods/a.jar` and rejects `../evil`, `C:/evil`, `/evil`, `\evil`, `a/../..`, empty.
- [ ] **Step 2:** `cargo test modpack` → FAIL (module empty/unregistered).
- [ ] **Step 3: Implement** structs (serde with `#[serde(rename = "formatVersion")]`, `#[serde(default)]` on optional collections; `BTreeMap` for `dependencies`), `parse` → `serde_json::from_slice` then `validate`, `validate` per Global Constraints, `loader_dep` mapping `fabric-loader`→`Fabric`, `quilt-loader`→`Quilt`, `forge`→`Forge`, `neoforge`→`NeoForge`, `instance_rel_path` per spec (normalize `\`→`/`, split, reject `..`/empty result/drive colon in second char of the raw string/leading `/`).
- [ ] **Step 4:** `cargo test` → all pass (151+ existing stay green).
- [ ] **Step 5:** Commit: `Add modpack index model: .mrpack parsing, validation, path safety`.

### Task 2: Engine — install pipeline

**Files:**
- Modify: `src/modpack.rs` (add pipeline + tests)

**Interfaces (produces):**
```rust
pub struct InstallOutcome {
    pub pack_name: String,
    pub version_id: String,      // final loader version id installed
    pub files_downloaded: usize,
    pub overrides_extracted: usize,
}
pub fn read_index(zip_path: &Path) -> Result<MrpackIndex>;  // reads modrinth.index.json from the archive
pub fn install(zip_path: &Path, game_root: &Path, agent: &ureq::Agent, lang: Language,
               progress: &mut dyn FnMut(&str)) -> Result<InstallOutcome>;
```

- [ ] **Step 1: Write failing tests** with a synthetic `.mrpack` (built in-test with the `zip` crate into a temp dir via `std::env::temp_dir()+unique suffix`): overrides layering (`overrides/a.txt`, `client-overrides/a.txt` + `b.txt`, `server-overrides/c.txt` → after extract: `a.txt` = client-overrides content, `b.txt` exists, `c.txt` absent; count == 3), unsafe override entry (`overrides/../evil`) → install error containing "unsafe path", index file with `env.client == "unsupported"` counted as skipped (exposed via a small `MrpackFile::client_installable() -> bool` helper tested directly), existing file with correct sha1 not rewritten (mtime/content check via marker file replaced by expected bytes).
- [ ] **Step 2:** `cargo test modpack` → new tests FAIL.
- [ ] **Step 3: Implement:**
  - `read_index`: `zip::ZipArchive::new(File::open)`, `by_name("modrinth.index.json")`, read to Vec, `MrpackIndex::parse`. Errors reuse existing keys `"failed to open the zip archive"`, new key `"the archive has no modrinth.index.json"`.
  - `install` steps: `read_index` → `mc_version`/`loader_dep` → progress `"Installing Minecraft {0}"` + `updater::fetch_manifest` + `updater::find_version` + `updater::install_version` → progress `"Installing {0} {1}"` (loader label, build) + `updater::fetch_loader_builds` + find exact `LoaderBuild` by pinned version (error: `"loader build {0} for {1} not found"` new key) + `updater::install_loader` → download loop over `files` (skip `!client_installable()`; `net::get_bytes` per mirror URL from `downloads[]` until one succeeds; reuse `"the file has no download URL"`; SHA-1 via `updater::sha1_hex`, mismatch → existing key `"{0}: checksum mismatch (expected {1}, got {2})"`; skip existing-with-correct-hash; write `dest.tmp` + rename; progress `"pack files {0}/{1}"`) → extract override layers `overrides/`, `client-overrides/` with `instance_rel_path` validation (new key `"unsafe path in the modpack: {0}"`), overwrite allowed, `server-overrides/` skipped → `InstallOutcome`.
  - Full-file SHA-1 read of existing files (small mods — fine at this scale).
- [ ] **Step 4:** `cargo test` → all pass.
- [ ] **Step 5:** Commit: `Add modpack install pipeline: MC+loader via updater, indexed downloads with SHA-1 skip, layered overrides`.

### Task 3: Content tab — modpack archives go to Downloads

**Files:**
- Modify: `src/content.rs` (`goes_to_downloads`, `dest_dir`), `src/content.rs` tests, `src/gui/content_ui.rs:678-695` (success toast)

**Interfaces:** consumes `ContentKind::Modpack`; no new APIs.

- [ ] **Step 1: Failing test** in `content.rs` tests: `assert!(ContentKind::Modpack.goes_to_downloads());`
- [ ] **Step 2:** `cargo test content` → FAIL.
- [ ] **Step 3:** Add `Modpack` to `goes_to_downloads()` matches!; delete the `dest_dir` `Modpack => "versions"` arm + its comment (catch-all `"downloads"` covers it, unreachable through the UI).
- [ ] **Step 4:** `content_ui.rs` `download_content` apply: when `kind == ContentKind::Modpack`, success message uses new key `"Modpack archive saved to {0} — create an instance to install it"` (arg: `path.display()`), else the existing `"{0} installed to {1}"`.
- [ ] **Step 5:** `cargo test` → pass. Commit: `Download modpack archives into the user's Downloads folder with a how-to-install hint`.

### Task 4: GUI — New instance modpack mode

**Files:**
- Modify: `src/gui/state.rs` (App fields), `src/gui/freestanding.rs` (init + helpers), `src/gui/screens.rs` (dialog), `src/gui/app.rs:859` (busy repaint), `src/instances.rs` (`unique_name`), `src/home.rs` (`instances_dir`), `src/icons.rs` (`INVENTORY_2` + `ALL_ICONS`)

**Interfaces:**
```rust
// state.rs (App, Instances tab section)
pub new_instance_modpack: bool,
pub new_instance_pack_file: String,
pub new_instance_pack_summary: Option<String>,
pub modpack_installing: bool,
pub modpack_progress: Arc<Mutex<String>>,
// home.rs
pub fn instances_dir(home: &Path) -> PathBuf;               // home.join("instances")
// instances.rs
pub fn unique_name(&self, base: &str) -> String;            // create() refactored to use it
// freestanding.rs
pub(crate) fn set_modpack_file(&mut self, path: &Path);     // read_index -> autofill name/dir/summary
pub(crate) fn start_modpack_install(&mut self);             // validations + spawn install thread
pub(crate) fn browse_modpack_file(&mut self);               // rfd pick_file, dir = Downloads
```

- [ ] **Step 1: state/infra**: add fields, `instances_dir`, `unique_name` (create() uses it; behavior unchanged — existing instances tests stay green), `INVENTORY_2 = "\u{e1a6}"` icon + `ALL_ICONS` entry, App init in freestanding.rs (`new_instance_modpack: false`, pack fields default, `modpack_progress: Arc::new(Mutex::new(String::new()))`).
- [ ] **Step 2: helpers** in freestanding.rs:
  - `set_modpack_file`: validate extension `.mrpack` (error key `"the modpack file must have the .mrpack extension"` → `instances_error`), `modpack::read_index`, autofill `new_instance_name` (pack `name`), `new_instance_dir` = `home::instances_dir(&self.home_dir).join(instance_store.unique_name(&name))`, `new_instance_pack_summary` = `summary`; index errors → `instances_error`.
  - `browse_modpack_file`: `rfd::FileDialog::new().add_filter("Modrinth modpack", &["mrpack"]).set_directory(dirs::download_dir())` → `pick_file()` → `set_modpack_file`.
  - `start_modpack_install`: guard `modpack_installing`; validate name/dir non-empty (existing keys), pack path non-empty + exists + `.mrpack`; compute `final_name = unique_name(name)`, `game_root = if user-edited dir use it else instances_dir.join(final_name)` (use the `new_instance_dir` field as-is — it was auto-filled and stays editable); set `modpack_installing=true`, progress = `tr_fmt("Installing modpack '{0}'…", name)`; `spawn_job` with `modpack::install` + progress closure writing into `modpack_progress`; apply: `modpack_installing=false`, progress clear; `Ok(outcome)` → `instance_store.create(&final_name_used_by_dir?...)`.
    - **Dir/name consistency:** install runs into the dir the user sees; `create` is called with the same `(name, dir)` pair the user confirmed — pass the *original* `new_instance_name` (not pre-suffixed) so `create()` derives the identical suffix; the auto-filled dir already contains the suffix, and when the user edited either field they own the pair. Register instance, save store (existing error path), `std::fs::remove_file(pack)` (failure → warning toast with the error, instance stays), success toast `"Modpack '{0}' installed into instance '{1}'"`, clear `new_instance_*` fields; `Err(e)` → `instances_error` + error toast, pack file kept.
- [ ] **Step 3: dialog** in `screens.rs` `ui_instances` Create section: checkbox row under the grid (`INVENTORY_2` + `"Install modpack (.mrpack)"`); when on — grid row `"Modpack file"`: singleline field (hint `"Select a .mrpack archive…"`) + Browse button (`FOLDER` icon); summary weak label when set; Create button `add_enabled(!self.modpack_installing)` branches to `start_modpack_install()` in modpack mode; progress label under the button reading `modpack_progress` while installing.
- [ ] **Step 4: app.rs**: busy repaint condition `|| self.modpack_installing`.
- [ ] **Step 5:** `cargo test` → green; `cargo build` → ok. Commit: `Add modpack import mode to the New instance dialog with background install and progress`.

### Task 5: Drag & drop of .mrpack onto the dialog

**Files:**
- Modify: `src/gui/app.rs` (`update`), `src/gui/freestanding.rs` (`handle_dropped_files`)

- [ ] **Step 1:** in `update()` after `apply_pending_jobs`: `let dropped = ctx.input(|i| i.raw.dropped_files.clone()); if !dropped.is_empty() { self.handle_dropped_files(dropped); }`.
- [ ] **Step 2:** `handle_dropped_files(files: Vec<egui::DroppedFile>)`: only when `self.screen == Screen::Instances && self.new_instance_modpack`; take the first dropped path ending `.mrpack` (case-insensitive) → `set_modpack_file`; other files/screens silently ignored.
- [ ] **Step 3:** `cargo test` + `cargo build` green. Commit: `Fill the modpack file field by dragging a .mrpack onto the New instance dialog`.

### Task 6: Translations, version bump, final build, journal

**Files:**
- Modify: `src/lang/{de,es,fr,it,ja,ko,pl,pt,ru,tr_,uk,zh}.rs`, `Cargo.toml`, Vault notes

**New canonical keys** (canonical = English; values for all 12 languages authored in this task; `ru` shown as the reference):
`"Install modpack (.mrpack)"` → «Установить сборку (.mrpack)»; `"Modpack file"` → «Файл сборки»; `"Select a .mrpack archive…"` → «Выберите файл сборки (.mrpack)…»; `"Modpack '{0}' installed into instance '{1}'"` → «Сборка «{0}» установлена в инстанс «{1}»»; `"Modpack archive saved to {0} — create an instance to install it"` → «Архив сборки сохранён в {0} — создайте инстанс, чтобы установить его»; `"the modpack file must have the .mrpack extension"` → «файл сборки должен иметь расширение .mrpack»; `"Installing modpack '{0}'…"` → «Установка сборки «{0}»…»; `"the archive has no modrinth.index.json"` → «в архиве нет modrinth.index.json»; `"failed to parse modrinth.index.json: {0}"` → «не удалось разобрать modrinth.index.json: {0}»; `"unsupported modpack format version {0}"` → «неподдерживаемая версия формата сборки: {0}»; `"unsupported modpack game '{0}'"` → «неподдерживаемая игра сборки: {0}»; `"the modpack does not declare a Minecraft version"` → «сборка не указывает версию Minecraft»; `"the modpack must declare exactly one loader (found {0})"` → «сборка должна указывать ровно один лоадер (найдено: {0})»; `"unknown modpack loader '{0}'"` → «неизвестный лоадер сборки: {0}»; `"unsafe path in the modpack: {0}"` → «опасный путь в сборке: {0}»; `"loader build {0} for {1} not found"` → «сборка лоадера {0} для {1} не найдена»; `"Installing Minecraft {0}"` → «Установка Minecraft {0}»; `"Installing {0} {1}"` → «Установка {0} {1}»; `"pack files {0}/{1}"` → «файлы сборки {0}/{1}»; `"Extracting overrides…"` → «Распаковка overrides…».

- [ ] **Step 1:** insert all 20 keys into each of the 12 language files (alphabetical position, match table style).
- [ ] **Step 2:** `powershell -File check_translations.ps1` → 0 missing/extra.
- [ ] **Step 3:** `Cargo.toml` version → `0.3.0-alpha.1`; `cargo test` green; `cargo build --release` green.
- [ ] **Step 4:** Commit + push: `Translate modpack installation strings and bump version to 0.3.0-alpha.1`.
- [ ] **Step 5:** Vault: `Projects/RustLauncher/Последние действия` entry + README feed row; commit Vault repo.

## Self-review notes

- Spec coverage: engine (T1-2), Downloads redirect + hint (T3), dialog + progress + delete-on-success (T4), drag&drop (T5), i18n + versioning (T6). Optional-mods dialog / cancel / CF / CLI — declared non-goals in the spec.
- Type consistency: `InstallOutcome` consumed only inside T4's job closure via message strings; `unique_name` used by both `set_modpack_file` (dir autofill) and `create` (registration) — same algorithm, so suffixes match.
- Translation gate is mechanical (`check_translations.ps1`), so authoring the remaining 10 languages during execution is safe; `ru` values above are the reference register.
