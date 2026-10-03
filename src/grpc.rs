//! gRPC control plane (N1b): agent registration + heartbeat.
//!
//! Agents dial out to this server, exchange a join token for a server-bound
//! agent token, then heartbeat. Desired-state delivery (`Poll`/bidi) and the
//! separate edge role land in N1c/N3.

// tonic::Status is a large error type and the `Control` trait fixes the
// signature, so `result_large_err` cannot be avoided here.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;

use tonic::{Request, Response, Status};

use turaes_core::crypto::{random_token, token_hash};
use turaes_core::models::Server;

use crate::state::AppState;

/// Generated protobuf/tonic types (`turaes.v1`).
pub mod pb {
    #![allow(clippy::result_large_err)] // generated tonic stubs return large Status
    tonic::include_proto!("turaes.v1");
}

use pb::control_server::{Control, ControlServer};
use pb::{
    DesiredApp, HeartbeatRequest, HeartbeatResponse, PollRequest, PollResponse, RegisterRequest,
    RegisterResponse, ReportRequest, ReportResponse,
};

fn internal(e: impl std::fmt::Display) -> Status {
    Status::internal(e.to_string())
}

/// tonic service wrapping the register/heartbeat logic.
#[derive(Clone)]
pub struct ControlService {
    state: AppState,
}

impl ControlService {
    /// Build the service over shared state.
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl Control for ControlService {
    async fn register(
        &self,
        req: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        Ok(Response::new(
            register(&self.state, req.into_inner()).await?,
        ))
    }

    async fn heartbeat(
        &self,
        req: Request<HeartbeatRequest>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        Ok(Response::new(
            heartbeat(&self.state, req.into_inner()).await?,
        ))
    }

    async fn poll(&self, req: Request<PollRequest>) -> Result<Response<PollResponse>, Status> {
        Ok(Response::new(poll(&self.state, req.into_inner()).await?))
    }

    async fn report(
        &self,
        req: Request<ReportRequest>,
    ) -> Result<Response<ReportResponse>, Status> {
        Ok(Response::new(report(&self.state, req.into_inner()).await?))
    }
}

/// Resolve the server for an agent token, or fail unauthenticated.
async fn authenticate(state: &AppState, token: &str) -> Result<Server, Status> {
    if token.is_empty() {
        return Err(Status::unauthenticated("missing agent token"));
    }
    let hash = token_hash(token);
    sqlx::query_as::<_, Server>("SELECT * FROM servers WHERE agent_token_hash = ?")
        .bind(&hash)
        .fetch_optional(&state.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| Status::unauthenticated("unknown agent token"))
}

/// Desired apps for the calling agent's node.
pub async fn poll(state: &AppState, req: PollRequest) -> Result<PollResponse, Status> {
    use sqlx::Row;

    let server = authenticate(state, &req.agent_token).await?;
    let rows = sqlx::query(
        "SELECT a.id, a.name, a.args, a.port, a.runtime, a.health_path, a.metrics_path, \
         (SELECT artifact_hash FROM deployments d \
          WHERE d.application_id = a.id AND artifact_hash IS NOT NULL \
          ORDER BY rowid DESC LIMIT 1) AS artifact_hash \
         FROM applications a WHERE a.server_id = ?",
    )
    .bind(&server.id)
    .fetch_all(&state.pool)
    .await
    .map_err(internal)?;

    let mut apps = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.try_get("id").map_err(internal)?;
        let env = load_env(state, &id).await.map_err(internal)?;
        apps.push(DesiredApp {
            name: row.try_get("name").map_err(internal)?,
            artifact_hash: row
                .try_get::<Option<String>, _>("artifact_hash")
                .map_err(internal)?
                .unwrap_or_default(),
            args: row
                .try_get::<Option<String>, _>("args")
                .map_err(internal)?
                .unwrap_or_default(),
            port: row.try_get::<i64, _>("port").map_err(internal)? as u32,
            runtime: row.try_get("runtime").map_err(internal)?,
            health_path: row.try_get("health_path").map_err(internal)?,
            metrics_path: row
                .try_get::<Option<String>, _>("metrics_path")
                .map_err(internal)?
                .unwrap_or_default(),
            env: env.into_iter().collect(),
        });
    }
    Ok(PollResponse { apps })
}

async fn load_env(state: &AppState, app_id: &str) -> turaes_core::Result<BTreeMap<String, String>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value_enc FROM env_vars WHERE application_id = ?")
            .bind(app_id)
            .fetch_all(&state.pool)
            .await?;
    let mut env = BTreeMap::new();
    for (key, sealed) in rows {
        env.insert(key, state.secrets.open(&sealed)?);
    }
    Ok(env)
}

