//! The runtime abstraction: everything turaes needs to run an app as a native
//! process, independent of *how* it is supervised.
//!
//! Two implementations ship today:
//! - [`crate::systemd::SystemdRuntime`] — generated units, cgroup accounting,
//!   journald logs (the production default on Linux).
//! - [`crate::proc::ProcRuntime`] — an embedded supervisor that spawns child
//!   processes directly (fallback/dev, no root required).

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use turaes_core::Result;

/// Everything the runtime needs to manage one application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSpec {
    /// Application slug (also the unit/install name).
    pub name: String,
    /// Source path of the prebuilt binary.
    pub binary_path: String,
    /// Where the binary is installed on the server.
    pub installed_path: String,
    /// Optional extra arguments appended to `ExecStart`.
    pub args: Option<String>,
    /// Port the process should bind (exported as `PORT`).
    pub port: u16,
    /// Per-app working/state directory.
    pub state_dir: String,
    /// Environment file path, if any.
    pub env_file: Option<String>,
    /// systemd `User=`/`Group=` (defaults to the app name).
    pub user: Option<String>,
}

impl AppSpec {
    /// systemd unit name, e.g. `beruang.service`.
    pub fn unit_name(&self) -> String {
        format!("{}.service", self.name)
    }

    /// Effective service user (falls back to the app name).
    pub fn user(&self) -> String {
        self.user.clone().unwrap_or_else(|| self.name.clone())
    }
}

/// Observed runtime state of an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    /// Running normally.
    Running,
    /// Stopped.
    Stopped,
    /// Failed / crash-looping.
    Failed,
    /// Could not be determined.
    Unknown,
}

impl RunState {
    /// Map to the application `status` string persisted in the database.
    pub fn as_status(self) -> &'static str {
        match self {
            RunState::Running => "running",
            RunState::Stopped => "stopped",
            RunState::Failed => "failed",
            RunState::Unknown => "unknown",
        }
    }
}

/// A supervisor backend that can install, run and observe a native app.
#[async_trait]
pub trait Runtime: Send + Sync {
    /// Install/update the artifact and (re)write supervision config.
    async fn apply(&self, spec: &AppSpec, env: &BTreeMap<String, String>) -> Result<()>;

    /// Start the app (idempotent).
    async fn start(&self, spec: &AppSpec) -> Result<()>;

    /// Stop the app (idempotent).
    async fn stop(&self, spec: &AppSpec) -> Result<()>;

    /// Restart the app.
    async fn restart(&self, spec: &AppSpec) -> Result<()>;

    /// Remove the app and its supervision config.
    async fn remove(&self, spec: &AppSpec) -> Result<()>;

    /// Query the current state.
    async fn status(&self, spec: &AppSpec) -> Result<RunState>;

    /// Recent log lines.
    async fn logs(&self, spec: &AppSpec, lines: usize) -> Result<String>;
}
