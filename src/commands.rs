//! One-shot CLI operations that act directly on the local database.
//!
//! These exist so an operator can bootstrap and deploy apps on a fresh server
//! without configuring GitHub OAuth first. Conventions match the HTTP API.

use turaes_core::db::Pool;
use turaes_core::models::{Application, Server};
use turaes_core::{Error, Result};

use crate::audit;
use crate::cli::{AppCommand, ServerCommand};
use crate::routes::apps;
use crate::routes::quotas;
use crate::state::AppState;

/// Resolve an `--org` reference (id or slug) to an organization id.
async fn resolve_org(pool: &Pool, org: Option<&str>) -> Result<String> {
    let org_ref = org.unwrap_or("default");
    sqlx::query_scalar::<_, String>("SELECT id FROM organizations WHERE id = ? OR slug = ?")
        .bind(org_ref)
        .bind(org_ref)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| {
            Error::NotFound(format!(
                "unknown organization '{org_ref}' (create it on the Organization page)"
            ))
        })
}

/// Dispatch a `turaes app ...` subcommand.
pub async fn run(state: &AppState, cmd: AppCommand, org: Option<&str>, json: bool) -> Result<()> {
    let org_id = resolve_org(&state.pool, org).await?;
    match cmd {
        AppCommand::Add {
            name,
            binary,
            port,
            domain,
            health,
            metrics,
            runtime,
            args,
        } => {
            add(
                state,
                &org_id,
                &name,
                &binary,
                port,
                domain,
                &health,
                &metrics,
                &runtime,
                args.as_deref(),
            )
            .await
        }
        AppCommand::Deploy { name } => deploy(state, &org_id, &name).await,
        AppCommand::Rollback { name } => rollback(state, &org_id, &name).await,
        AppCommand::Remove { name } => remove(state, &org_id, &name).await,
        AppCommand::List => list(&state.pool, &org_id, json).await,
        AppCommand::Show { name } => show(state, &org_id, &name, json).await,
    }
}

/// Dispatch a `turaes server ...` subcommand (nodes are platform-global).
pub async fn run_server(state: &AppState, cmd: ServerCommand, json: bool) -> Result<()> {
    match cmd {
        ServerCommand::Add {
            name,
            address,
            ssh_host,
            ssh_port,
            ssh_user,
            ssh_key_file,
        } => {
            server_add(
                state,
                &name,
                &address,
                ssh_host,
                ssh_port,
                ssh_user,
                ssh_key_file,
            )
            .await
        }
        ServerCommand::List => server_list(&state.pool, json).await,
        ServerCommand::Remove { id } => server_remove(state, &id).await,
        ServerCommand::Bootstrap { id } => {
            let output = crate::routes::servers::run_bootstrap(state, &id).await?;
            println!("{output}");
            Ok(())
        }
    }
}

async fn server_add(
    state: &AppState,
    name: &str,
    address: &str,
    ssh_host: Option<String>,
    ssh_port: Option<i64>,
    ssh_user: Option<String>,
    ssh_key_file: Option<String>,
) -> Result<()> {
    if name.trim().is_empty() || address.trim().is_empty() {
        return Err(Error::BadRequest(
            "server --name and --address are required".into(),
        ));
    }
    let ssh_key_enc = match ssh_key_file {
        Some(path) => {
            let key = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| Error::BadRequest(format!("cannot read ssh key '{path}': {e}")))?;
            Some(state.secrets.seal(&key)?)
        }
        None => None,
    };
    let id = uuid::Uuid::new_v4().to_string();
    let server = sqlx::query_as::<_, Server>(
        "INSERT INTO servers \
         (id, name, address, ssh_host, ssh_port, ssh_user, ssh_key_enc, is_local, status) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 0, 'unknown') RETURNING *",
    )
    .bind(&id)
    .bind(name)
    .bind(address)
    .bind(&ssh_host)
    .bind(ssh_port)
    .bind(&ssh_user)
    .bind(&ssh_key_enc)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db) = &e {
            if db.message().contains("UNIQUE") {
                return Error::Conflict(format!("server '{name}' already exists"));
            }
        }
        Error::Db(e)
    })?;
    println!(
        "created server {} ({}) at {}",
        server.name, server.id, server.address
    );
    Ok(())
}

