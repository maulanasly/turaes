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
    /// Resident memory ceiling in MiB (`MemoryMax=`); unset = unlimited.
    #[serde(default)]
    pub mem_limit_mb: Option<u64>,
    /// CPU ceiling in percent of one core (`CPUQuota=`); unset = unlimited.
    #[serde(default)]
    pub cpu_quota_pct: Option<u32>,
    /// Application kind: `service` (default), `static` (serve a directory),
    /// or `worker` (background process, no HTTP surface).
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Exec argv for interpreted apps; replaces the installed binary as the
    /// process image (no shell involved). Mutually exclusive with `args`.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Working directory override; defaults to `state_dir`.
    #[serde(default)]
    pub workdir: Option<String>,
    /// Source directory synced for `static` apps.
    #[serde(default)]
    pub publish_dir: Option<String>,
}

fn default_kind() -> String {
    "service".to_string()
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

    /// Effective working directory (`workdir` override or `state_dir`).
    pub fn working_dir(&self) -> String {
        self.workdir
            .clone()
            .unwrap_or_else(|| self.state_dir.clone())
    }

    /// Directory served for `static` apps, slot-scoped for atomic cutover.
    pub fn public_dir(&self) -> String {
        format!("{}/{}/public", self.state_dir, self.instance())
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

/// Sync a publish directory to `dest`, replacing it.
///
/// Removes `dest` first so deleted files disappear, then copies recursively.
/// Used for `static` apps; both slots keep their own copy for atomic cutover.
pub async fn sync_dir(src: &str, dest: &str) -> Result<()> {
    use turaes_core::Error;
    let src_path = std::path::Path::new(src);
    if !src_path.is_dir() {
        return Err(Error::BadRequest(format!(
            "publish_dir '{src}' is not a directory"
        )));
    }
    if tokio::fs::metadata(dest).await.is_ok() {
        tokio::fs::remove_dir_all(dest)
            .await
            .map_err(|e| Error::Internal(format!("failed to clear {dest}: {e}")))?;
    }
    let mut stack = vec![(src_path.to_path_buf(), std::path::PathBuf::from(dest))];
    while let Some((from_dir, to_dir)) = stack.pop() {
        tokio::fs::create_dir_all(&to_dir)
            .await
            .map_err(|e| Error::Internal(format!("failed to create {}: {e}", to_dir.display())))?;
        let mut entries = tokio::fs::read_dir(&from_dir)
            .await
            .map_err(|e| Error::Internal(format!("failed to read {}: {e}", from_dir.display())))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::Internal(format!("failed to list {}: {e}", from_dir.display())))?
        {
            let from = entry.path();
            let to = to_dir.join(entry.file_name());
            let ftype = entry
                .file_type()
                .await
                .map_err(|e| Error::Internal(format!("failed to stat {}: {e}", from.display())))?;
            if ftype.is_dir() {
                stack.push((from, to));
            } else if ftype.is_file() {
                tokio::fs::copy(&from, &to).await.map_err(|e| {
                    Error::Internal(format!("failed to copy {}: {e}", from.display()))
                })?;
            }
            // Symlinks and friends are skipped deliberately (no escape from dest).
        }
    }
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
