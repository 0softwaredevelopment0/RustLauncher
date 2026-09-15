# RustLauncher

![Development status](https://img.shields.io/badge/status-alpha-orange)

A Minecraft launcher rewritten in **Rust** — the successor of the Java/JavaFX
[PowerLaunch](https://github.com/rizer001-Development). Same launch pipeline,
no JVM-inside-a-JVM: the launcher itself is a small native binary.

## What it does

- **Version discovery** — scans the game directory (`versions/<name>/<name>.json`
  + `<name>.jar`, including `.minecraft/` layouts) like the official launcher.
- **Offline accounts** — the standard offline scheme: UUID = md5 of
  `OfflinePlayer:<name>` (v3 UUID), with Minecraft's username rules enforced.
- **Proper version.json parsing** — structural JSON parsing (serde), not the
  string-searching the Java version used. Reads `mainClass`, `jar` reference,
  `assetIndex`/`assets`, `javaVersion.majorVersion`, and `libraries`.
- **Java selection** — probes `java -version`, picks the best installed JRE/JDK
  for the game's required major version (PATH first, then common Windows
  install locations), supports a bundled `runtime/` next to the binary and an
  explicit `--java-path` override.
- **Classpath assembly** — recursive jar collection, Maven-artifact
  deduplication (keeps the newest version), the Gson ≥ 2.14 fallback
  (`setStrictness` was removed there), LWJGL 2/3 conflict resolution, and the
  Fabric special case: only the version jar + bootstrap libraries on `-cp`
  (Fabric's own classloader loads the rest).
- **Safe custom JVM args** — the same blocklist as PowerLaunch: duplicate heap
  flags, proxy/DNS hijacks, TLS downgrades, `-javaagent`, bootclasspath
  injection, and JDWP are dropped before launch.
- **Natives extraction** — unpacks `.dll/.so/.dylib` from `*natives*.jar` into
  the natives directory on every launch.

## Bugs fixed relative to PowerLaunch (Java)

1. **version.json was parsed with `indexOf("\"mainClass\"")`-style string
   searches** — fields inside string values or unrelated objects could match,
   and the parser broke on any reordering. RustLauncher parses JSON
   structurally.
2. **`getJavaPath()` returned the launcher's own `java.home`** — the game ran
   on the launcher's JRE even when the game needed a different major version.
   Now the required version from version.json drives selection explicitly.
3. **The CLI had no `--java-path`** — a broken configured Java path could only
   be fixed in the GUI database. Now it is a launch flag, validated up front.
4. **`--accessToken` used `auth.getUuid().toString()`** while the `--uuid`
   argument used the stripped form — inconsistent on some server stacks; both
   are now derived from the same canonical UUID.
5. **A 5-minute hard timeout killed the game** in CLI mode
   (`launchDone.await(5, TimeUnit.MINUTES)`). Minecraft sessions last far
   longer; the Rust CLI waits for as long as the game runs.
6. **The dedup and LWJGL logic kept hidden files and non-jars in the scan** and
   sorted by absolute string, which could put library jars in the wrong order
   relative to the version jar. Collection is now typed, sorted, and the
   version jar's position is explicit.
7. **Offline usernames were validated in three places with different rules** —
   validation now lives in one function with tests (3–16 chars,
   letters/digits/underscores).

## Usage

```sh
# List installed versions
rustlauncher versions --game-dir C:\Users\me\AppData\Roaming\.minecraft

# Launch
rustlauncher launch --version 1.21.4 --username Rizer001 --ram 4096

# Auto-connect to a server
rustlauncher launch --version 1.7.10 --username Rizer001 --server mc.example.com:25565

# Preview the command without running it
rustlauncher launch --version fabric-1.20.1 --username Rizer001 --dry-run
```

Default game directory: `%APPDATA%\.rustlauncher` (Windows),
`~/Library/Application Support/.rustlauncher` (macOS), `~/.rustlauncher`
(Linux). Override with `--game-dir`.

## Building

```sh
cargo build --release
```

The binary is `target/release/rustlauncher.exe` (single file, no runtime
dependencies).

## Project structure

```
RustLauncher/
└── src/
    ├── main.rs          — entry point, subcommand dispatch
    ├── cli.rs           — clap CLI definition
    ├── version.rs       — game dir + installed-version discovery
    ├── version_json.rs  — structural version.json parsing
    ├── auth.rs          — offline accounts (UUID v3 scheme)
    ├── java_locator.rs  — Java discovery and version probing
    ├── classpath.rs     — classpath assembly, dedup, LWJGL conflicts
    └── launcher.rs      — launch plan, arg filtering, process handling
```

## License

This project is licensed under the **GNU Affero General Public License v3.0**.
See the [LICENSE](./LICENSE) file for details.