async fn server_list(pool: &Pool, json: bool) -> Result<()> {
    let servers =
        sqlx::query_as::<_, Server>("SELECT * FROM servers ORDER BY is_local DESC, name ASC")
            .fetch_all(pool)
            .await?;
    if json {
        println!("{}", serde_json::json!({ "servers": servers }));
        return Ok(());
    }
    println!("NAME             STATUS     ADDRESS                  LOCAL  ID");
    for s in servers {
        println!(
            "{:<16} {:<10} {:<24} {:<6} {}",
            s.name,
            s.status,
            s.address,
            if s.is_local { "yes" } else { "no" },
            s.id
        );
    }
    Ok(())
}

async fn server_remove(state: &AppState, id: &str) -> Result<()> {
    let server = sqlx::query_as::<_, Server>("SELECT * FROM servers WHERE id = ? OR name = ?")
        .bind(id)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("server '{id}'")))?;
    if server.is_local {
        return Err(Error::BadRequest(
            "the local server cannot be removed (it represents this host)".into(),
        ));
    }
    let in_use: i64 = sqlx::query_scalar("SELECT count(*) FROM applications WHERE server_id = ?")
        .bind(&server.id)
        .fetch_one(&state.pool)
        .await?;
    if in_use > 0 {
        return Err(Error::Conflict(format!(
            "{in_use} application(s) are still placed on this server"
        )));
    }
    sqlx::query("DELETE FROM servers WHERE id = ?")
        .bind(&server.id)
        .execute(&state.pool)
        .await?;
    println!("removed server {}", server.name);
    Ok(())
}

fn scoped_not_found(name: &str, org_id: &str) -> Error {
    Error::NotFound(format!(
        "no application named '{name}' in organization '{org_id}'"
    ))
}

#[allow(clippy::too_many_arguments)]
async fn add(
    state: &AppState,
    org_id: &str,
    name: &str,
    binary: &str,
    port: u16,
    domain: Option<String>,
    health: &str,
    metrics: &str,
    runtime: &str,
    args: Option<&str>,
) -> Result<()> {
    apps::validate_name(name)?;
    if binary.trim().is_empty() {
        return Err(Error::BadRequest(
            "app --binary is required (absolute path to a prebuilt binary)".into(),
        ));
    }
    if !matches!(runtime, "systemd" | "proc") {
        return Err(Error::BadRequest(
            "runtime must be 'systemd' or 'proc'".into(),
        ));
    }
    apps::ensure_port_free(
        &state.pool,
        port as i64,
        state.cfg.runtime.slot_offset as i64,
        "",
    )
    .await?;
    quotas::ensure_capacity(&state.pool, org_id, "", None, None).await?;
    if domain.as_ref().is_some_and(|d| !d.is_empty()) {
        quotas::ensure_domain_capacity(&state.pool, org_id).await?;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let app = sqlx::query_as::<_, Application>(
        "INSERT INTO applications \
         (id, org_id, name, binary_path, args, port, health_path, metrics_path, domain, runtime, \
          auto_restart, status) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 'stopped') RETURNING *",
    )
    .bind(&id)
    .bind(org_id)
    .bind(name)
    .bind(binary)
    .bind(args)
    .bind(port as i64)
    .bind(health)
    .bind(metrics)
    .bind(&domain)
    .bind(runtime)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db) = &e {
            if db.message().contains("UNIQUE") {
                return Error::Conflict(format!("application '{name}' already exists"));
            }
        }
        Error::Db(e)
    })?;
    // Seed the first placement, mirroring the API.
    sqlx::query(
        "INSERT OR IGNORE INTO app_servers (id, application_id, server_id, port) \
         VALUES (?, ?, 'local', ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&app.id)
    .bind(app.port)
    .execute(&state.pool)
    .await?;
    audit::record(
        state,
        Some(org_id),
        None,
        Some(&app.id),
        "app.create",
        Some("application"),
        Some(&app.id),
        Some(&serde_json::json!({"name": app.name, "port": app.port, "via": "cli"}).to_string()),
    )
    .await?;
    println!("created {} ({})", app.name, app.id);
    Ok(())
}

