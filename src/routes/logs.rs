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
        _ => {
            // Blue/green units are named `{name}-a` / `{name}-b`; the bare
            // `{name}.service` never exists, and journalctl matches nothing
            // for it (empty stream, no error). Tail the active slot, derived
            // from active_port the same way cutover does; before the first
            // deploy (active_port None) cover both slots plus the legacy
            // bare name — journalctl ignores units with no entries.
            let offset = cfg.runtime.slot_offset as i64;
            let units: Vec<String> = match app.active_port {
                Some(ap) if ap == app.port + offset => {
                    vec![format!("{}-b.service", app.name)]
                }
                Some(_) => vec![format!("{}-a.service", app.name)],
                None => vec![
                    format!("{}.service", app.name),
                    format!("{}-a.service", app.name),
                    format!("{}-b.service", app.name),
                ],
            };
            let mut args = Vec::with_capacity(units.len() * 2 + 6);
            for u in units {
                args.push("-u".to_string());
                args.push(u);
            }
            args.extend(
                ["-f", "-n", "50", "--no-pager", "-o", "short-iso"]
                    .iter()
                    .map(|s| s.to_string()),
            );
            ("journalctl".into(), args)
        }
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

    fn app(runtime: &str, name: &str, active_port: Option<i64>) -> Application {
        Application {
            id: "x".into(),
            name: name.into(),
            description: None,
            binary_path: "/bin/true".into(),
            args: None,
            port: 8000,
            active_port,
            health_path: "/health".into(),
            metrics_path: None,
            domain: None,
            server_id: "local".into(),
            org_id: "default".into(),
            mem_limit_mb: None,
            cpu_quota_pct: None,
            kind: "service".into(),
            command: None,
            workdir: None,
            publish_dir: None,
            runtime: runtime.into(),
            auto_restart: true,
            status: "running".into(),
            maintenance: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn systemd_tails_the_active_slot() {
        let cfg = Config::from_toml(include_str!("../../config/default.toml")).unwrap();
        // Slot A serves the base port.
        let (program, args) = log_argv(&cfg, &app("systemd", "beruang", Some(8000)));
        assert_eq!(program, "journalctl");
        assert!(args.contains(&"beruang-a.service".to_string()));
        assert!(!args.contains(&"beruang-b.service".to_string()));
        assert!(!args.contains(&"beruang.service".to_string()));
        assert!(args.contains(&"-f".to_string()));
        // Slot B serves base port + slot_offset.
        let b_port = 8000 + cfg.runtime.slot_offset as i64;
        let (program, args) = log_argv(&cfg, &app("systemd", "beruang", Some(b_port)));
        assert_eq!(program, "journalctl");
        assert!(args.contains(&"beruang-b.service".to_string()));
        assert!(!args.contains(&"beruang-a.service".to_string()));
    }

    #[test]
    fn systemd_covers_all_units_before_the_first_deploy() {
        // active_port None: tail both slots plus the legacy bare unit name —
        // journalctl skips units with no entries, so this is always safe.
        let cfg = Config::from_toml(include_str!("../../config/default.toml")).unwrap();
        let (program, args) = log_argv(&cfg, &app("systemd", "beruang", None));
        assert_eq!(program, "journalctl");
        assert!(args.contains(&"beruang.service".to_string()));
        assert!(args.contains(&"beruang-a.service".to_string()));
        assert!(args.contains(&"beruang-b.service".to_string()));
    }

    #[test]
    fn proc_uses_tail() {
        let cfg = Config::from_toml(include_str!("../../config/default.toml")).unwrap();
        let (program, args) = log_argv(&cfg, &app("proc", "demo", None));
        assert_eq!(program, "tail");
        assert!(args.iter().any(|a| a.ends_with("/demo/demo.log")));
    }
}
