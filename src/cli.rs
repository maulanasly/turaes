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
    /// One-shot app operations against the local database (bootstrap/ops; no
    /// HTTP auth required).
    App {
        /// Which app operation to run.
        #[command(subcommand)]
        cmd: AppCommand,
    },
}

/// Local (non-HTTP) application management.
#[derive(Debug, Subcommand)]
pub enum AppCommand {
    /// Register an application.
    Add {
        /// Lowercase slug (unit + install name).
        #[arg(long)]
        name: String,
        /// Absolute path to the prebuilt binary.
        #[arg(long)]
        binary: String,
        /// Loopback port.
        #[arg(long)]
        port: u16,
        /// Primary hostname.
        #[arg(long)]
        domain: Option<String>,
        /// Health path.
        #[arg(long, default_value = "/health")]
        health: String,
        /// Metrics path.
        #[arg(long, default_value = "/metrics")]
        metrics: String,
        /// `systemd` or `proc`.
        #[arg(long, default_value = "systemd")]
        runtime: String,
        /// Extra ExecStart arguments.
        #[arg(long)]
        args: Option<String>,
    },
    /// Deploy (install + restart) an application by name.
    Deploy {
        /// Application name.
        name: String,
    },
    /// List applications.
    List,
    /// Show an application's status and latest metrics/visitors.
    Show {
        /// Application name.
        name: String,
    },
}