async fn deploy(state: &AppState, org_id: &str, name: &str) -> Result<()> {
    let app = sqlx::query_as::<_, Application>(
        "SELECT * FROM applications WHERE name = ? AND org_id = ?",
    )
    .bind(name)
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| scoped_not_found(name, org_id))?;
    let (dep_id, out) = apps::deploy_app(state, &app).await?;
    audit::record(
        state,
        Some(org_id),
        None,
        Some(&app.id),
        "app.deploy",
        Some("deployment"),
        Some(&dep_id),
        Some(&serde_json::json!({"artifact": out.artifact_hash, "via": "cli"}).to_string()),
    )
    .await?;
    println!(
        "deployed {} -> {} ({})",
        app.name,
        out.state.as_status(),
        out.artifact_hash
    );
    Ok(())
}

async fn rollback(state: &AppState, org_id: &str, name: &str) -> Result<()> {
    let app = sqlx::query_as::<_, Application>(
        "SELECT * FROM applications WHERE name = ? AND org_id = ?",
    )
    .bind(name)
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| scoped_not_found(name, org_id))?;
    let hashes: Vec<String> = sqlx::query_scalar(
        "SELECT artifact_hash FROM deployments \
         WHERE application_id = ? AND artifact_hash IS NOT NULL ORDER BY rowid DESC",
    )
    .bind(&app.id)
    .fetch_all(&state.pool)
    .await?;
    let previous = apps::select_previous_artifact(&hashes).ok_or_else(|| {
        Error::BadRequest("no previous build to roll back to (deploy at least twice first)".into())
    })?;
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
    let (dep_id, out) = apps::deploy_app_source(state, &app, source).await?;
    audit::record(
        state,
        Some(org_id),
        None,
        Some(&app.id),
        "app.rollback",
        Some("deployment"),
        Some(&dep_id),
        Some(&serde_json::json!({"rolled_back_to": previous, "via": "cli"}).to_string()),
    )
    .await?;
    println!(
        "rolled back {} -> {} ({})",
        app.name,
        previous,
        out.state.as_status()
    );
    Ok(())
}

/// Remove an application: stop its runtime units (legacy + both slots) and
/// delete it. Placements and deployments cascade in the database.
async fn remove(state: &AppState, org_id: &str, name: &str) -> Result<()> {
    let app = sqlx::query_as::<_, Application>(
        "SELECT * FROM applications WHERE name = ? AND org_id = ?",
    )
    .bind(name)
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| scoped_not_found(name, org_id))?;
    audit::record(
        state,
        Some(org_id),
        None,
        Some(&app.id),
        "app.delete",
        Some("application"),
        Some(&app.id),
        Some(&serde_json::json!({"name": app.name, "via": "cli"}).to_string()),
    )
    .await?;
    let runtime = apps::runtime_for(&state.cfg, &app.runtime);
    let offset = state.cfg.runtime.slot_offset as i64;
    let _ = runtime.remove(&apps::spec_for(&state.cfg, &app)).await;
    for (slot, port) in [
        (turaes_runtime::Slot::A, app.port),
        (turaes_runtime::Slot::B, app.port + offset),
    ] {
        let spec = apps::spec_for_slot(&state.cfg, &app, Some(slot), port as u16);
        let _ = runtime.remove(&spec).await;
    }
    sqlx::query("DELETE FROM applications WHERE id = ?")
        .bind(&app.id)
        .execute(&state.pool)
        .await?;
    let _ = apps::refresh_proxy_routes(state).await;
    println!("removed {}", app.name);
    Ok(())
}

