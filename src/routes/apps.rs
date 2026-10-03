//! Application CRUD, deploy, and per-app metric/visitor reads.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use turaes_core::config::Config;
use turaes_core::db::Pool;
use turaes_core::models::Application;
use turaes_core::{Error, Result};
use turaes_proxy::{RouteTable, Upstream};
use turaes_runtime::proc::ProcRuntime;
use turaes_runtime::systemd::SystemdRuntime;
use turaes_runtime::{AppSpec, DeployOutcome, Deployer, RunState, Runtime};

use crate::state::AppState;

/// Body for creating an application.
#[derive(Debug, Deserialize)]
pub struct CreateApp {
    /// Slug (lowercase letters, digits, dashes).
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Absolute path to the prebuilt binary.
    pub binary_path: String,
    /// Optional arguments.
    pub args: Option<String>,
    /// Loopback port.
    pub port: u16,
    /// Health path (defaults to `/health`).
    pub health_path: Option<String>,
    /// Metrics path (defaults to `/metrics`).
    pub metrics_path: Option<String>,
    /// Primary hostname.
    pub domain: Option<String>,
    /// Node to place the app on (defaults to `local`).
    pub server_id: Option<String>,
    /// `systemd` (default) or `proc`.
    pub runtime: Option<String>,
    /// Restart on unhealthy (defaults to true).
    pub auto_restart: Option<bool>,
}

/// Query for historical stats.
#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    /// Look-back window in hours (default 1, max 720).
    pub hours: Option<i64>,
}

pub(crate) fn validate_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if valid {
        Ok(())
    } else {
        Err(Error::BadRequest(
            "name must be a lowercase slug of a-z, 0-9 and '-'".into(),
        ))
    }
}

pub(crate) async fn fetch_app(pool: &Pool, id: &str) -> Result<Application> {
    sqlx::query_as::<_, Application>("SELECT * FROM applications WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("application {id}")))
}

/// Assemble the runtime spec for an app from configuration.
pub fn spec_for(cfg: &Config, app: &Application) -> AppSpec {
    AppSpec {
        name: app.name.clone(),
        binary_path: app.binary_path.clone(),
        installed_path: format!("{}/{}", cfg.runtime.bin_dir, app.name),
        args: app.args.clone(),
        port: app.port as u16,
        state_dir: format!("{}/{}", cfg.runtime.state_dir, app.name),
        env_file: Some(format!("{}/{}.env", cfg.runtime.env_dir, app.name)),
        user: None,
    }
}

/// Rebuild and publish the proxy routing table from the database.
///
/// Routes each app's domain and the dashboard host to their loopback ports.
/// No-op when the proxy is disabled.
pub async fn refresh_proxy_routes(state: &AppState) -> Result<()> {
    let Some(router) = &state.proxy_router else {
        return Ok(());
    };
    // N0: only local apps are routed (remote upstreams land in N1).
    let apps = sqlx::query_as::<_, Application>(
        "SELECT * FROM applications \
         WHERE domain IS NOT NULL AND domain != '' AND server_id = 'local'",
    )
    .fetch_all(&state.pool)
    .await?;

    let mut routes: HashMap<String, Upstream> = HashMap::new();
    for app in apps {
        if let Some(domain) = app.domain {
            routes.insert(
                domain.to_lowercase(),
                Upstream {
                    host: "127.0.0.1".into(),
                    port: app.port as u16,
                    tls: false,
                },
            );
        }
    }
    if let Some(host) = state.cfg.dashboard_host() {
        routes.insert(
            host.to_lowercase(),
            Upstream {
                host: "127.0.0.1".into(),
                port: state.cfg.server.port,
                tls: false,
            },
        );
    }
    router.publish(RouteTable::new(
        state.cfg.server.base_domain.clone(),
        routes,
    ));
    Ok(())
}

/// Select a supervisor backend by name.
pub fn runtime_for(cfg: &Config, driver: &str) -> Arc<dyn Runtime> {
    match driver {
        "proc" => Arc::new(ProcRuntime::new(&cfg.runtime.state_dir)),
        _ => Arc::new(SystemdRuntime::new(
            &cfg.runtime.unit_dir,
            &cfg.runtime.bin_dir,
        )),
    }
}

pub(crate) async fn load_env(state: &AppState, app_id: &str) -> Result<BTreeMap<String, String>> {
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

/// `GET /api/v1/apps`
pub async fn list(State(state): State<AppState>) -> Result<Json<serde_json::Value>> {
    let apps =
        sqlx::query_as::<_, Application>("SELECT * FROM applications ORDER BY created_at DESC")
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(serde_json::json!({ "applications": apps })))
}

/// `POST /api/v1/apps`
pub async fn create(
    State(state): State<AppState>,
    Json(input): Json<CreateApp>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    validate_name(&input.name)?;
    if input.binary_path.trim().is_empty() {
        return Err(Error::BadRequest("binary_path is required".into()));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let runtime = input
        .runtime
        .clone()
        .unwrap_or_else(|| state.cfg.runtime.driver.clone());
    if !matches!(runtime.as_str(), "systemd" | "proc") {
        return Err(Error::BadRequest(
            "runtime must be 'systemd' or 'proc'".into(),
        ));
    }
    let server_id = input.server_id.clone().unwrap_or_else(|| "local".into());
    let server_exists: i64 = sqlx::query_scalar("SELECT count(*) FROM servers WHERE id = ?")
        .bind(&server_id)
        .fetch_one(&state.pool)
        .await?;
    if server_exists == 0 {
        return Err(Error::BadRequest(format!("unknown server '{server_id}'")));
    }
    let inserted = sqlx::query_as::<_, Application>(
        "INSERT INTO applications \
         (id, name, description, binary_path, args, port, health_path, metrics_path, domain, \
          server_id, runtime, auto_restart, status, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'stopped', datetime('now'), datetime('now')) \
         RETURNING *",
    )
    .bind(&id)
    .bind(&input.name)
    .bind(&input.description)
    .bind(&input.binary_path)
    .bind(&input.args)
    .bind(input.port as i64)
    .bind(input.health_path.unwrap_or_else(|| "/health".into()))
    .bind(input.metrics_path.or_else(|| Some("/metrics".into())))
    .bind(&input.domain)
    .bind(&server_id)
    .bind(&runtime)
    .bind(input.auto_restart.unwrap_or(true) as i64)
    .fetch_one(&state.pool)
    .await
    .map_err(map_unique_name)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "application": inserted })),
    ))
}

