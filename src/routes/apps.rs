//! Application CRUD, deploy, and per-app metric/visitor reads.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;

use turaes_core::config::Config;
use turaes_core::db::Pool;
use turaes_core::models::{Application, Deployment};
use turaes_core::{Error, Result};
use turaes_monitor::health;
use turaes_proxy::{RouteTable, Upstream};
use turaes_runtime::proc::ProcRuntime;
use turaes_runtime::systemd::SystemdRuntime;
use turaes_runtime::{AppSpec, DeployOutcome, Deployer, RunState, Runtime, Slot};

use crate::audit;
use crate::authz::{self, CurrentUser, Role};
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

/// Query for deployment history.
#[derive(Debug, Deserialize)]
pub struct DeploymentsQuery {
    /// Max rows (default 20, max 200).
    pub limit: Option<i64>,
}

/// Body for editing an application (all fields optional).
#[derive(Debug, Deserialize)]
pub struct UpdateApp {
    /// Description.
    pub description: Option<String>,
    /// ExecStart arguments.
    pub args: Option<String>,
    /// Loopback port.
    pub port: Option<u16>,
    /// Health path.
    pub health_path: Option<String>,
    /// Metrics path.
    pub metrics_path: Option<String>,
    /// Primary hostname.
    pub domain: Option<String>,
    /// `systemd` or `proc`.
    pub runtime: Option<String>,
    /// Restart on unhealthy.
    pub auto_restart: Option<bool>,
    /// Node placement.
    pub server_id: Option<String>,
}

/// Body for rollback (optional explicit target).
#[derive(Debug, Deserialize, Default)]
pub struct RollbackBody {
    /// Artifact hash to roll back to (defaults to the previous build).
    pub artifact_hash: Option<String>,
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

/// Fetch an app scoped to an organization. Cross-org ids yield `NotFound` so
/// one tenant can never probe another tenant's applications.
pub(crate) async fn fetch_org_app(pool: &Pool, org_id: &str, id: &str) -> Result<Application> {
    sqlx::query_as::<_, Application>("SELECT * FROM applications WHERE id = ? AND org_id = ?")
        .bind(id)
        .bind(org_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("application {id}")))
}

/// Reject a loopback port (and its blue/green slot pair) already claimed by
/// another application. Ports are a host-global resource shared by all orgs.
async fn ensure_port_free(pool: &Pool, port: i64, offset: i64, except_id: &str) -> Result<()> {
    let paired = port + offset;
    let clash: Option<String> = sqlx::query_scalar(
        "SELECT name FROM applications \
         WHERE id != ? AND (port = ? OR port = ? OR port + ? = ? OR port + ? = ?) \
         LIMIT 1",
    )
    .bind(except_id)
    .bind(port)
    .bind(paired)
    .bind(offset)
    .bind(port)
    .bind(offset)
    .bind(paired)
    .fetch_optional(pool)
    .await?;
    if let Some(name) = clash {
        return Err(Error::Conflict(format!(
            "port {port} (or its blue/green pair) is already claimed by '{name}'"
        )));
    }
    Ok(())
}

/// Assemble the runtime spec for an app (unslotted).
pub fn spec_for(cfg: &Config, app: &Application) -> AppSpec {
    spec_for_slot(cfg, app, None, app.port as u16)
}

/// The spec for the currently-active slot (for lifecycle: stop/start/restart).
pub fn active_spec(cfg: &Config, app: &Application) -> AppSpec {
    let offset = cfg.runtime.slot_offset as i64;
    match app.active_port {
        Some(ap) if ap == app.port + offset => spec_for_slot(cfg, app, Some(Slot::B), ap as u16),
        Some(ap) => spec_for_slot(cfg, app, Some(Slot::A), ap as u16),
        None => spec_for(cfg, app),
    }
}

