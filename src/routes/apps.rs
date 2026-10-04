//! Application CRUD, deploy, and per-app metric/visitor reads.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use sha2::{Digest, Sha256};

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
use crate::routes::quotas;
use crate::state::AppState;

/// Body for creating an application.
#[derive(Debug, Deserialize)]
pub struct CreateApp {
    /// Slug (lowercase letters, digits, dashes).
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Absolute path to the prebuilt binary (required unless `command` is set,
    /// in which case the server uses `command[0]`; ignored for `static`).
    pub binary_path: Option<String>,
    /// Optional arguments.
    pub args: Option<String>,
    /// Exec argv for interpreted apps (mutually exclusive with meaningful
    /// `args`; the binary runs `command`, not `binary_path + args`).
    pub command: Option<Vec<String>>,
    /// Working directory override.
    pub workdir: Option<String>,
    /// Source directory synced for `static` apps.
    pub publish_dir: Option<String>,
    /// `service` (default), `static`, or `worker`.
    pub kind: Option<String>,
    /// Loopback port (`service`/`static` require one; `worker` passes 0/omit).
    pub port: Option<u16>,
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
    /// Resident memory ceiling in MiB (systemd only).
    pub mem_limit_mb: Option<i64>,
    /// CPU ceiling in percent of one core (systemd only).
    pub cpu_quota_pct: Option<i64>,
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
    /// Resident memory ceiling in MiB (systemd only). Absent keeps, `null`
    /// clears, a number sets.
    pub mem_limit_mb: Option<Option<i64>>,
    /// CPU ceiling in percent of one core (systemd only). Same tri-state.
    pub cpu_quota_pct: Option<Option<i64>>,
    /// Exec argv for interpreted apps (absent keeps, `null` clears).
    pub command: Option<Option<Vec<String>>>,
    /// Working directory override (absent keeps, `null` clears).
    pub workdir: Option<Option<String>>,
    /// Source directory synced for `static` apps (absent keeps, `null` clears).
    pub publish_dir: Option<Option<String>>,
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
/// Validate per-app resource limits. The `proc` runtime cannot confine, so
/// limits with it are rejected instead of silently ignored.
pub(crate) fn validate_limits(
    mem_mb: Option<i64>,
    cpu_pct: Option<i64>,
    runtime: &str,
) -> Result<()> {
    if let Some(m) = mem_mb {
        if !(16..=65536).contains(&m) {
            return Err(Error::BadRequest(
                "mem_limit_mb must be between 16 and 65536".into(),
            ));
        }
    }
    if let Some(c) = cpu_pct {
        if !(1..=6400).contains(&c) {
            return Err(Error::BadRequest(
                "cpu_quota_pct must be between 1 and 6400".into(),
            ));
        }
    }
    if (mem_mb.is_some() || cpu_pct.is_some()) && runtime == "proc" {
        return Err(Error::BadRequest(
            "resource limits require the systemd runtime".into(),
        ));
    }
    Ok(())
}

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
pub(crate) async fn ensure_port_free(
    pool: &Pool,
    port: i64,
    offset: i64,
    except_id: &str,
) -> Result<()> {
    let paired = port + offset;
    let clash: Option<String> = sqlx::query_scalar(
        "SELECT name FROM applications \
         WHERE kind != 'worker' AND id != ? \
         AND (port = ? OR port = ? OR port + ? = ? OR port + ? = ?) \
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
        mem_limit_mb: app.mem_limit_mb.map(|m| m as u64),
        cpu_quota_pct: app.cpu_quota_pct.map(|c| c as u32),
        kind: app.kind.clone(),
        command: app
            .command
            .as_deref()
            .and_then(|c| serde_json::from_str(c).ok()),
        workdir: app.workdir.clone(),
        publish_dir: app.publish_dir.clone(),
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
    // Workers bind nothing and are never routed.
    let placements: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT p.application_id, s.address, COALESCE(a.active_port, p.port) AS port \
         FROM app_servers p \
         JOIN servers s ON s.id = p.server_id \
         JOIN applications a ON a.id = p.application_id \
         WHERE a.kind != 'worker'",
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
    Query(q): Query<super::LimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let apps = sqlx::query_as::<_, Application>(
        "SELECT * FROM applications WHERE org_id = ? ORDER BY created_at DESC LIMIT ?",
    )
    .bind(&org_id)
    .bind(super::LimitQuery::effective(q.limit))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "applications": apps })))
}

