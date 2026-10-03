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

/// Blue/green slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    /// Slot A (base port).
    A,
    /// Slot B (base port + offset).
    B,
}

impl Slot {
    /// Lowercase label used in unit/state names.
    pub fn as_str(self) -> &'static str {
        match self {
            Slot::A => "a",
            Slot::B => "b",
        }
    }
}

/// Everything the runtime needs to manage one application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSpec {
    /// Application slug (also the unit/install name).
    pub name: String,
    /// Blue/green slot, when running slot-scoped instances.
    #[serde(default)]
    pub slot: Option<Slot>,
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
    /// Instance label (`name` or `name-a`/`name-b` when slotted).
    pub fn instance(&self) -> String {
        match self.slot {
            Some(slot) => format!("{}-{}", self.name, slot.as_str()),
            None => self.name.clone(),
        }
    }

    /// systemd unit name, e.g. `beruang.service` or `beruang-a.service`.
    pub fn unit_name(&self) -> String {
        format!("{}.service", self.instance())
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

/// Install a binary to `dest` atomically.
///
/// Writes `dest.turaes-new-<pid>` then renames it into place. Renaming replaces
/// the path without touching the running process's inode, avoiding
/// `ETXTBSY` ("Text file busy") when redeploying an app that is executing.
pub async fn install_binary(src: &str, dest: &str) -> Result<()> {
    let dest_path = std::path::Path::new(dest);
    if let Some(parent) = dest_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = format!("{dest}.turaes-new-{}", std::process::id());
    tokio::fs::copy(src, &tmp).await.map_err(|e| {
        turaes_core::Error::Internal(format!("failed to stage {src} -> {tmp}: {e}"))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mut perms) = tokio::fs::metadata(&tmp).await.map(|m| m.permissions()) {
            perms.set_mode(0o755);
            let _ = tokio::fs::set_permissions(&tmp, perms).await;
        }
    }

    tokio::fs::rename(&tmp, dest)
        .await
        .map_err(|e| turaes_core::Error::Internal(format!("failed to install {dest}: {e}")))?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::install_binary;

    #[tokio::test]
    async fn install_binary_replaces_and_sets_exec() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        tokio::fs::write(&src, b"#!/bin/sh\necho hi\n")
            .await
            .unwrap();
        let dest = dir.path().join("dest");
        // Simulate an existing (possibly running) installed binary.
        tokio::fs::write(&dest, b"old").await.unwrap();

        install_binary(src.to_str().unwrap(), dest.to_str().unwrap())
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read(&dest).await.unwrap(),
            b"#!/bin/sh\necho hi\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = tokio::fs::metadata(&dest)
                .await
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }
}
