//! Server (node) registry. The local host is the `local` row; remote nodes are
//! added here (SSH metadata only for now — bootstrap lands in N1).

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use turaes_core::models::Server;
use turaes_core::{Error, Result};

use crate::state::AppState;

/// Body for registering a server.
#[derive(Debug, Deserialize)]
pub struct CreateServer {
    /// Unique human name.
    pub name: String,
    /// Reachable address (private IP in the VPC).
    pub address: String,
    /// SSH host, for bootstrap.
    pub ssh_host: Option<String>,
    /// SSH port (default 22).
    pub ssh_port: Option<i64>,
    /// SSH user.
    pub ssh_user: Option<String>,
    /// SSH private key (sealed at rest; never returned).
    pub ssh_key: Option<String>,
}

/// `GET /api/v1/servers`
pub async fn list(State(state): State<AppState>) -> Result<Json<serde_json::Value>> {
    let servers =
        sqlx::query_as::<_, Server>("SELECT * FROM servers ORDER BY is_local DESC, name ASC")
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(serde_json::json!({ "servers": servers })))
}

/// `POST /api/v1/servers`
pub async fn create(
    State(state): State<AppState>,
    Json(input): Json<CreateServer>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    if input.name.trim().is_empty() || input.address.trim().is_empty() {
        return Err(Error::BadRequest("name and address are required".into()));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let ssh_key_enc = match input.ssh_key.as_deref() {
        Some(k) if !k.is_empty() => Some(state.secrets.seal(k)?),
        _ => None,
    };
    let server = sqlx::query_as::<_, Server>(
        "INSERT INTO servers \
         (id, name, address, ssh_host, ssh_port, ssh_user, ssh_key_enc, is_local, status) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 0, 'unknown') RETURNING *",
    )
    .bind(&id)
    .bind(&input.name)
    .bind(&input.address)
    .bind(&input.ssh_host)
    .bind(input.ssh_port)
    .bind(&input.ssh_user)
    .bind(&ssh_key_enc)
    .fetch_one(&state.pool)
    .await
    .map_err(map_unique_name)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "server": server })),
    ))
}

fn map_unique_name(e: sqlx::Error) -> Error {
    if let sqlx::Error::Database(db) = &e {
        if db.message().contains("UNIQUE") {
            return Error::Conflict("a server with that name already exists".into());
        }
    }
    Error::Db(e)
}

/// `GET /api/v1/servers/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let server = fetch_server(&state, &id).await?;
    Ok(Json(serde_json::json!({ "server": server })))
}

/// `DELETE /api/v1/servers/{id}`
pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    if id == "local" {
        return Err(Error::BadRequest(
            "the local server cannot be removed".into(),
        ));
    }
    let in_use: i64 = sqlx::query_scalar("SELECT count(*) FROM applications WHERE server_id = ?")
        .bind(&id)
        .fetch_one(&state.pool)
        .await?;
    if in_use > 0 {
        return Err(Error::Conflict(format!(
            "{in_use} application(s) are still placed on this server"
        )));
    }
    let affected = sqlx::query("DELETE FROM servers WHERE id = ?")
        .bind(&id)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(Error::NotFound(format!("server {id}")));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/servers/{id}/validate` — local is always reachable; remote
/// checks TCP connectivity to the SSH endpoint.
pub async fn validate(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let server = fetch_server(&state, &id).await?;
    if server.is_local {
        return Ok(Json(
            serde_json::json!({ "reachable": true, "status": "online" }),
        ));
    }
    let target = match &server.ssh_host {
        Some(h) => format!("{}:{}", h, server.ssh_port.unwrap_or(22)),
        None => format!("{}:22", server.address),
    };
    let reachable = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(&target),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false);
    let status = if reachable { "online" } else { "offline" };
    let _ = sqlx::query("UPDATE servers SET status = ?, last_seen_at = CASE WHEN ?='online' THEN datetime('now') ELSE last_seen_at END WHERE id = ?")
        .bind(status)
        .bind(status)
        .bind(&id)
        .execute(&state.pool)
        .await;
    Ok(Json(
        serde_json::json!({ "reachable": reachable, "status": status }),
    ))
}

/// `POST /api/v1/servers/{id}/bootstrap` — install + start the agent over SSH.
pub async fn bootstrap(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let output = run_bootstrap(&state, &id).await?;
    Ok(Json(serde_json::json!({ "ok": true, "output": output })))
}

/// Shared by the HTTP handler and the `turaes server bootstrap` CLI command.
pub async fn run_bootstrap(state: &AppState, id: &str) -> Result<String> {
    let server = fetch_server(state, id).await?;
    if server.is_local {
        return Err(Error::BadRequest(
            "the local server cannot be bootstrapped".into(),
        ));
    }
    if state.cfg.agent.join_token.is_empty() {
        return Err(Error::BadRequest(
            "agent join token is not configured (set TURAES_AGENT_JOIN_TOKEN)".into(),
        ));
    }
    let control = state.cfg.proxy.control_address.clone();
    if control.is_empty() {
        return Err(Error::BadRequest(
            "proxy.control_address is not set (control-plane address agents dial)".into(),
        ));
    }
    let key_pem = match &server.ssh_key_enc {
        Some(enc) => state.secrets.open(enc)?,
        None => {
            return Err(Error::BadRequest(
                "server has no ssh key (register with --ssh-key-file)".into(),
            ))
        }
    };

    let plan = crate::bootstrap::BootstrapPlan {
        host: server
            .ssh_host
            .clone()
            .unwrap_or_else(|| server.address.clone()),
        port: server.ssh_port.unwrap_or(22) as u16,
        user: server.ssh_user.clone().unwrap_or_else(|| "root".into()),
        key_pem,
        name: server.name.clone(),
        address: server.address.clone(),
        control_url: format!("http://{}:{}", control, state.cfg.grpc.port),
        http_url: format!("http://{}:{}", control, state.cfg.server.port),
        token: state.cfg.agent.join_token.clone(),
    };

    let output = crate::bootstrap::run(&plan).await?;
    sqlx::query(
        "UPDATE servers SET status = 'online', last_seen_at = datetime('now') WHERE id = ?",
    )
    .bind(&server.id)
    .execute(&state.pool)
    .await?;
    Ok(output)
}

async fn fetch_server(state: &AppState, id: &str) -> Result<Server> {
    sqlx::query_as::<_, Server>("SELECT * FROM servers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("server {id}")))
}
