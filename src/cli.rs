//! Command-line interface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// turaes — a lightweight, self-hosted, Docker-less deployment platform.
#[derive(Debug, Parser)]
#[command(name = "turaes", version, about, long_about = None)]
pub struct Cli {
    /// Path to a TOML config file (defaults to embedded config + env overrides).
    #[arg(long, global = true, env = "TURAES_CONFIG")]
    pub config: Option<PathBuf>,

    /// Subcommand (defaults to `serve`).
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the API, dashboard and (optionally) the proxy.
    Serve,
    /// Apply database migrations and exit.
    Migrate,
    /// Print resolved configuration (secrets redacted) and capability checks.
    Doctor,
}
