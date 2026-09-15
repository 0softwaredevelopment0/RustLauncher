# RustLauncher

A fast, native Minecraft launcher written in Rust — the full rewrite of the
Java/JavaFX **PowerLaunch**. One ~2 MB binary instead of a JVM, a SQLite
database and 10 000+ lines of Java.

![status](https://img.shields.io/badge/status-beta-orange)

## Features

- **GUI** (eframe/egui): Play, Console, Catalog, Servers, Accounts, Skins,
  News, Settings, Diagnostics — with dark/light theme.
- **Launch pipeline**: version discovery in three directory layouts,
  offline accounts (vanilla-compatible `OfflinePlayer:<name>` UUIDs),
  classpath assembly with dedup and LWJGL-conflict resolution, natives
  extraction, unsafe JVM-arg filtering, Java major-version selection.
- **Version catalog**: install any release/snapshot/old version directly
  from the Mojang manifest — version JSON, client jar (SHA-1 verified),
  asset index and all assets.
- **Servers**: import/export Minecraft's `servers.dat` **without losing
  per-server icons** (the Java version rewrote the file from scratch and
  dropped every unknown NBT tag), plus a TCP status probe.
- **Skins**: download from Crafatar by nickname, import local 64x32/64x64
  PNGs, in-app preview.
- **Profiles**: named settings snapshots (JSON), create/switch/delete.
- **Diagnostics**: DNS, HTTP and TCP checks against Mojang services and
  game servers.
- **Logs**: numbered session logs (`logs/game-N.log`), one per launch.
- **CLI kept**: `versions` and `launch` subcommands for scripting.

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
rustlauncher launch --version <name> --username <nick> [--ram 4096]
                    [--server host[:port]] [--game-dir <dir>] [--dry-run]
```

Settings/accounts/servers/profiles/skins/logs live in the launcher home
(`RUSTLAUNCHER_HOME` overrides it; otherwise the OS data directory +
`RustLauncher`). The default game directory is `%APPDATA%\.rustlauncher`
(Windows) or `~/.rustlauncher`; override it in Settings or `--game-dir`.

## Differences from PowerLaunch

| Java PowerLaunch            | RustLauncher                          |
|-----------------------------|---------------------------------------|
| JavaFX controller, 4300 LoC | egui GUI, native binary               |
| SQLite settings/accounts    | typed JSON files (config/accounts/…)  |
| Manual JSON string search   | serde struct parsing                  |
| Could only list remote versions | Full install with SHA-1 checks   |
| servers.dat rewrite lost icons | NBT-preserving merge               |
| 5-minute kill timer in CLI  | Waits as long as the game runs        |
| Launcher JRE used for game  | Required Java major auto-selected     |

Not ported (by design): MSA online authentication, the tab system, the
installer/updater. Ported with behavior fixes: news feed URL, skin
validation, JVM-arg filter, offline UUID consistency between CLI and GUI.

## License

AGPL-3.0 — same as the original PowerLaunch.
