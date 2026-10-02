//! systemd runtime: turaes generates a unit per application, matching the
//! hardened shape used by beruang and monthly-logs (`ProtectSystem=strict`,
//! `NoNewPrivileges`, `Restart=always`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::process::Command;

use turaes_core::{Error, Result};

use crate::runtime::{AppSpec, RunState, Runtime};

/// systemd-backed runtime.
#[derive(Debug, Clone)]
pub struct SystemdRuntime {
    /// Directory containing `.service` units.
    pub unit_dir: PathBuf,
    /// Directory where app binaries are installed.
    pub bin_dir: PathBuf,
}

impl SystemdRuntime {
    /// Create a runtime rooted at the given directories.
    pub fn new(unit_dir: impl Into<PathBuf>, bin_dir: impl Into<PathBuf>) -> Self {
        Self {
            unit_dir: unit_dir.into(),
            bin_dir: bin_dir.into(),
        }
    }

    /// Absolute path to an app's unit file.
    pub fn unit_path(&self, spec: &AppSpec) -> PathBuf {
        self.unit_dir.join(spec.unit_name())
    }

    /// Create the service user/group if it does not already exist.
    async fn ensure_user(&self, spec: &AppSpec) -> Result<()> {
        let user = spec.user();
        let exists = Command::new("id")
            .arg("-u")
            .arg(&user)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if exists {
            return Ok(());
        }
        let output = Command::new("useradd")
            .arg("--system")
            .arg("--no-create-home")
            .arg("--home-dir")
            .arg(&spec.state_dir)
            .arg("--shell")
            .arg("/usr/sbin/nologin")
            .arg(&user)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("failed to run useradd: {e}")))?;
        if !output.status.success() {
            return Err(Error::Internal(format!(
                "failed to create service user '{user}': {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Give the service user ownership of its state directory.
    async fn chown_state(&self, spec: &AppSpec) {
        let user = spec.user();
        let owner = format!("{user}:{user}");
        let _ = Command::new("chown")
            .arg("-R")
            .arg(&owner)
            .arg(&spec.state_dir)
            .output()
            .await;
        if let Some(env_file) = &spec.env_file {
            let _ = Command::new("chown")
                .arg(&owner)
                .arg(env_file)
                .output()
                .await;
        }
    }

    async fn systemctl(&self, args: &[&str]) -> Result<()> {
        let output = Command::new("systemctl")
            .args(args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("failed to run systemctl: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "systemctl {} failed: {}",
                args.join(" "),
                stderr.trim()
            )));
        }
        Ok(())
    }
}

/// Render a hardened systemd unit for an app. Pure so it is trivially testable.
pub fn render_unit(spec: &AppSpec) -> String {
    let args = spec
        .args
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!(" {s}"))
        .unwrap_or_default();
    let env_file = spec
        .env_file
        .as_deref()
        .map(|p| format!("EnvironmentFile={p}\n"))
        .unwrap_or_default();
    let user = spec.user();
    format!(
        "[Unit]\n\
         Description=turaes-managed application ({name})\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         User={user}\n\
         Group={user}\n\
         WorkingDirectory={state_dir}\n\
         {env_file}\
         Environment=PORT={port}\n\
         ExecStart={exec}{args}\n\
         Restart=always\n\
         RestartSec=5\n\
         NoNewPrivileges=true\n\
         PrivateTmp=true\n\
         ProtectSystem=strict\n\
         ReadWritePaths={state_dir}\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        name = spec.name,
        user = user,
        state_dir = spec.state_dir,
        env_file = env_file,
        port = spec.port,
        exec = spec.installed_path,
        args = args,
    )
}

/// Render an `EnvironmentFile` body from key/value pairs.
pub fn render_env_file(env: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (k, v) in env {
        // systemd EnvironmentFile is plain KEY=VALUE; quote values with spaces.
        if v.contains(' ') || v.contains('#') {
            out.push_str(&format!("{k}=\"{}\"\n", v.replace('"', "\\\"")));
        } else {
            out.push_str(&format!("{k}={v}\n"));
        }
    }
    out
}