/// Validate kind/shape fields, mirroring `AppManifest::validate`. Returns the
/// effective (db_port, binary_path): workers bind nothing (port 0), command
/// apps execute argv[0] (which becomes their binary path), static apps sync
/// their publish dir.
pub(crate) async fn resolve_kind_shape(input: &CreateApp) -> Result<(String, i64, String)> {
    let kind = input.kind.clone().unwrap_or_else(|| "service".into());
    if !matches!(kind.as_str(), "service" | "static" | "worker") {
        return Err(Error::BadRequest(
            "kind must be 'service', 'static' or 'worker'".into(),
        ));
    }
    if input.args.is_some() && input.command.is_some() {
        return Err(Error::BadRequest(
            "'args' and 'command' are mutually exclusive (argv carries its own arguments)".into(),
        ));
    }
    if let Some(argv) = &input.command {
        if argv.is_empty() || argv.iter().any(|a| a.trim().is_empty()) {
            return Err(Error::BadRequest(
                "'command' must be a non-empty argv array".into(),
            ));
        }
        if argv[0].contains(' ') {
            return Err(Error::BadRequest(
                "command[0] must be a binary path without spaces (no shell)".into(),
            ));
        }
        if tokio::fs::metadata(&argv[0]).await.is_err() {
            return Err(Error::BadRequest(format!(
                "command binary '{}' does not exist or is not readable",
                argv[0]
            )));
        }
    }
    let binary_path = if let Some(argv) = &input.command {
        argv[0].clone()
    } else if kind == "static" {
        match input.publish_dir.as_deref() {
            Some(d) if !d.trim().is_empty() => d.to_string(),
            _ => {
                return Err(Error::BadRequest("kind: static needs 'publish_dir'".into()));
            }
        }
    } else {
        match input.binary_path.as_deref() {
            Some(b) if !b.trim().is_empty() => b.to_string(),
            _ => return Err(Error::BadRequest("binary_path is required".into())),
        }
    };
    if kind == "static" && (input.binary_path.is_some() || input.command.is_some()) {
        return Err(Error::BadRequest(
            "kind: static takes 'publish_dir', not 'binary_path'/'command'".into(),
        ));
    }
    if kind != "static" && input.publish_dir.is_some() {
        return Err(Error::BadRequest(
            "'publish_dir' is only valid for kind: static".into(),
        ));
    }
    let port = match kind.as_str() {
        "worker" => match input.port {
            None | Some(0) => 0,
            Some(_) => {
                return Err(Error::BadRequest(
                    "kind: worker takes no port (pass 0 or omit it)".into(),
                ));
            }
        },
        _ => match input.port {
            Some(p) if p != 0 => p as i64,
            _ => {
                return Err(Error::BadRequest(
                    "kind: service/static needs 'port'".into(),
                ))
            }
        },
    };
    if kind == "worker" {
        if input.domain.as_ref().is_some_and(|d| !d.is_empty()) {
            return Err(Error::BadRequest(
                "kind: worker takes no 'domain' (nothing is routed)".into(),
            ));
        }
        if input.metrics_path.is_some() {
            return Err(Error::BadRequest(
                "kind: worker takes no 'metrics_path' (nothing is scraped)".into(),
            ));
        }
    }
    Ok((kind, port, binary_path))
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
    let (kind, port, binary_path) = resolve_kind_shape(&input).await?;
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
    if port != 0 {
        ensure_port_free(&state.pool, port, state.cfg.runtime.slot_offset as i64, "").await?;
    }
    validate_limits(input.mem_limit_mb, input.cpu_quota_pct, &runtime)?;
    quotas::ensure_capacity(
        &state.pool,
        &org_id,
        "",
        input.mem_limit_mb,
        input.cpu_quota_pct,
    )
    .await?;
    if input.domain.as_ref().is_some_and(|d| !d.is_empty()) {
        quotas::ensure_domain_capacity(&state.pool, &org_id).await?;
    }
    let command_json = input
        .command
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| Error::BadRequest(format!("invalid command argv: {e}")))?;
    let inserted = sqlx::query_as::<_, Application>(
        "INSERT INTO applications \
         (id, org_id, name, description, binary_path, args, port, health_path, metrics_path, domain, \
          server_id, runtime, auto_restart, mem_limit_mb, cpu_quota_pct, kind, command, workdir, \
          publish_dir, status, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'stopped', datetime('now'), datetime('now')) \
         RETURNING *",
    )
    .bind(&id)
    .bind(&org_id)
    .bind(&input.name)
    .bind(&input.description)
    .bind(&binary_path)
    .bind(&input.args)
    .bind(port)
    .bind(input.health_path.unwrap_or_else(|| "/health".into()))
    .bind(match kind.as_str() {
        "worker" => None,
        _ => input.metrics_path.or_else(|| Some("/metrics".into())),
    })
    .bind(&input.domain)
    .bind(&server_id)
    .bind(&runtime)
    .bind(input.auto_restart.unwrap_or(true) as i64)
    .bind(input.mem_limit_mb)
    .bind(input.cpu_quota_pct)
    .bind(&kind)
    .bind(&command_json)
    .bind(&input.workdir)
    .bind(&input.publish_dir)
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
/// Identity hash for command apps: `cmd:<sha256(argv JSON)>`. The code lives
/// wherever argv[0] resolves (venv, host interpreter); the hash names the
/// config generation, not bytes, so rollback means a fresh restart cutover.
pub(crate) fn command_identity_hash(app: &Application) -> Result<String> {
    let argv = app
        .command
        .as_deref()
        .ok_or_else(|| Error::Internal("command app is missing its argv".into()))?;
    let digest = Sha256::digest(argv.as_bytes());
    let mut hex = String::with_capacity(64);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok(format!("cmd:{hex}"))
}

