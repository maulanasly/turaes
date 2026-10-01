//! Embedded process supervisor.
//!
//! Used when running without root or without systemd (local development, CI,
//! non-Linux hosts). Unlike systemd it has no cgroup accounting, so resource
//! stats fall back to `/proc` sampling in the monitor crate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::Command;

use turaes_core::{Error, Result};

use crate::runtime::{AppSpec, RunState, Runtime};

/// Supervisor that spawns child processes and tracks them by pid file.
#[derive(Debug, Clone)]
pub struct ProcRuntime {
    /// Root directory for pid/log files (usually each spec's `state_dir`).
    pub default_state_dir: PathBuf,
}

impl ProcRuntime {
    /// Create a runtime with a fallback state directory.
    pub fn new(default_state_dir: impl Into<PathBuf>) -> Self {
        Self {
            default_state_dir: default_state_dir.into(),
        }
    }

    fn pid_path(&self, spec: &AppSpec) -> PathBuf {
        Path::new(&spec.state_dir).join(format!("{}.pid", spec.name))
    }

    fn log_path(&self, spec: &AppSpec) -> PathBuf {
        Path::new(&spec.state_dir).join(format!("{}.log", spec.name))
    }

    async fn read_pid(&self, spec: &AppSpec) -> Option<u32> {
        let raw = tokio::fs::read_to_string(self.pid_path(spec)).await.ok()?;
        raw.trim().parse::<u32>().ok()
    }
}

/// Parse an `EnvironmentFile` body (KEY=VALUE, optional double quotes).
pub fn parse_env_file(body: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(v);
            map.insert(k.trim().to_string(), v.replace("\\\"", "\""));
        }
    }
    map
}

/// Keep the last `n` lines of a log body.
pub fn tail(body: &str, n: usize) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[async_trait]
impl Runtime for ProcRuntime {
    async fn apply(&self, spec: &AppSpec, env: &BTreeMap<String, String>) -> Result<()> {
        tokio::fs::create_dir_all(&spec.state_dir).await?;
        tokio::fs::copy(&spec.binary_path, &spec.installed_path)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "failed to install {} -> {}: {e}",
                    spec.binary_path, spec.installed_path
                ))
            })?;
        if let Some(env_file) = &spec.env_file {
            let body = env
                .iter()
                .map(|(k, v)| format!("{k}={v}\n"))
                .collect::<String>();
            tokio::fs::write(env_file, body).await?;
        }
        Ok(())
    }

    async fn start(&self, spec: &AppSpec) -> Result<()> {
        if matches!(self.status(spec).await?, RunState::Running) {
            return Ok(());
        }
        tokio::fs::create_dir_all(&spec.state_dir).await?;
        let env = match &spec.env_file {
            Some(path) => tokio::fs::read_to_string(path)
                .await
                .map(|body| parse_env_file(&body))
                .unwrap_or_default(),
            None => BTreeMap::new(),
        };

        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path(spec))?;
        let stderr = log.try_clone()?;

        let mut cmd = Command::new(&spec.installed_path);
        if let Some(args) = &spec.args {
            cmd.args(args.split_whitespace());
        }
        cmd.envs(&env)
            .env("PORT", spec.port.to_string())
            .current_dir(&spec.state_dir)
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr))
            .stdin(Stdio::null());

        let child = cmd.spawn().map_err(|e| {
            Error::Internal(format!("failed to spawn {}: {e}", spec.installed_path))
        })?;
        let pid = child.id().unwrap_or(0);
        tokio::fs::write(self.pid_path(spec), pid.to_string()).await?;
        Ok(())
    }

    async fn stop(&self, spec: &AppSpec) -> Result<()> {
        if let Some(pid) = self.read_pid(spec).await {
            let _ = Command::new("kill").arg(pid.to_string()).status().await;
            let _ = tokio::fs::remove_file(self.pid_path(spec)).await;
        }
        Ok(())
    }

    async fn restart(&self, spec: &AppSpec) -> Result<()> {
        self.stop(spec).await?;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        self.start(spec).await
    }

    async fn remove(&self, spec: &AppSpec) -> Result<()> {
        self.stop(spec).await?;
        let _ = tokio::fs::remove_file(self.log_path(spec)).await;
        Ok(())
    }

    async fn status(&self, spec: &AppSpec) -> Result<RunState> {
        let Some(pid) = self.read_pid(spec).await else {
            return Ok(RunState::Stopped);
        };
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        Ok(if alive {
            RunState::Running
        } else {
            RunState::Stopped
        })
    }

    async fn logs(&self, spec: &AppSpec, lines: usize) -> Result<String> {
        match tokio::fs::read_to_string(self.log_path(spec)).await {
            Ok(body) => Ok(tail(&body, lines)),
            Err(_) => Ok(String::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_roundtrip() {
        let body = "A=1\n# comment\nSPACED=\"a b\"\nEMPTY=\n";
        let env = parse_env_file(body);
        assert_eq!(env.get("A").unwrap(), "1");
        assert_eq!(env.get("SPACED").unwrap(), "a b");
        assert_eq!(env.get("EMPTY").unwrap(), "");
        assert!(!env.contains_key("# comment"));
    }

    #[test]
    fn tail_keeps_last_lines() {
        let body = "1\n2\n3\n4\n5";
        assert_eq!(tail(body, 2), "4\n5");
        assert_eq!(tail(body, 99), body);
    }

    #[test]
    fn pid_and_log_paths() {
        let rt = ProcRuntime::new("/tmp");
        let spec = AppSpec {
            name: "demo".into(),
            binary_path: "/bin/true".into(),
            installed_path: "/tmp/demo".into(),
            args: None,
            port: 9000,
            state_dir: "/var/lib/demo".into(),
            env_file: None,
            user: None,
        };
        assert!(rt.pid_path(&spec).ends_with("demo.pid"));
        assert!(rt.log_path(&spec).ends_with("demo.log"));
    }
}