async fn list(pool: &Pool, org_id: &str, json: bool) -> Result<()> {
    let apps = sqlx::query_as::<_, Application>(
        "SELECT * FROM applications WHERE org_id = ? ORDER BY created_at DESC",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    if json {
        println!("{}", serde_json::json!({ "applications": apps }));
        return Ok(());
    }
    if apps.is_empty() {
        println!("no applications");
        return Ok(());
    }
    println!("NAME             STATUS     PORT    DOMAIN                   BINARY");
    for a in apps {
        println!(
            "{:<16} {:<10} {:<7} {:<24} {}",
            a.name,
            a.status,
            a.port,
            a.domain.unwrap_or_else(|| "-".into()),
            a.binary_path
        );
    }
    Ok(())
}

async fn show(state: &AppState, org_id: &str, name: &str, json: bool) -> Result<()> {
    use sqlx::Row;

    let app = sqlx::query_as::<_, Application>(
        "SELECT * FROM applications WHERE name = ? AND org_id = ?",
    )
    .bind(name)
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| scoped_not_found(name, org_id))?;
    if json {
        println!("{}", serde_json::json!({ "application": app }));
        return Ok(());
    }

    println!("app        {}", app.name);
    println!("status     {}", app.status);
    println!("runtime    {}", app.runtime);
    println!("port       {}", app.port);
    println!(
        "domain     {}",
        app.domain.clone().unwrap_or_else(|| "-".into())
    );
    println!("binary     {}", app.binary_path);

    let metric = sqlx::query(
        "SELECT cpu_pct, mem_bytes, recorded_at FROM app_metrics \
         WHERE application_id = ? ORDER BY recorded_at DESC LIMIT 1",
    )
    .bind(&app.id)
    .fetch_optional(&state.pool)
    .await?;
    match metric {
        Some(row) => {
            let cpu: f64 = row.try_get("cpu_pct").unwrap_or(0.0);
            let mem: i64 = row.try_get("mem_bytes").unwrap_or(0);
            let at: String = row.try_get("recorded_at").unwrap_or_default();
            println!("cpu        {cpu:.1}%");
            println!("memory     {:.1} MB", mem as f64 / 1_048_576.0);
            println!("sampled_at {at}");
        }
        None => println!("cpu        (no samples yet)"),
    }

    let visits = sqlx::query(
        "SELECT COALESCE(SUM(visits),0) AS v, COALESCE(MAX(uniques),0) AS u \
         FROM visit_metrics WHERE application_id = ? AND recorded_at >= datetime('now','-24 hours')",
    )
    .bind(&app.id)
    .fetch_one(&state.pool)
    .await?;
    println!("visits_24h {}", visits.try_get::<i64, _>("v").unwrap_or(0));
    println!("unique_24h {}", visits.try_get::<i64, _>("u").unwrap_or(0));

    let health = sqlx::query(
        "SELECT status, status_code, response_time_ms, checked_at FROM health_results \
         WHERE application_id = ? ORDER BY checked_at DESC LIMIT 1",
    )
    .bind(&app.id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(row) = health {
        let status: String = row.try_get("status").unwrap_or_default();
        let code: Option<i64> = row.try_get("status_code").ok();
        let ms: Option<i64> = row.try_get("response_time_ms").ok();
        let at: String = row.try_get("checked_at").unwrap_or_default();
        println!(
            "health     {status} ({} {}) at {at}",
            code.map(|c| c.to_string()).unwrap_or_else(|| "-".into()),
            ms.map(|m| format!("{m}ms")).unwrap_or_else(|| "-".into())
        );
    }
    Ok(())
}
