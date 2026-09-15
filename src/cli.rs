//! CLI definition for RustLauncher (clap).

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "rustlauncher",
    version,
    about = "RustLauncher — a Minecraft launcher with GUI (offline accounts)",
    arg_required_else_help = false
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// List Minecraft versions found in the game directory.
    Versions {
        /// Game directory override (defaults to auto-detection).
        #[arg(long, short = 'g')]
        game_dir: Option<String>,
    },
    /// Launch a Minecraft version.
    Launch {
        /// Version name (a folder under versions/ with <name>.jar + <name>.json).
        #[arg(long, short = 'v')]
        version: String,

        /// Game directory override (defaults to auto-detection).
        #[arg(long, short = 'g')]
        game_dir: Option<String>,

        /// Offline username (letters, digits, underscores; 3–16 chars).
        #[arg(long, short = 'u')]
        username: Option<String>,

        /// RAM for the game in MB (minimum 512).
        #[arg(long, short = 'r', default_value_t = 4096)]
        ram: u32,

        /// Server address to auto-connect to (host or host:port).
        #[arg(long, short = 's')]
        server: Option<String>,

        /// Print every log line from the game (default: errors only).
        #[arg(long, short = 'a', default_value_t = false)]
        all_logs: bool,

        /// Print the launch command instead of running it.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}