#[async_trait]
impl Runtime for SystemdRuntime {
    async fn apply(&self, spec: &AppSpec, env: &BTreeMap<String, String>) -> Result<()> {
        // 1. Install the binary atomically (safe while the old copy runs).
        crate::runtime::install_binary(&spec.binary_path, &spec.installed_path).await?;

        // 2. Service user, state dir + env file.
        self.ensure_user(spec).await?;
        tokio::fs::create_dir_all(&spec.state_dir).await?;
        if let Some(env_file) = &spec.env_file {
            if let Some(parent) = Path::new(env_file).parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(env_file, render_env_file(env)).await?;
        }
        self.chown_state(spec).await;

        // 3. Unit file.
        tokio::fs::create_dir_all(&self.unit_dir).await?;
        tokio::fs::write(self.unit_path(spec), render_unit(spec)).await?;
        self.systemctl(&["daemon-reload"]).await?;
        self.systemctl(&["enable", &spec.unit_name()]).await?;
        Ok(())
    }

    async fn start(&self, spec: &AppSpec) -> Result<()> {
        self.systemctl(&["start", &spec.unit_name()]).await
    }

    async fn stop(&self, spec: &AppSpec) -> Result<()> {
        self.systemctl(&["stop", &spec.unit_name()]).await
    }

    async fn restart(&self, spec: &AppSpec) -> Result<()> {
        self.systemctl(&["restart", &spec.unit_name()]).await
    }

    async fn remove(&self, spec: &AppSpec) -> Result<()> {
        let _ = self
            .systemctl(&["disable", "--now", &spec.unit_name()])
            .await;
        let _ = tokio::fs::remove_file(self.unit_path(spec)).await;
        let _ = self.systemctl(&["daemon-reload"]).await;
        Ok(())
    }

    async fn status(&self, spec: &AppSpec) -> Result<RunState> {
        let output = Command::new("systemctl")
            .args(["is-active", &spec.unit_name()])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("failed to run systemctl: {e}")))?;
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(match state.as_str() {
            "active" => RunState::Running,
            "activating" | "reloading" => RunState::Running,
            "inactive" | "deactivating" => RunState::Stopped,
            "failed" => RunState::Failed,
            _ => RunState::Unknown,
        })
    }

    async fn logs(&self, spec: &AppSpec, lines: usize) -> Result<String> {
        let output = Command::new("journalctl")
            .args([
                "-u",
                &spec.unit_name(),
                "-n",
                &lines.to_string(),
                "--no-pager",
                "-o",
                "short-iso",
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("failed to run journalctl: {e}")))?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AppSpec {
        AppSpec {
            name: "beruang".into(),
            binary_path: "/tmp/beruang-gateway".into(),
            installed_path: "/usr/local/bin/beruang-gateway".into(),
            args: None,
            port: 8000,
            state_dir: "/var/lib/beruang".into(),
            env_file: Some("/etc/beruang.env".into()),
            user: None,
        }
    }

    #[test]
    fn unit_contains_hardening_and_execstart() {
        let unit = render_unit(&spec());
        assert!(unit.contains("ExecStart=/usr/local/bin/beruang-gateway"));
        assert!(unit.contains("User=beruang"));
        assert!(unit.contains("EnvironmentFile=/etc/beruang.env"));
        assert!(unit.contains("Environment=PORT=8000"));
        assert!(unit.contains("ProtectSystem=strict"));
        assert!(unit.contains("ReadWritePaths=/var/lib/beruang"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn unit_appends_args() {
        let mut s = spec();
        s.args = Some("--verbose --port 8000".into());
        let unit = render_unit(&s);
        assert!(unit.contains("ExecStart=/usr/local/bin/beruang-gateway --verbose --port 8000"));
    }

    #[test]
    fn env_file_quotes_spaces() {
        let mut env = BTreeMap::new();
        env.insert("PLAIN".to_string(), "value".to_string());
        env.insert("SPACED".to_string(), "a b".to_string());
        let body = render_env_file(&env);
        assert!(body.contains("PLAIN=value"));
        assert!(body.contains("SPACED=\"a b\""));
    }
}
