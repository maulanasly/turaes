//! SSH bootstrap: install and start the turaes agent on a fresh node.
//!
//! The control plane pushes its own binary, writes `/etc/turaes/agent.env` +
//! `turaes-agent.service`, and starts the unit. Uses the system `ssh`/`scp`
//! with a temporary private key (0600).

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use turaes_core::{Error, Result};

/// The systemd unit installed on the node.
pub const AGENT_UNIT: &str = "[Unit]\n\
Description=turaes agent (control-plane managed)\n\
After=network-online.target\n\
Wants=network-online.target\n\
\n\
[Service]\n\
Type=simple\n\
EnvironmentFile=/etc/turaes/agent.env\n\
ExecStart=/usr/local/bin/turaes agent\n\
Restart=always\n\
RestartSec=5\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n";

/// Everything needed to bootstrap a node.
#[derive(Debug, Clone)]
pub struct BootstrapPlan {
    /// SSH host.
    pub host: String,
    /// SSH port.
    pub port: u16,
    /// SSH user.
    pub user: String,
    /// SSH private key (PEM, decrypted).
    pub key_pem: String,
    /// Node name (agent identity).
    pub name: String,
    /// Node address advertised to the control plane.
    pub address: String,
    /// Control-plane gRPC URL, e.g. `http://10.0.0.2:9443`.
    pub control_url: String,
    /// Control-plane HTTP URL, e.g. `http://10.0.0.2:8787`.
    pub http_url: String,
    /// Shared join token.
    pub token: String,
}

impl BootstrapPlan {
    /// The `agent.env` written on the node.
    pub fn env_file(&self) -> String {
        format!(
            "TURAES_CONTROL_URL={}\n\
             TURAES_CONTROL_HTTP={}\n\
             TURAES_AGENT_TOKEN={}\n\
             TURAES_AGENT_NAME={}\n\
             TURAES_AGENT_ADDRESS={}\n\
             TURAES_RUNTIME_DRIVER=systemd\n\
             TURAES_LOG=info\n",
            self.control_url, self.http_url, self.token, self.name, self.address
        )
    }
}

fn ssh_base(key: &std::path::Path, plan: &BootstrapPlan) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.arg("-i")
        .arg(key)
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg("-p")
        .arg(plan.port.to_string())
        .arg(format!("{}@{}", plan.user, plan.host));
    cmd
}

/// Run the bootstrap over SSH. Returns the remote stdout on success.
pub async fn run(plan: &BootstrapPlan) -> Result<String> {
    let key_path = std::env::temp_dir().join(format!("turaes-bootstrap-{}", uuid::Uuid::new_v4()));
    tokio::fs::write(&key_path, &plan.key_pem)
        .await
        .map_err(|e| Error::Internal(format!("failed to write temp key: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).await;
    }

    let result = run_inner(plan, &key_path).await;
    let _ = tokio::fs::remove_file(&key_path).await;
    result
}

/// Warn when connecting to a host with no pinned key: `accept-new` trusts
/// pre-seeded `~/.ssh/known_hosts` entries, so operators can pin keys ahead
/// of time — but a first connection is pure TOFU and should be noticed.
fn warn_if_tofu(host: &str) {
    let known = std::process::Command::new("ssh-keygen")
        .arg("-F")
        .arg(host)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !known {
        tracing::warn!(
            host,
            "no pinned SSH host key found; trusting the presented key on first use (TOFU). \
             Pre-seed ~/.ssh/known_hosts to pin it"
        );
    }
}

async fn run_inner(plan: &BootstrapPlan, key_path: &std::path::Path) -> Result<String> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::Internal(format!("cannot locate own binary: {e}")))?;
    warn_if_tofu(&plan.host);

    // 1. Push the control-plane binary.
    let target = format!("{}@{}", plan.user, plan.host);
    let scp = Command::new("scp")
        .arg("-i")
        .arg(key_path)
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg("-P")
        .arg(plan.port.to_string())
        .arg(&exe)
        .arg(format!("{target}:/tmp/turaes-agent-bin"))
        .output()
        .await
        .map_err(|e| Error::Internal(format!("failed to run scp: {e}")))?;
    if !scp.status.success() {
        return Err(Error::Internal(format!(
            "scp failed: {}",
            String::from_utf8_lossy(&scp.stderr).trim()
        )));
    }

    // 2. Install binary, write env + unit, start the service.
    let script = format!(
        "set -e\n\
         install -m 0755 /tmp/turaes-agent-bin /usr/local/bin/turaes\n\
         rm -f /tmp/turaes-agent-bin\n\
         mkdir -p /etc/turaes\n\
         cat > /etc/turaes/agent.env <<'TURAES_ENV'\n{}\
         TURAES_ENV\n\
         chmod 0600 /etc/turaes/agent.env\n\
         cat > /etc/systemd/system/turaes-agent.service <<'TURAES_UNIT'\n{}\
         TURAES_UNIT\n\
         systemctl daemon-reload\n\
         systemctl enable --now turaes-agent\n\
         sleep 1\n\
         systemctl is-active turaes-agent\n",
        plan.env_file(),
        AGENT_UNIT
    );

    let mut child = ssh_base(key_path, plan)
        .arg("bash -s")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| Error::Internal(format!("failed to run ssh: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .await
            .map_err(|e| Error::Internal(format!("failed to send script: {e}")))?;
        drop(stdin);
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| Error::Internal(format!("ssh wait failed: {e}")))?;
    if !output.status.success() {
        return Err(Error::Internal(format!(
            "remote setup failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> BootstrapPlan {
        BootstrapPlan {
            host: "10.0.0.9".into(),
            port: 22,
            user: "root".into(),
            key_pem: "KEY".into(),
            name: "worker-1".into(),
            address: "10.0.0.9".into(),
            control_url: "http://10.0.0.2:9443".into(),
            http_url: "http://10.0.0.2:8787".into(),
            token: "secret".into(),
        }
    }

    #[test]
    fn env_file_has_expected_keys() {
        let env = plan().env_file();
        assert!(env.contains("TURAES_CONTROL_URL=http://10.0.0.2:9443"));
        assert!(env.contains("TURAES_CONTROL_HTTP=http://10.0.0.2:8787"));
        assert!(env.contains("TURAES_AGENT_NAME=worker-1"));
        assert!(env.contains("TURAES_RUNTIME_DRIVER=systemd"));
    }

    #[test]
    fn unit_runs_agent_with_env_file() {
        assert!(AGENT_UNIT.contains("EnvironmentFile=/etc/turaes/agent.env"));
        assert!(AGENT_UNIT.contains("ExecStart=/usr/local/bin/turaes agent"));
    }
}
