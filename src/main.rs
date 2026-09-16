//! RustLauncher — a Minecraft launcher rewritten in Rust.
//!
//! Successor of the Java/JavaFX PowerLaunch: the same launch pipeline
//! (version discovery, offline auth, classpath assembly, JVM arg handling)
//! plus a full GUI, without the JVM-inside-a-JVM overhead.
//!
//! Running without arguments opens the GUI; `versions` / `launch` remain
//! available as a scriptable CLI.

mod accounts;
mod auth;
mod classpath;
mod cli;
mod content;
mod diagnostics;
mod gui;
mod home;
mod java_locator;
mod jvm;
mod launcher;
mod logs;
mod nbt;
mod net;
mod news;
mod notifications;
mod profiles;
mod servers;
mod settings;
mod skins;
mod updater;
mod version;
mod version_json;

use anyhow::Result;
use clap::Parser as _;

use cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    let result = match &cli.command {
        None => cmd_gui(),
        Some(Command::Versions { game_dir }) => cmd_versions(game_dir.as_deref()),
        Some(Command::Launch {
            version,
            game_dir,
            username,
            ram,
            server,
            all_logs,
            dry_run,
        }) => cmd_launch(
            version,
            game_dir,
            username.as_deref(),
            ram,
            server.as_deref(),
            *all_logs,
            *dry_run,
        ),
    };

    if let Err(err) = result {
        eprintln!();
        eprintln!("  [ERROR] {err:#}");
        std::process::exit(1);
    }
}

fn cmd_gui() -> Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([900.0, 600.0])
            .with_icon(load_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "RustLauncher",
        native_options,
        Box::new(|cc| Ok(Box::new(gui::App::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("GUI failed: {e}"))
}

/// A simple built-in window icon (a green "play" triangle).
fn load_icon() -> egui::IconData {
    let size = 32usize;
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            // Triangle with vertices roughly at (8,6), (8,26), (26,16).
            let inside =
                (8..=26).contains(&x) && y >= 6 + (x - 8) * 5 / 9 && y <= 26 - (x - 8) * 5 / 9;
            if inside {
                let i = (y * size + x) * 4;
                rgba[i] = 60;
                rgba[i + 1] = 200;
                rgba[i + 2] = 90;
                rgba[i + 3] = 255;
            }
        }
    }
    egui::IconData {
        width: size as u32,
        height: size as u32,
        rgba,
    }
}

fn cmd_versions(game_dir: Option<&str>) -> Result<()> {
    let root = version::resolve_game_dir(game_dir)?;
    println!("Game directory: {}", root.display());
    println!();

    let versions = version::list_versions(&root)?;
    if versions.is_empty() {
        println!("No installed versions found.");
        println!("Expected layout: <game dir>/versions/<name>/<name>.json + <name>.jar");
        return Ok(());
    }

    println!("Installed versions ({}):", versions.len());
    for v in &versions {
        let marker = if v.jar.is_file() {
            ""
        } else {
            "  (jar missing!)"
        };
        println!("  - {}{marker}", v.name);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_launch(
    version_name: &str,
    game_dir: &str,
    username: Option<&str>,
    ram: &str,
    server: Option<&str>,
    all_logs: bool,
    dry_run: bool,
) -> Result<()> {
    println!("=== RustLauncher ===");
    println!();

    // The CLI never guesses the game directory: without --game-dir there is
    // nothing to launch from.
    if game_dir.trim().is_empty() {
        return Err(anyhow::anyhow!(
            "no game directory given: pass --game-dir <path> (versions live in <dir>/versions)"
        ));
    }
    let dir = game_dir.trim();
    let root = std::path::PathBuf::from(dir);
    println!("  Game dir: {}", root.display());

    if !root.is_dir() {
        return Err(anyhow::anyhow!(
            "game directory does not exist: {}",
            root.display()
        ));
    }

    // Account.
    let account = match username {
        Some(name) => {
            let account = auth::login_offline(name)?;
            println!("  Account:  {} (offline)", account.username);
            account
        }
        None => {
            return Err(anyhow::anyhow!(
                "no username given: pass --username <name> (3-16 chars, letters/digits/underscore)"
            ));
        }
    };

    // Version.
    let found = version::find_version(&root, version_name)?;
    if !found.jar.is_file() {
        return Err(anyhow::anyhow!(
            "version jar missing: {}",
            found.jar.display()
        ));
    }
    let json = version_json::VersionJson::load(&found.json)?;
    println!(
        "  Version:  {} (mainClass: {})",
        found.name,
        json.main_class()
    );
    if let Some(major) = json.required_java_major() {
        println!("  Java:     {major}+ required");
    }

    // --ram turns into the minimal JVM flag pair; flags are mandatory.
    let ram_trim = ram.trim();
    let ram_normalized = ram_trim.to_ascii_uppercase();
    let ram_value = if ram_normalized.ends_with('G') || ram_normalized.ends_with('M') {
        ram_trim.to_string()
    } else {
        format!("{ram_trim}m")
    };
    let jvm_flags = format!("-Xms1m -Xmx{ram_value}");

    let plan = launcher::build_launch_plan(
        &root,
        &found.name,
        &json,
        &found.jar,
        &account,
        &jvm_flags,
        None,
        server,
        None,
    )?;

    if dry_run {
        println!();
        println!("  Java: {}", plan.java.display());
        println!();
        println!("  Command:");
        for arg in &plan.args {
            println!("    {arg}");
        }
        return Ok(());
    }

    println!();
    println!("========================================");
    println!("  MINECRAFT OUTPUT");
    println!("========================================");
    println!();

    let handler = launcher::line_handler(move |line: &str| {
        if all_logs || is_error_line(line) {
            println!("{line}");
        }
    });
    let code = launcher::run_plan(plan, handler)?;

    println!();
    println!("========================================");
    if code == 0 {
        println!("  MINECRAFT EXITED SUCCESSFULLY (code: 0)");
    } else {
        println!("  MINECRAFT EXITED (code: {code})");
    }
    println!("========================================");
    std::process::exit(code);
}

/// The CLI defaults to error-only output; recognize common error markers.
fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("exception")
        || lower.contains("failed")
        || lower.contains("fatal")
        || lower.contains("crash")
}
