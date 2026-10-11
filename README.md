# RustLauncher

A fast, native Minecraft launcher written in Rust.

![Latest release](https://img.shields.io/github/v/release/0softwaredevelopment0/RustLauncher)

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
  per-server icons** — the NBT merge keeps every tag it doesn't touch,
  so per-server icons survive round trips, plus a TCP status probe.
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
├── main.rs             — entry point: CLI dispatch, GUI bootstrap
├── cli.rs              — clap definitions for the scripting subcommands
├── game/               — the launch pipeline
│   ├── launcher.rs     — launch plan construction
│   ├── classpath.rs    — classpath assembly with dedup
│   ├── jvm.rs          — JVM flag presets and validation
│   ├── instances.rs    — instance registry (instances.json)
│   ├── version.rs      — local version discovery
│   ├── version_json.rs — version JSON → launch plan inputs
│   └── java/           — locator.rs (JDK/JRE discovery, and download.rs
│                          for Temurin/Zulu/Corretto runtimes)
├── account/            — mod.rs re-exports the everyday API
│   ├── auth.rs         — offline, Ely.by and Microsoft authentication
│   ├── store.rs        — account store (Offline / Ely.by / Mojang)
│   └── db.rs           — Argon2id password DB for offline accounts
├── updater/            — remote discovery and installation
│   ├── mod.rs          — Mojang manifest, SHA-1 + download helpers
│   ├── loaders.rs      — Fabric/Quilt/NeoForge/Forge build discovery
│   ├── vanilla.rs      — vanilla version install (client jar + assets)
│   ├── loader_install.rs — mod loader installation
│   └── libraries.rs    — Maven library resolution shared by installers
├── content/            — mod platform
│   ├── mod.rs          — Modrinth API client and downloads
│   └── modpack.rs      — .mrpack modpack installation
├── services/           — launcher feature services
│   ├── settings.rs     — typed settings (config.json)
│   ├── servers.rs      — servers.dat NBT merge + status probe
│   ├── news.rs         — news feed
│   ├── skins.rs        — skin download/import/preview
│   ├── diagnostics.rs  — DNS/HTTP/TCP checks
│   └── notifications.rs — toast model (aging, countdown, expand)
├── util/               — shared infrastructure
│   ├── home.rs         — launcher home layout (portable)
│   ├── logs.rs         — session logs
│   ├── nbt.rs          — NBT read/write for servers.dat
│   └── net.rs          — HTTP agent
├── lang/               — UI translations (ru, de, es, fr, ...)
└── gui/                — the eframe/egui interface
    ├── mod.rs          — module docs and wiring
    ├── state.rs        — App state, shared types, version model
    ├── app.rs          — frame loop, navigation, confirmation dialogs
    ├── screens/        — one file per screen: general, instances,
    │                     versions, servers, accounts, skins, news,
    │                     settings, diagnostics
    ├── freestanding/   — App logic by concern: game (launch/stop/kill),
    │                     console, java, modpack, persistence, toasts,
    │                     visuals
    ├── theme/          — fonts.rs (CJK fallback), icons.rs (Material Icons)
    ├── content_ui.rs   — the Modrinth browser
    ├── toast_ui.rs     — notification rendering and painter icons
    └── tests.rs        — GUI unit tests
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

## Design highlights

| Area                | How RustLauncher does it                         |
|---------------------|--------------------------------------------------|
| Interface           | egui GUI in a small native binary                |
| Storage             | typed JSON files (config/accounts/…)             |
| Version JSON        | serde struct parsing, full install with SHA-1 checks |
| Version list        | one merged local + remote catalog                |
| servers.dat         | NBT-preserving merge, icons survive              |
| CLI                 | waits as long as the game runs                   |
| Java runtime        | required major version auto-selected             |
| Instances           | multiple instances, parallel launches            |
| Mods                | Modrinth search, pages and installs              |
| Offline accounts    | vanilla-compatible UUIDs, Argon2id password DB   |

## License

AGPL-3.0
