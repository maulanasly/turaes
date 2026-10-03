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
    /// Fleet node registry operations (local database).
    Server {
        /// Which server operation to run.
        #[command(subcommand)]
        cmd: ServerCommand,
    },
    /// Run as a node agent: register with the control plane and heartbeat.
    Agent {
        /// Control-plane gRPC endpoint, e.g. http://10.0.0.2:9443.
        #[arg(long, env = "TURAES_CONTROL_URL")]
        control: String,
        /// Control-plane HTTP base for artifacts, e.g. http://10.0.0.2:8787.
        #[arg(long, env = "TURAES_CONTROL_HTTP")]
        http: String,
        /// Shared join token.
        #[arg(long, env = "TURAES_AGENT_TOKEN")]
        token: String,
        /// Node name (unique in the fleet).
        #[arg(long)]
        name: String,
        /// Reachable address advertised to the control plane.
        #[arg(long, default_value = "")]
        address: String,
        /// Heartbeat interval in seconds.
        #[arg(long, default_value_t = 15)]
        interval: u64,
    },
    /// Run as a standalone Pingora edge that pulls routes from the control plane.
    /// Requires a build with `--features proxy`.
    Edge {
        /// Control-plane gRPC endpoint, e.g. http://10.0.0.2:9443.
        #[arg(long, env = "TURAES_CONTROL_URL")]
        control: String,
        /// Shared control token.
        #[arg(long, env = "TURAES_AGENT_TOKEN")]
        token: String,
        /// Route refresh interval in seconds.
        #[arg(long, default_value_t = 10)]
        interval: u64,
    },
}

/// Local (non-HTTP) server registry management.
#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    /// Register a server.
    Add {
        /// Unique name.
        #[arg(long)]
        name: String,
        /// Reachable address (private IP in the VPC).
        #[arg(long)]
        address: String,
        /// SSH host for bootstrap.
        #[arg(long)]
        ssh_host: Option<String>,
        /// SSH port.
        #[arg(long)]
        ssh_port: Option<i64>,
        /// SSH user.
        #[arg(long)]
        ssh_user: Option<String>,
        /// Path to an SSH private key file (sealed at rest).
        #[arg(long)]
        ssh_key_file: Option<String>,
    },
    /// List servers.
    List,
    /// Remove a server by id or name.
    Remove {
        /// Server id or name.
        id: String,
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
    /// Roll back an application to its previous artifact.
    Rollback {
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