/// Record an apply result from an agent.
pub async fn report(state: &AppState, req: ReportRequest) -> Result<ReportResponse, Status> {
    let server = authenticate(state, &req.agent_token).await?;
    if req.app_name.is_empty() {
        return Err(Status::invalid_argument("app_name is required"));
    }
    let app: Option<String> =
        sqlx::query_scalar("SELECT id FROM applications WHERE name = ? AND server_id = ?")
            .bind(&req.app_name)
            .bind(&server.id)
            .fetch_optional(&state.pool)
            .await
            .map_err(internal)?;
    let Some(app_id) = app else {
        return Err(Status::not_found("no such app on this server"));
    };

    sqlx::query("UPDATE applications SET status = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&req.status)
        .bind(&app_id)
        .execute(&state.pool)
        .await
        .map_err(internal)?;

    sqlx::query(
        "UPDATE deployments SET status = ?, log = ?, finished_at = datetime('now') \
         WHERE id = (SELECT id FROM deployments WHERE application_id = ? ORDER BY rowid DESC LIMIT 1)",
    )
    .bind(&req.status)
    .bind(&req.message)
    .bind(&app_id)
    .execute(&state.pool)
    .await
    .map_err(internal)?;

    tracing::info!(server = %server.name, app = %req.app_name, status = %req.status, "agent reported");
    Ok(ReportResponse { ok: true })
}

/// Validate the join token and issue (or rebind) an agent token.
pub async fn register(state: &AppState, req: RegisterRequest) -> Result<RegisterResponse, Status> {
    let expected = &state.cfg.agent.join_token;
    if expected.is_empty() {
        return Err(Status::failed_precondition(
            "agent registration is disabled (no join token configured)",
        ));
    }
    if req.join_token != *expected {
        return Err(Status::unauthenticated("invalid join token"));
    }
    if req.name.trim().is_empty() {
        return Err(Status::invalid_argument("name is required"));
    }

    let token = random_token(32);
    let hash = token_hash(&token);
    let enc = state.secrets.seal(&token).map_err(internal)?;

    let existing: Option<Server> =
        sqlx::query_as::<_, Server>("SELECT * FROM servers WHERE name = ?")
            .bind(&req.name)
            .fetch_optional(&state.pool)
            .await
            .map_err(internal)?;

    let id = if let Some(server) = existing {
        if server.is_local {
            return Err(Status::invalid_argument(
                "cannot register an agent as the local server",
            ));
        }
        sqlx::query(
            "UPDATE servers SET address = ?, status = 'online', last_seen_at = datetime('now'), \
             agent_version = ?, agent_token_hash = ?, agent_token_enc = ? WHERE id = ?",
        )
        .bind(&req.address)
        .bind(&req.version)
        .bind(&hash)
        .bind(&enc)
        .bind(&server.id)
        .execute(&state.pool)
        .await
        .map_err(internal)?;
        server.id
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO servers \
             (id, name, address, is_local, status, last_seen_at, agent_version, \
              agent_token_hash, agent_token_enc) \
             VALUES (?, ?, ?, 0, 'online', datetime('now'), ?, ?, ?)",
        )
        .bind(&id)
        .bind(&req.name)
        .bind(&req.address)
        .bind(&req.version)
        .bind(&hash)
        .bind(&enc)
        .execute(&state.pool)
        .await
        .map_err(internal)?;
        id
    };

    tracing::info!(server = %req.name, address = %req.address, "agent registered");
    Ok(RegisterResponse {
        server_id: id,
        agent_token: token,
    })
}

/// Authenticate a heartbeat by hashed token and refresh last-seen.
pub async fn heartbeat(
    state: &AppState,
    req: HeartbeatRequest,
) -> Result<HeartbeatResponse, Status> {
    if req.agent_token.is_empty() {
        return Err(Status::unauthenticated("missing agent token"));
    }
    let hash = token_hash(&req.agent_token);
    let server: Option<Server> =
        sqlx::query_as::<_, Server>("SELECT * FROM servers WHERE agent_token_hash = ?")
            .bind(&hash)
            .fetch_optional(&state.pool)
            .await
            .map_err(internal)?;
    let Some(server) = server else {
        return Err(Status::unauthenticated("unknown agent token"));
    };
    sqlx::query(
        "UPDATE servers SET status = 'online', last_seen_at = datetime('now') WHERE id = ?",
    )
    .bind(&server.id)
    .execute(&state.pool)
    .await
    .map_err(internal)?;
    Ok(HeartbeatResponse {
        ok: true,
        server_id: server.id,
    })
}

/// Run the gRPC control server until the process exits.
pub async fn serve(state: AppState) -> turaes_core::Result<()> {
    let addr = format!("{}:{}", state.cfg.grpc.host, state.cfg.grpc.port);
    let socket = addr
        .parse()
        .map_err(|e| turaes_core::Error::Config(format!("invalid gRPC address {addr}: {e}")))?;
    let service = ControlService::new(state);
    tracing::info!(%addr, "gRPC control listening");
    tonic::transport::Server::builder()
        .add_service(ControlServer::new(service))
        .serve(socket)
        .await
        .map_err(|e| turaes_core::Error::Internal(format!("gRPC server: {e}")))
}