fn map_unique_name(e: sqlx::Error) -> Error {
    if let sqlx::Error::Database(db) = &e {
        if db.message().contains("UNIQUE") {
            return Error::Conflict("an application with that name already exists".into());
        }
    }
    Error::Db(e)
}

/// `GET /api/v1/apps/{id}`
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let app = fetch_app(&state.pool, &id).await?;
    Ok(Json(serde_json::json!({ "application": app })))
}

/// `DELETE /api/v1/apps/{id}`
pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    let app = fetch_app(&state.pool, &id).await?;
    let spec = spec_for(&state.cfg, &app);
    let _ = runtime_for(&state.cfg, &app.runtime).remove(&spec).await;
    sqlx::query("DELETE FROM applications WHERE id = ?")
        .bind(&id)
        .execute(&state.pool)
        .await?;
    let _ = refresh_proxy_routes(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/apps/{id}/deploy`
pub async fn deploy(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let app = fetch_app(&state.pool, &id).await?;
    let (dep_id, out) = deploy_app(&state, &app).await?;
    Ok(Json(serde_json::json!({
        "deployment_id": dep_id,
        "state": out.state,
        "artifact_hash": out.artifact_hash,
        "log": out.log,
    })))
}

/// Install + restart an app, persisting the deployment record.
///
/// Stores the current binary in the artifact store first, then deploys from the
/// stored (content-addressed) copy — so rollback and remote agents can reuse it.
/// Shared by the HTTP handler and the `turaes app deploy` CLI command.
pub async fn deploy_app(state: &AppState, app: &Application) -> Result<(String, DeployOutcome)> {
    let hash = state
        .artifacts
        .put_file(std::path::Path::new(&app.binary_path))
        .await?;

    if app.server_id == "local" {
        let source = state
            .artifacts
            .path_for(&hash)?
            .to_string_lossy()
            .to_string();
        return deploy_app_source(state, app, source).await;
    }

    // Remote: queue the artifact for the node's agent to reconcile.
    let dep_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO deployments (id, application_id, status, artifact_hash, started_at) \
         VALUES (?, ?, 'queued', ?, datetime('now'))",
    )
    .bind(&dep_id)
    .bind(&app.id)
    .bind(&hash)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE applications SET status = 'deploying', updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&app.id)
    .execute(&state.pool)
    .await?;
    tracing::info!(app = %app.name, server = %app.server_id, hash = %hash, "deploy queued for agent");
    Ok((
        dep_id,
        DeployOutcome {
            artifact_hash: hash,
            state: RunState::Unknown,
            log: "queued for agent".into(),
        },
    ))
}

/// Deploy an app from an explicit on-disk artifact `source`.
pub async fn deploy_app_source(
    state: &AppState,
    app: &Application,
    source: String,
) -> Result<(String, DeployOutcome)> {
    let env = load_env(state, &app.id).await?;
    let mut spec = spec_for(&state.cfg, app);
    spec.binary_path = source;
    let dep_id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO deployments (id, application_id, status, started_at) \
         VALUES (?, ?, 'installing', datetime('now'))",
    )
    .bind(&dep_id)
    .bind(&app.id)
    .execute(&state.pool)
    .await?;

    let outcome = Deployer::new(runtime_for(&state.cfg, &app.runtime))
        .deploy(&spec, &env)
        .await;

    match outcome {
        Ok(out) => {
            let status = out.state.as_status();
            sqlx::query(
                "UPDATE applications SET status = ?, updated_at = datetime('now') WHERE id = ?",
            )
            .bind(status)
            .bind(&app.id)
            .execute(&state.pool)
            .await?;
            sqlx::query(
                "UPDATE deployments SET status = ?, artifact_hash = ?, log = ?, \
                 finished_at = datetime('now') WHERE id = ?",
            )
            .bind(status)
            .bind(&out.artifact_hash)
            .bind(&out.log)
            .bind(&dep_id)
            .execute(&state.pool)
            .await?;
            let _ = refresh_proxy_routes(state).await;
            Ok((dep_id, out))
        }
        Err(e) => {
            sqlx::query(
                "UPDATE deployments SET status = 'failed', log = ?, finished_at = datetime('now') WHERE id = ?",
            )
            .bind(e.to_string())
            .bind(&dep_id)
            .execute(&state.pool)
            .await?;
            sqlx::query(
                "UPDATE applications SET status = 'failed', updated_at = datetime('now') WHERE id = ?",
            )
            .bind(&app.id)
            .execute(&state.pool)
            .await?;
            Err(e)
        }
    }
}

fn ensure_local(app: &Application) -> Result<()> {
    if app.server_id != "local" {
        return Err(Error::BadRequest(format!(
            "remote deploy to server '{}' is not implemented yet (N1: agent transport)",
            app.server_id
        )));
    }
    Ok(())
}

/// Pick the most recent deployment hash that differs from the current one.
pub(crate) fn select_previous_artifact(hashes: &[String]) -> Option<String> {
    let current = hashes.first()?;
    hashes.iter().find(|h| *h != current).cloned()
}

/// `POST /api/v1/apps/{id}/rollback` — redeploy the previous artifact.
pub async fn rollback(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let app = fetch_app(&state.pool, &id).await?;
    ensure_local(&app)?;
    let hashes: Vec<String> = sqlx::query_scalar(
        "SELECT artifact_hash FROM deployments \
         WHERE application_id = ? AND artifact_hash IS NOT NULL ORDER BY rowid DESC",
    )
    .bind(&app.id)
    .fetch_all(&state.pool)
    .await?;
    let previous = select_previous_artifact(&hashes)
        .ok_or_else(|| Error::BadRequest("no previous artifact to roll back to".into()))?;
    if !state.artifacts.has(&previous) {
        return Err(Error::NotFound(format!(
            "artifact {previous} is no longer in the store"
        )));
    }
    let source = state
        .artifacts
        .path_for(&previous)?
        .to_string_lossy()
        .to_string();
    let (dep_id, out) = deploy_app_source(&state, &app, source).await?;
    Ok(Json(serde_json::json!({
        "deployment_id": dep_id,
        "rolled_back_to": previous,
        "state": out.state,
        "artifact_hash": out.artifact_hash,
        "log": out.log,
    })))
}

/// `GET /api/v1/apps/{id}/stats`
pub async fn stats(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<serde_json::Value>> {
    fetch_app(&state.pool, &id).await?;
    let hours = q.hours.unwrap_or(1).clamp(1, 720);
    let rows: Vec<turaes_core::models::AppMetric> = sqlx::query_as(
        "SELECT * FROM app_metrics \
         WHERE application_id = ? AND recorded_at >= datetime('now', ?) \
         ORDER BY recorded_at ASC",
    )
    .bind(&id)
    .bind(format!("-{hours} hours"))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "metrics": rows })))
}

/// `GET /api/v1/apps/{id}/visitors`
pub async fn visitors(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<serde_json::Value>> {
    fetch_app(&state.pool, &id).await?;
    let hours = q.hours.unwrap_or(24).clamp(1, 720);
    let rows: Vec<turaes_core::models::VisitMetric> = sqlx::query_as(
        "SELECT * FROM visit_metrics \
         WHERE application_id = ? AND recorded_at >= datetime('now', ?) \
         ORDER BY recorded_at ASC",
    )
    .bind(&id)
    .bind(format!("-{hours} hours"))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "visitors": rows })))
}

#[cfg(test)]
mod tests {
    use super::select_previous_artifact;

    #[test]
    fn picks_previous_distinct_artifact() {
        let hs = vec!["sha256:aa".into(), "sha256:aa".into(), "sha256:bb".into()];
        assert_eq!(select_previous_artifact(&hs).as_deref(), Some("sha256:bb"));
        assert_eq!(select_previous_artifact(&["sha256:aa".into()]), None);
        assert_eq!(select_previous_artifact(&[]), None);
    }
}
