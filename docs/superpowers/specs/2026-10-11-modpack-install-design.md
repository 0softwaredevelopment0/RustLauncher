# Modpack installation (.mrpack) — Design

Date: 2026-10-11
Status: approved (design discussion 2026-10-11), pending implementation

## Overview

RustLauncher can already search Modrinth (and CurseForge) and download single
content items, but the `Modpack` category only dumps the archive into
`versions/` — nothing is installed. This feature adds real modpack support:

1. Downloading a modpack from Modrinth puts the `.mrpack` file into the
   user's **Downloads** folder (instead of `versions/`).
2. The **New instance** dialog gains an "Install modpack (.mrpack)" mode:
   the user picks a downloaded `.mrpack` (manual path, file dialog that
   starts in Downloads, or drag & drop), the launcher installs the whole
   pack (Minecraft version + loader + every file listed in the index +
   overrides) into the instance directory, then deletes the `.mrpack`.

Format: Modrinth `.mrpack` v1 only. CurseForge is explicitly out of scope
(requires an API key).

## Non-goals (v1)

- CurseForge modpacks (`.zip` with `manifest.json`).
- Cancel/pause of a running installation (engine is idempotent; retry works).
- Optional-mods selection dialog: files with `env.client == "optional"` are
  installed unconditionally in v1 (the spec recommends a dialog; a natural
  follow-up).
- CLI command (`install --modpack <path>`) — the engine is reusable later.
- SHA512 verification (SHA1 only — the crate is already a dependency; the
  index always contains SHA1; sources are the Modrinth CDN whitelist).

## Format reference (Modrinth .mrpack v1)

A `.mrpack` is a ZIP containing `modrinth.index.json` (UTF-8, at the root)
plus optional override folders.

`modrinth.index.json`:

- `formatVersion`: number, must be `1`.
- `game`: string, must be `"minecraft"` — anything else is a clear error.
- `name`: human-readable pack name → default instance name.
- `summary`: optional description (shown in the dialog as a hint).
- `files[]`: files to download.
  - `path`: destination relative to the instance directory.
    **Security:** must never escape the instance directory — reject any
    value containing `..` or starting with a drive name (`[A-Za-z]:/`,
    `[A-Za-z]:\`), `/` or `\`. The same validation applies to every path
    extracted from override folders.
  - `hashes`: object; always contains `sha1` (verified after download).
  - `env` (optional): `{ client, server }`, values
    `required`/`optional`/`unsupported`. Missing field = install.
    Client launcher rule: skip the file when `client == "unsupported"`.
  - `downloads[]`: HTTPS mirror URLs, tried in order; redirects must be
    followed (ureq does this by default).
  - `fileSize`: byte size, used for download progress.
- `dependencies`: object; `minecraft` is required, plus exactly one loader
  key: `fabric-loader`, `quilt-loader`, `forge`, `neoforge`. Unknown keys
  are tolerated (the spec may add IDs).

Override folders (layered, later layers overwrite earlier):

1. `overrides/` → always extracted.
2. `client-overrides/` → extracted after `overrides/` (client launcher).
3. `server-overrides/` → skipped entirely.

Order of installation: index `files[]` first, then override layers —
overrides win over downloaded files with the same path.

## Architecture

### Engine — `src/modpack.rs` (new module)

Linear pipeline, no job system:

```text
read_index(zip_path) -> Result<MrpackIndex>
    parse modrinth.index.json straight from the archive (ZipArchive),
    validate formatVersion / game / dependencies.
    Used by the UI for the name/summary preview before installing.

install(zip_path, game_root, agent, progress) -> Result<InstallOutcome>
    1. Read + validate the index; resolve the loader from dependencies
       (fabric-loader -> Loader::Fabric, quilt-loader -> Loader::Quilt,
       forge -> Loader::Forge, neoforge -> Loader::NeoForge) and its
       pinned version.
    2. Install the pinned Minecraft version + loader build into
       game_root via the existing updater paths (the same machinery the
       CLI `install` command uses: vanilla version install + loader
       install, including NeoForge-from-installer-metadata).
    3. Download every index file with client != "unsupported":
       try `downloads[]` mirrors in order, verify SHA1, write to
       game_root/<path>. A file that already exists with the correct
       hash is skipped (idempotency — safe retry after interruption).
    4. Extract `overrides/`, then `client-overrides/` (every extracted
       path goes through the same escape validation).
    5. Return InstallOutcome { name, mc, loader, files_downloaded,
       bytes_downloaded, overrides_extracted }.
```

`progress` is a callback receiving a human-readable step string
(`"Downloading mods/… 3/57 (12.3 MB)"`), called on step boundaries and
between file downloads. The GUI wraps it with an
`Arc<Mutex<String>>` (`modpack_progress`, same pattern as
`install_progress`/`content_progress`).

The GUI runs `install` on `std::thread::spawn` and applies the results on
the UI thread through the existing `jobs` queue
(`Box<dyn FnOnce(&mut App) + Send>`): register the instance, delete the
`.mrpack` (only on full success), push a toast, clear progress.

Errors: typed error with the failed step; the engine leaves already-written
files in place, so a retry continues from where it stopped (hash skip).

### Content tab change — `src/content.rs`

`ContentKind::Modpack` becomes a Downloads-kind: `goes_to_downloads()`
returns `true` for it (the archive is a hand-off artifact, not game
content). After a successful download the toast points the user at the
New instance dialog. `dest_dir()`'s `Modpack => "versions"` arm is removed
with its comment.

### New instance dialog — `src/gui/screens.rs`, `state.rs`, `freestanding.rs`

- New toggle in the New instance section: "Install modpack (.mrpack)"
  (material icon, e.g. `INVENTORY_2`).
- When enabled, an extra grid row "Modpack file" appears:
  - text field for the path (hint shows `*.mrpack`);
  - Browse button (`rfd::FileDialog::pick_file`, filter `*.mrpack`,
    starting directory `dirs::download_dir()`, material `FOLDER` icon);
  - drag & drop: `App` receives egui `raw.dropped_files` while the New
    instance screen is open and the modpack toggle is on; a single
    dropped `.mrpack` file fills the field (other screens unaffected).
- On file selection, `read_index` fills:
  - instance name (from `name`, still editable),
  - game directory (auto-filled `<launcher home>/instances/<Name>`,
    still editable),
  - summary shown as a hint line.
- Create button behavior in modpack mode: validate inputs, spawn the
  background install thread, show progress in the dialog, disable the
  button while installing. On success: instance appears in the store,
  `.mrpack` is deleted from Downloads, success toast. On failure: error
  text in the dialog + error toast; the `.mrpack` stays.

Material icons everywhere in the new UI (project style, `icons.rs`).

### Translations

All new UI strings get keys in all 12 languages; `check_translations.ps1`
must stay green (429 keys today + new keys). English strings are the
canonical keys.

## Testing

Unit tests (new `modpack.rs` test module, synthetic `.mrpack` fixtures
built with the `zip` crate):

- index parsing: happy path, wrong `game`, wrong `formatVersion`,
  missing `minecraft` dependency, unknown loader key combination,
  unknown dependency keys tolerated;
- path escape validation (`..`, drive prefix, absolute `/`/`\`) for index
  files and override entries;
- `env.client` filtering (`unsupported` skipped, missing env installed);
- idempotency: existing file with correct SHA1 is not re-downloaded,
  wrong hash is replaced;
- overrides layering: `overrides/` then `client-overrides/` overwrite,
  `server-overrides/` untouched.

Regression: existing tests keep passing (`cargo test`), translations
script green, `cargo build --release` succeeds.

Manual smoke test (documented in the journal, not automated): download a
popular small Modrinth pack, import it, launch it.

## Files touched

- `src/modpack.rs` (new) — engine + tests.
- `src/main.rs` — module registration.
- `src/content.rs` — Modpack → Downloads kind; `dest_dir` cleanup.
- `src/gui/screens.rs` — modpack section of the New instance dialog.
- `src/gui/state.rs` — state fields (file path, toggle, progress, busy flag).
- `src/gui/freestanding.rs` — drag & drop handling (input plumbing).
- `src/lang/*.rs` (12 files) — new keys.
- `Cargo.toml` — no new dependencies expected (`zip`, `sha1`, `rfd`,
  `dirs`, `ureq` are already there).

## Versioning

New user-facing feature, untested in real usage → propose
`0.2.1-alpha.1` → `0.3.0-alpha.1` (subversion bump, alpha channel) after
implementation; promote to beta after a real pack is imported and launched.