/// Assemble a slot-scoped runtime spec (blue/green).
pub fn spec_for_slot(cfg: &Config, app: &Application, slot: Option<Slot>, port: u16) -> AppSpec {
    let instance = match slot {
        Some(s) => format!("{}-{}", app.name, s.as_str()),
        None => app.name.clone(),
    };
    AppSpec {
        name: app.name.clone(),
        slot,
        binary_path: app.binary_path.clone(),
        installed_path: format!("{}/{}", cfg.runtime.bin_dir, app.name),
        args: app.args.clone(),
        port,
        state_dir: format!("{}/{}", cfg.runtime.state_dir, app.name),
        env_file: Some(format!("{}/{}.env", cfg.runtime.env_dir, instance)),
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

    // Placements (replicas) joined with their server address.
    // Route to the active blue/green slot port (falls back to the base port).
    let placements: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT p.application_id, s.address, COALESCE(a.active_port, p.port) AS port \
         FROM app_servers p \
         JOIN servers s ON s.id = p.server_id \
         JOIN applications a ON a.id = p.application_id",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut by_app: HashMap<String, Vec<Upstream>> = HashMap::new();
    for (app_id, address, port) in placements {
        by_app.entry(app_id).or_default().push(Upstream {
            host: address,
            port: port as u16,
            tls: false,
        });
    }

    let mut routes: HashMap<String, Vec<Upstream>> = HashMap::new();

    // Primary domain per app.
    let primaries: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, domain FROM applications WHERE domain IS NOT NULL AND domain != ''",
    )
    .fetch_all(&state.pool)
    .await?;
    for (app_id, domain) in primaries {
        if let Some(ups) = by_app.get(&app_id) {
            routes
                .entry(domain.to_lowercase())
                .or_default()
                .extend(ups.iter().cloned());
        }
    }

    // Domain aliases.
    let aliases: Vec<(String, String)> =
        sqlx::query_as("SELECT application_id, domain FROM domains")
            .fetch_all(&state.pool)
            .await?;
    for (app_id, domain) in aliases {
        if let Some(ups) = by_app.get(&app_id) {
            routes
                .entry(domain.to_lowercase())
                .or_default()
                .extend(ups.iter().cloned());
        }
    }

    if let Some(host) = state.cfg.dashboard_host() {
        routes
            .entry(host.to_lowercase())
            .or_default()
            .push(Upstream {
                host: "127.0.0.1".into(),
                port: state.cfg.server.port,
                tls: false,
            });
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

/// `GET /api/v1/orgs/{org}/apps`
pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let apps = sqlx::query_as::<_, Application>(
        "SELECT * FROM applications WHERE org_id = ? ORDER BY created_at DESC",
    )
    .bind(&org_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "applications": apps })))
}

/// `POST /api/v1/orgs/{org}/apps`
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Json(input): Json<CreateApp>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
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
    ensure_port_free(
        &state.pool,
        input.port as i64,
        state.cfg.runtime.slot_offset as i64,
        "",
    )
    .await?;
    let inserted = sqlx::query_as::<_, Application>(
        "INSERT INTO applications \
         (id, org_id, name, description, binary_path, args, port, health_path, metrics_path, domain, \
          server_id, runtime, auto_restart, status, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'stopped', datetime('now'), datetime('now')) \
         RETURNING *",
    )
    .bind(&id)
    .bind(&org_id)
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

    // Seed the app's first placement (replicas can be added later).
    sqlx::query(
        "INSERT OR IGNORE INTO app_servers (id, application_id, server_id, port) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&inserted.id)
    .bind(&inserted.server_id)
    .bind(inserted.port)
    .execute(&state.pool)
    .await?;

    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&inserted.id),
        "app.create",
        Some("application"),
        Some(&inserted.id),
        Some(&serde_json::json!({"name": inserted.name, "port": inserted.port}).to_string()),
    )
    .await?;

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

/// `GET /api/v1/orgs/{org}/apps/{id}`
pub async fn get(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let app = fetch_org_app(&state.pool, &org_id, &id).await?;
    Ok(Json(serde_json::json!({ "application": app })))
}