/// What gets installed: a stored file, or an argv identity (command apps
/// execute in place — argv[0] may be a bare `PATH` name, never stored).
enum Artifact {
    File(String),
    Command(String),
}

pub async fn deploy_app(state: &AppState, app: &Application) -> Result<(String, DeployOutcome)> {
    // Static apps sync directories, not files; remote agents only speak files.
    if app.kind == "static" {
        if app.server_id != "local" {
            return Err(Error::BadRequest(format!(
                "remote deploy of static app '{}' is not implemented yet (agent ships files, not trees)",
                app.name
            )));
        }
        return deploy_static(state, app).await;
    }

    // Command apps execute in place; remote agents only speak files.
    if app.command.is_some() {
        let hash = command_identity_hash(app)?;
        if app.server_id == "local" {
            return deploy_artifact(state, app, Artifact::Command(hash)).await;
        }
        return Err(Error::BadRequest(format!(
            "remote deploy of command app '{}' is not implemented yet (agent transport carries files, not argv)",
            app.name
        )));
    }

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

/// Pick the inactive blue/green slot for the next deploy.
/// Returns (target slot, target port, previous spec, previous is legacy).
async fn pick_inactive_slot(
    state: &AppState,
    app: &Application,
    runtime: &Arc<dyn Runtime>,
) -> (Slot, i64, Option<AppSpec>, bool) {
    let offset = state.cfg.runtime.slot_offset as i64;
    let port_a = app.port;
    let port_b = app.port + offset;
    match app.active_port {
        None => {
            // Migrate a running legacy (unslotted) unit to slot B without a
            // port clash; a never-deployed app starts on slot A.
            let legacy = spec_for(&state.cfg, app);
            let running = matches!(runtime.status(&legacy).await, Ok(RunState::Running));
            if running {
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
    }
}

/// Hash a publish directory (relative paths + bytes) for static deploy history.
/// Format: `dir:<hex>`. The files themselves live in the per-slot public
/// directories; the hash is the identity, not a retrieval key.
async fn dir_content_hash(publish_dir: &str) -> Result<String> {
    let root = std::path::Path::new(publish_dir);
    if !root.is_dir() {
        return Err(Error::BadRequest(format!(
            "publish_dir '{publish_dir}' is not a directory"
        )));
    }
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| Error::Internal(format!("failed to read {}: {e}", dir.display())))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::Internal(format!("failed to list {}: {e}", dir.display())))?
        {
            let path = entry.path();
            let ftype = entry
                .file_type()
                .await
                .map_err(|e| Error::Internal(format!("failed to stat {}: {e}", path.display())))?;
            if ftype.is_dir() {
                stack.push(path);
            } else if ftype.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut hasher = Sha256::new();
    let mut empty = true;
    for path in files {
        empty = false;
        let rel = path.strip_prefix(root).unwrap_or(&path);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| Error::Internal(format!("failed to read {}: {e}", path.display())))?;
        hasher.update(&bytes);
    }
    if empty {
        tracing::warn!(dir = publish_dir, "deploying an empty publish directory");
    }
    let mut hex = String::with_capacity(64);
    for b in hasher.finalize() {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok(format!("dir:{hex}"))
}

/// Finish a successful slot deploy: cut `active_port` over, record history,
/// refresh the proxy, then drain the previous slot.
#[allow(clippy::too_many_arguments)]
async fn cutover(
    state: &AppState,
    app: &Application,
    dep_id: &str,
    port: i64,
    artifact_hash: &str,
    log: &str,
    previous: Option<AppSpec>,
    prev_legacy: bool,
) -> Result<()> {
    let runtime = runtime_for(&state.cfg, &app.runtime);
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
    .bind(artifact_hash)
    .bind(log)
    .bind(dep_id)
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
    Ok(())
}

/// Deploy a `static` app: sync the publish dir into the inactive slot's public
/// directory, serve it, gate on the always-200 health endpoint, then cut over.
pub async fn deploy_static(state: &AppState, app: &Application) -> Result<(String, DeployOutcome)> {
    let publish_dir = app
        .publish_dir
        .as_deref()
        .ok_or_else(|| Error::BadRequest("static app is missing publish_dir".into()))?;
    let hash = dir_content_hash(publish_dir).await?;
    let env = load_env(state, &app.id).await?;
    let runtime = runtime_for(&state.cfg, &app.runtime);
    let (slot, port, previous, prev_legacy) = pick_inactive_slot(state, app, &runtime).await;
    let spec = spec_for_slot(&state.cfg, app, Some(slot), port as u16);
    let dep_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO deployments (id, application_id, status, started_at) \
         VALUES (?, ?, 'installing', datetime('now'))",
    )
    .bind(&dep_id)
    .bind(&app.id)
    .execute(&state.pool)
    .await?;

    let outcome = Deployer::new(runtime.clone())
        .deploy_static(&spec, &env)
        .await;
    let out = match outcome {
        Ok(out) => out,
        Err(e) => {
            let _ = runtime.stop(&spec).await;
            fail_deployment(state, app, &dep_id, &format!("start failed: {e}")).await?;
            return Err(e);
        }
    };

    // The file server always answers 200; the gate still proves it serves.
    if !health_gate(state, app, port).await {
        let _ = runtime.stop(&spec).await;
        fail_deployment(state, app, &dep_id, "new slot failed health check").await?;
        return Err(Error::Internal(
            "deploy failed health check; previous slot kept".into(),
        ));
    }

    let log = format!("static {hash}\nsynced {publish_dir}\n{}", out.log);
    cutover(
        state,
        app,
        &dep_id,
        port,
        &hash,
        &log,
        previous,
        prev_legacy,
    )
    .await?;
    Ok((
        dep_id,
        DeployOutcome {
            artifact_hash: hash,
            state: RunState::Running,
            log,
        },
    ))
}

/// Roll back a `static` app by cutting back to the slot whose public
/// directory still exists (the generic artifact store holds no file trees).
pub async fn rollback_static(
    state: &AppState,
    app: &Application,
) -> Result<(String, DeployOutcome)> {
    let offset = state.cfg.runtime.slot_offset as i64;
    let other = match app.active_port {
        Some(ap) if ap == app.port + offset => (Slot::A, app.port),
        Some(_) => (Slot::B, app.port + offset),
        None => {
            return Err(Error::BadRequest(
                "static app was never deployed; nothing to roll back to".into(),
            ));
        }
    };
    let spec = spec_for_slot(&state.cfg, app, Some(other.0), other.1 as u16);
    if tokio::fs::metadata(spec.public_dir()).await.is_err() {
        return Err(Error::BadRequest(
            "no previous static slot survives to roll back to".into(),
        ));
    }
    let runtime = runtime_for(&state.cfg, &app.runtime);
    runtime.start(&spec).await?;
    if !health_gate(state, app, other.1).await {
        let _ = runtime.stop(&spec).await;
        return Err(Error::Internal(
            "previous static slot failed health check".into(),
        ));
    }
    let dep_id = uuid::Uuid::new_v4().to_string();
    let hash = dir_content_hash(app.publish_dir.as_deref().unwrap_or_default())
        .await
        .unwrap_or_else(|_| "dir:unknown".into());
    sqlx::query(
        "INSERT INTO deployments (id, application_id, status, artifact_hash, log, started_at, finished_at) \
         VALUES (?, ?, 'rolled_back', ?, 'cut back to previous static slot', datetime('now'), datetime('now'))",
    )
    .bind(&dep_id)
    .bind(&app.id)
    .bind(&hash)
    .execute(&state.pool)
    .await?;
    cutover_no_drain(state, app, other.1).await?;
    Ok((
        dep_id,
        DeployOutcome {
            artifact_hash: hash,
            state: RunState::Running,
            log: "cut back to previous static slot".into(),
        },
    ))
}

/// Cut `active_port` over without draining (rollback already runs the target).
async fn cutover_no_drain(state: &AppState, app: &Application, port: i64) -> Result<()> {
    sqlx::query(
        "UPDATE applications SET active_port = ?, status = 'running', updated_at = datetime('now') \
         WHERE id = ?",
    )
    .bind(port)
    .bind(&app.id)
    .execute(&state.pool)
    .await?;
    let _ = refresh_proxy_routes(state).await;
    Ok(())
}

/// Deploy an app from an explicit on-disk artifact `source`, using blue/green
/// slots: start the inactive slot, health-gate it, then cut the route over and
/// drain the previous slot. The previous slot is left installed for rollback.
///
/// Workers skip the health gate (nothing listens): a clean start is success.
pub async fn deploy_app_source(
    state: &AppState,
    app: &Application,
    source: String,
) -> Result<(String, DeployOutcome)> {
    deploy_artifact(state, app, Artifact::File(source)).await
}

/// Shared slot-deploy body for stored files and argv identities.
async fn deploy_artifact(
    state: &AppState,
    app: &Application,
    artifact: Artifact,
) -> Result<(String, DeployOutcome)> {
    let env = load_env(state, &app.id).await?;

    // Target the inactive slot; remember the previous slot to drain on success.
    let runtime = runtime_for(&state.cfg, &app.runtime);

    // (target slot, target port, previous spec, previous is legacy-unslotted)
    let (slot, port, previous, prev_legacy) = pick_inactive_slot(state, app, &runtime).await;

    let mut spec = spec_for_slot(&state.cfg, app, Some(slot), port as u16);
    // Files deploy from the stored copy; argv identities keep executing the
    // command in place (runtimes skip the install for `command` specs).
    let precomputed_hash = match artifact {
        Artifact::File(source) => {
            spec.binary_path = source;
            None
        }
        Artifact::Command(hash) => Some(hash),
    };
    let dep_id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO deployments (id, application_id, status, started_at) \
         VALUES (?, ?, 'installing', datetime('now'))",
    )
    .bind(&dep_id)
    .bind(&app.id)
    .execute(&state.pool)
    .await?;

    let deployer = Deployer::new(runtime.clone());
    let outcome = match precomputed_hash {
        Some(hash) => deployer.deploy_with_hash(&spec, &env, hash).await,
        None => deployer.deploy(&spec, &env).await,
    };
    let out = match outcome {
        Ok(out) => out,
        Err(e) => {
            let _ = runtime.stop(&spec).await;
            fail_deployment(state, app, &dep_id, &format!("start failed: {e}")).await?;
            return Err(e);
        }
    };

    // Health-gate the new slot before cutting over (zero-downtime only if it
    // is up). Workers bind nothing: a clean start is success.
    if app.kind != "worker" && !health_gate(state, app, port).await {
        let _ = runtime.stop(&spec).await;
        fail_deployment(state, app, &dep_id, "new slot failed health check").await?;
        return Err(Error::Internal(
            "deploy failed health check; previous slot kept".into(),
        ));
    }

    cutover(
        state,
        app,
        &dep_id,
        port,
        &out.artifact_hash,
        &out.log,
        previous,
        prev_legacy,
    )
    .await?;

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
    // Static trees live in the slot directories, not the artifact store.
    if app.kind == "static" {
        let (dep_id, out) = rollback_static(&state, &app).await?;
        audit::record(
            &state,
            Some(&org_id),
            Some(&user),
            Some(&app.id),
            "app.rollback",
            Some("deployment"),
            Some(&dep_id),
            Some(&serde_json::json!({"rolled_back_to": out.artifact_hash}).to_string()),
        )
        .await?;
        return Ok(Json(serde_json::json!({
            "deployment_id": dep_id,
            "rolled_back_to": out.artifact_hash,
            "state": out.state,
            "artifact_hash": out.artifact_hash,
            "log": out.log,
        })));
    }
    // Command apps version config, not bytes: rolling back means a fresh
    // restart cutover of the current argv (the store holds no command blob).
    if app.command.is_some() {
        let (dep_id, out) = deploy_app(&state, &app).await?;
        audit::record(
            &state,
            Some(&org_id),
            Some(&user),
            Some(&app.id),
            "app.rollback",
            Some("deployment"),
            Some(&dep_id),
            Some(&serde_json::json!({"rolled_back_to": out.artifact_hash}).to_string()),
        )
        .await?;
        return Ok(Json(serde_json::json!({
            "deployment_id": dep_id,
            "rolled_back_to": out.artifact_hash,
            "state": out.state,
            "artifact_hash": out.artifact_hash,
            "log": out.log,
        })));
    }
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
            select_previous_artifact(&hashes).ok_or_else(|| {
                Error::BadRequest(
                    "no previous build to roll back to (deploy at least twice first)".into(),
                )
            })?
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
    if app.kind == "worker" && port != 0 {
        return Err(Error::BadRequest(
            "kind: worker takes no port (pass 0 or omit it)".into(),
        ));
    }
    if port != 0 && port != app.port {
        ensure_port_free(&state.pool, port, state.cfg.runtime.slot_offset as i64, &id).await?;
    }
    let mem_limit_mb = input.mem_limit_mb.unwrap_or(app.mem_limit_mb);
    let cpu_quota_pct = input.cpu_quota_pct.unwrap_or(app.cpu_quota_pct);
    validate_limits(mem_limit_mb, cpu_quota_pct, &runtime)?;
    quotas::ensure_capacity(&state.pool, &org_id, &id, mem_limit_mb, cpu_quota_pct).await?;
    let domain = input.domain.or(app.domain.clone());
    if domain.as_ref().is_some_and(|d| !d.is_empty()) && domain != app.domain {
        quotas::ensure_domain_capacity(&state.pool, &org_id).await?;
    }
    // Command/workdir/publish changes take effect on the next deploy. A new
    // command re-points the executable (binary_path tracks argv[0]).
    let command: Option<Vec<String>> = match &input.command {
        Some(None) => None,
        Some(Some(argv)) => {
            if argv.is_empty() || argv.iter().any(|a| a.trim().is_empty()) {
                return Err(Error::BadRequest(
                    "'command' must be a non-empty argv array".into(),
                ));
            }
            if argv[0].contains(' ') {
                return Err(Error::BadRequest(
                    "command[0] must be a binary path without spaces (no shell)".into(),
                ));
            }
            if input.args.is_some() {
                return Err(Error::BadRequest(
                    "'args' and 'command' are mutually exclusive (argv carries its own arguments)"
                        .into(),
                ));
            }
            Some(argv.clone())
        }
        None => app
            .command
            .as_deref()
            .and_then(|c| serde_json::from_str(c).ok()),
    };
    let workdir = input.workdir.unwrap_or(app.workdir);
    let publish_dir = input.publish_dir.unwrap_or(app.publish_dir);
    // Setting a command supersedes flat args (argv carries its own).
    let args = if matches!(&input.command, Some(Some(_))) {
        None
    } else {
        input.args.or(app.args)
    };
    let binary_path = match &command {
        Some(argv) => argv[0].clone(),
        None => app.binary_path.clone(),
    };
    let updated = sqlx::query_as::<_, Application>(
        "UPDATE applications SET description = ?, args = ?, port = ?, health_path = ?, \
         metrics_path = ?, domain = ?, runtime = ?, auto_restart = ?, server_id = ?, \
         mem_limit_mb = ?, cpu_quota_pct = ?, binary_path = ?, command = ?, workdir = ?, \
         publish_dir = ?, \
         updated_at = datetime('now') WHERE id = ? RETURNING *",
    )
    .bind(input.description.or(app.description))
    .bind(args)
    .bind(port)
    .bind(input.health_path.unwrap_or(app.health_path))
    .bind(input.metrics_path.or(app.metrics_path))
    .bind(domain)
    .bind(&runtime)
    .bind(input.auto_restart.unwrap_or(app.auto_restart) as i64)
    .bind(&server_id)
    .bind(mem_limit_mb)
    .bind(cpu_quota_pct)
    .bind(&binary_path)
    .bind(
        command
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| Error::BadRequest(format!("invalid command argv: {e}")))?,
    )
    .bind(workdir)
    .bind(publish_dir)
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
