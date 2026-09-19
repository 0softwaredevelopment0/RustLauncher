# RustLauncher

A fast, native Minecraft launcher written in Rust — the successor of the
Java/JavaFX PowerLaunch.

![status](https://img.shields.io/badge/status-beta-orange)

## Features

- **GUI** (eframe/egui): General, Console, Instances, Versions, Servers,
  Accounts, Skins, Modrinth, News, Settings, Diagnostics — with dark/light
  theme and Material-style icons.
- **Instances**: named game directories. Create/delete instances (delete
  never touches the game files), select one on the General tab and launch;
  several instances can run at the same time, each with its own console,
  Stop and Kill.
- **Launch pipeline**: version discovery in three directory layouts,
  offline accounts (vanilla-compatible `OfflinePlayer:<name>` UUIDs),
  classpath assembly with dedup and LWJGL-conflict resolution, natives
  extraction, unsafe JVM-arg filtering, Java major-version selection.
  Memory is configured via JVM flags (`-Xms`/`-Xmx`); the launcher refuses
  to start the game without heap flags or without a game directory.
- **Versions tab**: one merged list of everything — versions auto-discovered
  in the active instance's directory (vanilla, Fabric, any modded install)
  and every version from the Mojang manifest — with type/loader filters,
  sorting, a search box, and Select/Install buttons per row.
- **Modrinth tab**: search mods, resource packs, data packs, shaders,
  modpacks, plugins and server software; filters by MC version, loader,
  categories and license; project pages with descriptions, changelogs,
  version lists and R/B/A release badges; downloads land in the instance
  (mods, resource packs) or the Downloads folder (data packs, shaders,
  plugins, servers).
- **Accounts**: Offline (Argon2id-hashed password in the launcher DB),
  Ely.by and Mojang/Microsoft (device-code flow); removal re-authenticates.
- **Servers**: import/export Minecraft's `servers.dat` **without losing
  per-server icons** (the Java version rewrote the file from scratch and
  dropped every unknown NBT tag), plus a TCP status probe.
- **Skins**: download from Crafatar by nickname, import local 64x32/64x64
  PNGs, in-app preview.
- **Profiles**: named settings snapshots (JSON), create/switch/delete.
- **Diagnostics**: DNS, HTTP and TCP checks against Mojang services and
  game servers.
- **Notifications**: bottom-left toast stack with a countdown bar; error
  toasts carry the error code and the log tail, click to expand.
- **Logs**: numbered session logs (`logs/game-N.log` per launch,
  `logs/launcher-N.log` for the launcher itself).
- **JVM presets**: Minimal (`-Xms1m -Xmx4g`), optimized G1GC, Shenandoah,
  ZGC and Parallel GC sets; free-form editing with live validation. Java is
  auto-detected by default or pinned to a custom executable (a checkbox
  switches between the two modes).
- **CLI kept**: `versions` and `launch` subcommands for scripting
  (`launch` requires an explicit `--game-dir`).

## Source layout

```
src/
├── main.rs           — entry point: CLI dispatch, GUI bootstrap
├── cli.rs            — clap definitions for the scripting subcommands
├── db.rs             — Argon2id password DB for offline accounts
├── gui/              — the eframe/egui interface
│   ├── mod.rs        — module docs and wiring
│   ├── state.rs      — App state, shared types, version model
│   ├── freestanding.rs — version merge/sort/filter, process plumbing
│   ├── screens.rs    — General / Console / Instances / Versions /
│   │                   Servers / Accounts / Skins
│   ├── content_ui.rs — the Modrinth browser
│   ├── toast_ui.rs   — notification stack and painter icons
│   ├── app.rs        — frame loop, navigation, confirmation dialogs
│   └── tests.rs      — GUI unit tests
├── accounts.rs       — account store (Offline / Ely.by / Mojang)
├── auth.rs           — offline, Ely.by and Microsoft authentication
├── classpath.rs      — classpath assembly with dedup
├── content.rs        — Modrinth API client and downloads
├── diagnostics.rs    — DNS/HTTP/TCP checks
├── home.rs           — launcher home layout (portable)
├── icons.rs          — embedded Material Icons font
├── instances.rs      — instance registry (instances.json)
├── java_locator.rs   — installed JDK/JRE discovery
├── jvm.rs            — JVM flag presets and validation
├── launcher.rs       — launch plan construction
├── logs.rs           — session logs
├── nbt.rs            — NBT read/write for servers.dat
├── net.rs            — HTTP agent
├── news.rs           — news feed
├── notifications.rs  — toast model (aging, countdown, expand)
├── profiles.rs       — named settings snapshots
├── servers.rs        — servers.dat NBT merge + status probe
├── settings.rs       — typed settings (config.json)
├── skins.rs          — skin download/import/preview
├── updater.rs        — Mojang manifest + loader manifests
├── version.rs        — local version discovery
└── version_json.rs   — version JSON → launch plan inputs
```

## Build

```
cargo build --release
```

The binary lands in `target/release/rustlauncher(.exe)`.

## Usage

Run without arguments to open the GUI:

```
rustlauncher
```

Scriptable CLI:

```
rustlauncher versions [--game-dir <dir>]
rustlauncher launch --version <name> --username <nick> [--server host[:port]]
                    [--game-dir <dir>] [--jvm-args "..."] [--dry-run]
```

Settings/accounts/servers/profiles/skins/instances/logs live in the launcher
home (`RUSTLAUNCHER_HOME` overrides it; otherwise the OS data directory +
`RustLauncher`). The default game directory is `%APPDATA%\.rustlauncher`
(Windows) or `~/.rustlauncher`; override it in Settings, `--game-dir`, or by
creating an instance with its own directory.

## Differences from PowerLaunch

| Java PowerLaunch            | RustLauncher                          |
|-----------------------------|---------------------------------------|
| JavaFX controller, 4300 LoC | egui GUI, native binary               |
| SQLite settings/accounts    | typed JSON files (config/accounts/…)  |
| Manual JSON string search   | serde struct parsing                  |
| Could only list remote versions | Full install with SHA-1 checks; one merged local+remote version list |
| servers.dat rewrite lost icons | NBT-preserving merge               |
| 5-minute kill timer in CLI  | Waits as long as the game runs        |
| Launcher JRE used for game  | Required Java major auto-selected     |
| No instance concept         | Multiple instances, parallel launches |
| No mod browser              | Modrinth search, pages and installs   |

Ported with behavior fixes: news feed URL, skin validation, JVM-arg filter,
offline UUID consistency between CLI and GUI.

## License

AGPL-3.0 — same as the original PowerLaunch.