/// `DELETE /api/v1/orgs/{org}/apps/{id}`
pub async fn delete(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<StatusCode> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let app = fetch_org_app(&state.pool, &org_id, &id).await?;
    // Record before the row disappears (the FK then nulls the reference).
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "app.delete",
        Some("application"),
        Some(&app.id),
        Some(&serde_json::json!({"name": app.name}).to_string()),
    )
    .await?;
    let runtime = runtime_for(&state.cfg, &app.runtime);
    let offset = state.cfg.runtime.slot_offset as i64;
    let _ = runtime.remove(&spec_for(&state.cfg, &app)).await;
    for (slot, port) in [(Slot::A, app.port), (Slot::B, app.port + offset)] {
        let spec = spec_for_slot(&state.cfg, &app, Some(slot), port as u16);
        let _ = runtime.remove(&spec).await;
    }
    sqlx::query("DELETE FROM applications WHERE id = ?")
        .bind(&id)
        .execute(&state.pool)
        .await?;
    let _ = refresh_proxy_routes(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/orgs/{org}/apps/{id}/deploy`
pub async fn deploy(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Developer).await?;
    let app = fetch_org_app(&state.pool, &org_id, &id).await?;
    let (dep_id, out) = deploy_app(&state, &app).await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "app.deploy",
        Some("deployment"),
        Some(&dep_id),
        Some(&serde_json::json!({"artifact": out.artifact_hash}).to_string()),
    )
    .await?;
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

/// Deploy an app from an explicit on-disk artifact `source`, using blue/green
/// slots: start the inactive slot, health-gate it, then cut the route over and
/// drain the previous slot. The previous slot is left installed for rollback.
pub async fn deploy_app_source(
    state: &AppState,
    app: &Application,
    source: String,
) -> Result<(String, DeployOutcome)> {
    let env = load_env(state, &app.id).await?;
    let offset = state.cfg.runtime.slot_offset as i64;
    let port_a = app.port;
    let port_b = app.port + offset;

    // Target the inactive slot; remember the previous slot to drain on success.
    let runtime = runtime_for(&state.cfg, &app.runtime);

    // (target slot, target port, previous spec, previous is legacy-unslotted)
    let (slot, port, previous, prev_legacy): (Slot, i64, Option<AppSpec>, bool) =
        match app.active_port {
            None => {
                // Migrate a running legacy (unslotted) unit to slot B without a
                // port clash; a never-deployed app starts on slot A.
                let legacy = spec_for(&state.cfg, app);
                if matches!(runtime.status(&legacy).await, Ok(RunState::Running)) {
                    (Slot::B, port_b, Some(legacy), true)
                } else {
                    (Slot::A, port_a, None, false)
                }
            }
            Some(active) if active == port_b => (
                Slot::A,
                port_a,
                Some(spec_for_slot(&state.cfg, app, Some(Slot::B), port_b as u16)),
                false,
            ),
            Some(_) => (
                Slot::B,
                port_b,
                Some(spec_for_slot(&state.cfg, app, Some(Slot::A), port_a as u16)),
                false,
            ),
        };

    let mut spec = spec_for_slot(&state.cfg, app, Some(slot), port as u16);
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

    let outcome = Deployer::new(runtime.clone()).deploy(&spec, &env).await;
    let out = match outcome {
        Ok(out) => out,
        Err(e) => {
            let _ = runtime.stop(&spec).await;
            fail_deployment(state, app, &dep_id, &format!("start failed: {e}")).await?;
            return Err(e);
        }
    };

    // Health-gate the new slot before cutting over (zero-downtime only if it is up).
    if !health_gate(state, app, port).await {
        let _ = runtime.stop(&spec).await;
        fail_deployment(state, app, &dep_id, "new slot failed health check").await?;
        return Err(Error::Internal(
            "deploy failed health check; previous slot kept".into(),
        ));
    }

    sqlx::query(
        "UPDATE applications SET active_port = ?, status = 'running', updated_at = datetime('now') \
         WHERE id = ?",
    )
    .bind(port)
    .bind(&app.id)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE deployments SET status = 'running', artifact_hash = ?, log = ?, \
         finished_at = datetime('now') WHERE id = ?",
    )
    .bind(&out.artifact_hash)
    .bind(&out.log)
    .bind(&dep_id)
    .execute(&state.pool)
    .await?;
    let _ = refresh_proxy_routes(state).await;

    // Drain/replace the previous instance after a short window.
    if let Some(prev) = previous {
        tokio::time::sleep(std::time::Duration::from_secs(state.cfg.runtime.drain_secs)).await;
        if prev_legacy {
            // Legacy unslotted unit is superseded by the slots.
            let _ = runtime.remove(&prev).await;
        } else {
            let _ = runtime.stop(&prev).await;
        }
    }

    Ok((dep_id, out))
}

/// Poll the app's health endpoint on `port` until healthy (bounded).
async fn health_gate(state: &AppState, app: &Application, port: i64) -> bool {
    let url = format!("http://127.0.0.1:{}{}", port, app.health_path);
    for _ in 0..20 {
        if health::probe(&state.http, &url, std::time::Duration::from_secs(2))
            .await
            .is_healthy()
        {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    false
}

async fn fail_deployment(
    state: &AppState,
    app: &Application,
    dep_id: &str,
    reason: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE deployments SET status = 'failed', log = ?, finished_at = datetime('now') WHERE id = ?",
    )
    .bind(reason)
    .bind(dep_id)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE applications SET status = 'failed', updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&app.id)
    .execute(&state.pool)
    .await?;
    Ok(())
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

/// `POST /api/v1/orgs/{org}/apps/{id}/rollback` — redeploy a build (previous by default).
pub async fn rollback(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
    body: Option<Json<RollbackBody>>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Developer).await?;
    let app = fetch_org_app(&state.pool, &org_id, &id).await?;
    ensure_local(&app)?;
    let target = body.and_then(|Json(b)| b.artifact_hash);
    let previous = match target {
        Some(hash) => hash,
        None => {
            let hashes: Vec<String> = sqlx::query_scalar(
                "SELECT artifact_hash FROM deployments \
                 WHERE application_id = ? AND artifact_hash IS NOT NULL ORDER BY rowid DESC",
            )
            .bind(&app.id)
            .fetch_all(&state.pool)
            .await?;
            select_previous_artifact(&hashes)
                .ok_or_else(|| Error::BadRequest("no previous build to roll back to".into()))?
        }
    };
    if !state.artifacts.has(&previous) {
        return Err(Error::NotFound(format!(
            "build {previous} is no longer in the store"
        )));
    }
    let source = state
        .artifacts
        .path_for(&previous)?
        .to_string_lossy()
        .to_string();
    let (dep_id, out) = deploy_app_source(&state, &app, source).await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "app.rollback",
        Some("deployment"),
        Some(&dep_id),
        Some(&serde_json::json!({"rolled_back_to": previous}).to_string()),
    )
    .await?;
    Ok(Json(serde_json::json!({
        "deployment_id": dep_id,
        "rolled_back_to": previous,
        "state": out.state,
        "artifact_hash": out.artifact_hash,
        "log": out.log,
    })))
}

/// Run a lifecycle action (`stop`/`start`/`restart`) and persist the state.
async fn lifecycle(
    state: &AppState,
    app: &Application,
    action: &str,
) -> Result<Json<serde_json::Value>> {
    ensure_local(app)?;
    let spec = active_spec(&state.cfg, app);
    let runtime = runtime_for(&state.cfg, &app.runtime);
    match action {
        "stop" => runtime.stop(&spec).await?,
        "start" => runtime.start(&spec).await?,
        _ => runtime.restart(&spec).await?,
    }
    let st = runtime.status(&spec).await.unwrap_or(RunState::Unknown);
    sqlx::query("UPDATE applications SET status = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(st.as_status())
        .bind(&app.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({ "state": st })))
}

/// `POST /api/v1/orgs/{org}/apps/{id}/stop`
pub async fn stop(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Developer).await?;
    let app = fetch_org_app(&state.pool, &org_id, &id).await?;
    let out = lifecycle(&state, &app, "stop").await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "app.stop",
        Some("application"),
        Some(&app.id),
        None,
    )
    .await?;
    Ok(out)
}

/// `POST /api/v1/orgs/{org}/apps/{id}/start`
pub async fn start(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Developer).await?;
    let app = fetch_org_app(&state.pool, &org_id, &id).await?;
    let out = lifecycle(&state, &app, "start").await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "app.start",
        Some("application"),
        Some(&app.id),
        None,
    )
    .await?;
    Ok(out)
}

/// `POST /api/v1/orgs/{org}/apps/{id}/restart`
pub async fn restart(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Developer).await?;
    let app = fetch_org_app(&state.pool, &org_id, &id).await?;
    let out = lifecycle(&state, &app, "restart").await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "app.restart",
        Some("application"),
        Some(&app.id),
        None,
    )
    .await?;
    Ok(out)
}

/// `PATCH /api/v1/orgs/{org}/apps/{id}` — edit an application (and its placement).
pub async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
    Json(input): Json<UpdateApp>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let app = fetch_org_app(&state.pool, &org_id, &id).await?;

    let runtime = input.runtime.unwrap_or_else(|| app.runtime.clone());
    if !matches!(runtime.as_str(), "systemd" | "proc") {
        return Err(Error::BadRequest(
            "runtime must be 'systemd' or 'proc'".into(),
        ));
    }
    let server_id = input.server_id.unwrap_or_else(|| app.server_id.clone());
    let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM servers WHERE id = ?")
        .bind(&server_id)
        .fetch_one(&state.pool)
        .await?;
    if exists == 0 {
        return Err(Error::BadRequest(format!("unknown server '{server_id}'")));
    }

    let port = input.port.map(|p| p as i64).unwrap_or(app.port);
    if port != app.port {
        ensure_port_free(&state.pool, port, state.cfg.runtime.slot_offset as i64, &id).await?;
    }
    let updated = sqlx::query_as::<_, Application>(
        "UPDATE applications SET description = ?, args = ?, port = ?, health_path = ?, \
         metrics_path = ?, domain = ?, runtime = ?, auto_restart = ?, server_id = ?, \
         updated_at = datetime('now') WHERE id = ? RETURNING *",
    )
    .bind(input.description.or(app.description))
    .bind(input.args.or(app.args))
    .bind(port)
    .bind(input.health_path.unwrap_or(app.health_path))
    .bind(input.metrics_path.or(app.metrics_path))
    .bind(input.domain.or(app.domain))
    .bind(&runtime)
    .bind(input.auto_restart.unwrap_or(app.auto_restart) as i64)
    .bind(&server_id)
    .bind(&id)
    .fetch_one(&state.pool)
    .await?;

    // Keep the placement in sync (single-placement model).
    sqlx::query("UPDATE app_servers SET server_id = ?, port = ? WHERE application_id = ?")
        .bind(&server_id)
        .bind(port)
        .bind(&id)
        .execute(&state.pool)
        .await?;

    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "app.update",
        Some("application"),
        Some(&app.id),
        Some(&serde_json::json!({"port": port, "server_id": server_id}).to_string()),
    )
    .await?;

    let _ = refresh_proxy_routes(&state).await;
    Ok(Json(serde_json::json!({ "application": updated })))
}

/// `GET /api/v1/orgs/{org}/apps/{id}/stats`
pub async fn stats(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    fetch_org_app(&state.pool, &org_id, &id).await?;
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

/// `GET /api/v1/orgs/{org}/apps/{id}/visitors`
pub async fn visitors(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    fetch_org_app(&state.pool, &org_id, &id).await?;
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

/// `GET /api/v1/orgs/{org}/apps/{id}/deployments` — recent deployments (log truncated).
pub async fn deployments(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
    Query(q): Query<DeploymentsQuery>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    fetch_org_app(&state.pool, &org_id, &id).await?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let rows = sqlx::query_as::<_, Deployment>(
        "SELECT id, application_id, status, artifact_hash, substr(log, 1, 2000) AS log, \
         previous_artifact, started_at, finished_at \
         FROM deployments WHERE application_id = ? ORDER BY rowid DESC LIMIT ?",
    )
    .bind(&id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "deployments": rows })))
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
