//! RustLauncher — a Minecraft launcher rewritten in Rust.
//!
//! Successor of the Java/JavaFX PowerLaunch: the same launch pipeline
//! (version discovery, offline auth, classpath assembly, JVM arg handling)
//! without a GUI, database, or the JVM-inside-a-JVM overhead.

mod auth;
mod classpath;
mod cli;
mod java_locator;
mod launcher;
mod version;
mod version_json;

use anyhow::Result;
use clap::Parser as _;

use cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Versions { game_dir } => cmd_versions(game_dir.as_deref()),
        Command::Launch {
            version,
            game_dir,
            username,
            ram,
            server,
            all_logs,
            dry_run,
        } => cmd_launch(
            version,
            game_dir.as_deref(),
            username.as_deref(),
            *ram,
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

fn cmd_launch(
    version_name: &str,
    game_dir: Option<&str>,
    username: Option<&str>,
    ram: u32,
    server: Option<&str>,
    all_logs: bool,
    dry_run: bool,
) -> Result<()> {
    println!("=== RustLauncher ===");
    println!();

    let root = version::resolve_game_dir(game_dir)?;
    println!("  Game dir: {}", root.display());

    if !root.is_dir() {
        return Err(anyhow::anyhow!(
            "game directory does not exist: {} (create it or pass --game-dir)",
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

    // Java selection happens inside the plan builder (needs required version).
    if dry_run {
        let plan = launcher::build_launch_plan(
            &root,
            &found.name,
            &json,
            &found.jar,
            &account,
            ram,
            "",
            None,
            server,
            None,
        )?;
        println!();
        println!("  Java: {}", plan.java.display());
        println!();
        println!("  Command:");
        for arg in &plan.args {
            println!("    {arg}");
        }
        return Ok(());
    }

    let plan = launcher::build_launch_plan(
        &root,
        &found.name,
        &json,
        &found.jar,
        &account,
        ram,
        "",
        None,
        server,
        None,
    )?;

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
