//! Live log streaming over WebSocket.
//!
//! Local apps stream `journalctl -f` (systemd) or `tail -f` (proc). Remote apps
//! are not supported here yet (agent log streaming is a later N-phase).

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::Extension;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use turaes_core::config::Config;
use turaes_core::models::Application;
use turaes_core::Result;

use crate::authz::{self, CurrentUser, Role};
use crate::routes::apps::fetch_org_app;
use crate::state::AppState;

/// Program + args used to follow an app's logs.
pub fn log_argv(cfg: &Config, app: &Application) -> (String, Vec<String>) {
    match app.runtime.as_str() {
        "proc" => {
            let log = format!("{}/{}/{}.log", cfg.runtime.state_dir, app.name, app.name);
            (
                "tail".into(),
                vec!["-n".into(), "50".into(), "-f".into(), log],
            )
        }
        _ => (
            "journalctl".into(),
            vec![
                "-u".into(),
                format!("{}.service", app.name),
                "-f".into(),
                "-n".into(),
                "50".into(),
                "--no-pager".into(),
                "-o".into(),
                "short-iso".into(),
            ],
        ),
    }
}

/// `GET /api/v1/orgs/{org}/apps/{id}/logs` — upgrade to a WebSocket log stream.
pub async fn stream(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Result<Response> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Developer).await?;
    let app = fetch_org_app(&state.pool, &org_id, &id).await?;
    let cfg = state.cfg.clone();
    Ok(ws.on_upgrade(move |socket| pump(cfg, app, socket)))
}

async fn pump(cfg: std::sync::Arc<Config>, app: Application, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();

    if app.server_id != "local" {
        let _ = sender
            .send(Message::Text(
                "remote logs are not supported yet (agent log streaming pending)".into(),
            ))
            .await;
        return;
    }

    let (program, args) = log_argv(&cfg, &app);
    let child = Command::new(&program)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            let _ = sender
                .send(Message::Text(
                    format!("failed to start {program}: {e}").into(),
                ))
                .await;
            return;
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            tokio::select! {
                incoming = receiver.next() => {
                    // Client closed (or sent something); either way, stop.
                    if incoming.is_none() { break; }
                }
                line = lines.next_line() => {
                    match line {
                        Ok(Some(text)) => {
                            if sender.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
            }
        }
    }

    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::log_argv;
    use turaes_core::config::Config;
    use turaes_core::models::Application;

    fn app(runtime: &str, name: &str) -> Application {
        Application {
            id: "x".into(),
            name: name.into(),
            description: None,
            binary_path: "/bin/true".into(),
            args: None,
            port: 1,
            active_port: None,
            health_path: "/health".into(),
            metrics_path: None,
            domain: None,
            server_id: "local".into(),
            org_id: "default".into(),
            runtime: runtime.into(),
            auto_restart: true,
            status: "running".into(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn systemd_uses_journalctl() {
        let cfg = Config::from_toml(include_str!("../../config/default.toml")).unwrap();
        let (program, args) = log_argv(&cfg, &app("systemd", "beruang"));
        assert_eq!(program, "journalctl");
        assert!(args.contains(&"beruang.service".to_string()));
        assert!(args.contains(&"-f".to_string()));
    }

    #[test]
    fn proc_uses_tail() {
        let cfg = Config::from_toml(include_str!("../../config/default.toml")).unwrap();
        let (program, args) = log_argv(&cfg, &app("proc", "demo"));
        assert_eq!(program, "tail");
        assert!(args.iter().any(|a| a.ends_with("/demo/demo.log")));
    }
}
