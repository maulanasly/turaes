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
    /// Exits non-zero when a check fails; `--json` emits machine output.
    Doctor {
        /// Emit the report (and check results) as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Take a timestamped SQLite snapshot into the backup dir (and prune).
    Backup,
    /// Restore the database from a snapshot file. Stop turaes first and pass
    /// `--force` to confirm.
    Restore {
        /// Snapshot file to restore from.
        file: PathBuf,
        /// Confirm turaes is stopped and the database may be replaced.
        #[arg(long)]
        force: bool,
    },
    /// Sealed secrets: per-app values plus rotation tooling (local database).
    Secrets {
        /// Which secrets operation to run.
        #[command(subcommand)]
        cmd: SecretsCommand,
        /// Organization id or slug to scope per-app operations to.
        #[arg(long, global = true)]
        org: Option<String>,
    },
    /// Delete artifact blobs no deployment references (plus stale uploads).
    Gc {
        /// Count and measure without deleting anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Serve a static directory on loopback (internal: supervision target for
    /// `static` apps; also handy for debugging).
    ServeStatic {
        /// Directory to serve.
        #[arg(long)]
        dir: PathBuf,
        /// Loopback port to bind.
        #[arg(long)]
        port: u16,
    },
    /// Apply a `turaes.yaml` manifest: create or update the app, domains, env
    /// and health checks (config only; deploy separately).
    Apply {
        /// Manifest file (defaults to `./turaes.yaml`).
        #[arg(long, short = 'f')]
        file: Option<PathBuf>,
        /// Organization id or slug to apply into.
        #[arg(long, global = true)]
        org: Option<String>,
        /// Print the diff without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// One-shot app operations against the local database (bootstrap/ops; no
    /// HTTP auth required). Names are globally unique; `--org` scopes lookups
    /// to one organization (default `default`).
    App {
        /// Which app operation to run.
        #[command(subcommand)]
        cmd: AppCommand,
        /// Organization id or slug to scope the operation to.
        #[arg(long, global = true)]
        org: Option<String>,
        /// Emit list/show output as JSON.
        #[arg(long, global = true)]
        json: bool,
    },
    /// Fleet node registry operations (local database; nodes are platform-global).
    Server {
        /// Which server operation to run.
        #[command(subcommand)]
        cmd: ServerCommand,
        /// Emit list output as JSON.
        #[arg(long, global = true)]
        json: bool,
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
        #[arg(long, env = "TURAES_AGENT_NAME")]
        name: String,
        /// Reachable address advertised to the control plane.
        #[arg(long, env = "TURAES_AGENT_ADDRESS", default_value = "")]
        address: String,
        /// Heartbeat interval in seconds.
        #[arg(long, env = "TURAES_AGENT_INTERVAL", default_value_t = 15)]
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

/// Sealed secrets: per-app values (manifest `secrets:` keys) and rotation.
#[derive(Debug, Subcommand)]
pub enum SecretsCommand {
    /// Re-seal every stored secret (app env vars, server SSH keys) with the
    /// primary cipher. Migrates legacy blobs; with TURAES_SECRET_PREVIOUS set,
    /// completes a rotation to the new secret.
    Reseal,
    /// Set a secret value (arg, `$KEY` env, or stdin when piped).
    Set {
        /// Application name.
        app: String,
        /// Secret key (`[A-Za-z_][A-Za-z0-9_]*`).
        key: String,
        /// Value (else read `$KEY` from the environment, else stdin).
        value: Option<String>,
    },
    /// Remove a secret value.
    Unset {
        /// Application name.
        app: String,
        /// Secret key.
        key: String,
    },
    /// List secret/env key names (values are never shown).
    List {
        /// Application name.
        app: String,
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
    /// Install + start the turaes agent on the node over SSH.
    Bootstrap {
        /// Server id or name.
        id: String,
    },
}

/// Local (non-HTTP) application management.
#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum AppCommand {
    /// Register an application.
    Add {
        /// Lowercase slug (unit + install name).
        #[arg(long)]
        name: String,
        /// Absolute path to the prebuilt binary (service/binary; defaults to
        /// `command[0]` / `publish_dir` for command/static apps).
        #[arg(long)]
        binary: Option<String>,
        /// Exec argv for interpreted apps; repeat per element
        /// (`--command /opt/venv/bin/python --command worker.py`).
        #[arg(long)]
        command: Option<Vec<String>>,
        /// Working directory override.
        #[arg(long)]
        workdir: Option<String>,
        /// Source directory synced for `static` apps.
        #[arg(long)]
        publish_dir: Option<String>,
        /// `service` (default), `static`, or `worker`.
        #[arg(long, default_value = "service")]
        kind: String,
        /// Loopback port (required for service/static; omit or 0 for workers).
        #[arg(long)]
        port: Option<u16>,
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
    /// Remove an application: stop its runtime units and delete it.
    Remove {
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
