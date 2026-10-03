//! One-shot CLI operations that act directly on the local database.
//!
//! These exist so an operator can bootstrap and deploy apps on a fresh server
//! without configuring GitHub OAuth first. Conventions match the HTTP API.

use turaes_core::db::Pool;
use turaes_core::models::{Application, Server};
use turaes_core::{Error, Result};

use crate::cli::{AppCommand, ServerCommand};
use crate::routes::apps;
use crate::state::AppState;

/// Dispatch a `turaes app ...` subcommand.
pub async fn run(state: &AppState, cmd: AppCommand) -> Result<()> {
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
        AppCommand::Deploy { name } => deploy(state, &name).await,
        AppCommand::Rollback { name } => rollback(state, &name).await,
        AppCommand::List => list(&state.pool).await,
        AppCommand::Show { name } => show(state, &name).await,
    }
}

/// Dispatch a `turaes server ...` subcommand.
pub async fn run_server(state: &AppState, cmd: ServerCommand) -> Result<()> {
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
        ServerCommand::List => server_list(&state.pool).await,
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
        return Err(Error::BadRequest("name and address are required".into()));
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

async fn server_list(pool: &Pool) -> Result<()> {
    let servers =
        sqlx::query_as::<_, Server>("SELECT * FROM servers ORDER BY is_local DESC, name ASC")
            .fetch_all(pool)
            .await?;
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
            "the local server cannot be removed".into(),
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

#[allow(clippy::too_many_arguments)]
async fn add(
    state: &AppState,
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
        return Err(Error::BadRequest("binary path is required".into()));
    }
    if !matches!(runtime, "systemd" | "proc") {
        return Err(Error::BadRequest(
            "runtime must be 'systemd' or 'proc'".into(),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let app = sqlx::query_as::<_, Application>(
        "INSERT INTO applications \
         (id, name, binary_path, args, port, health_path, metrics_path, domain, runtime, \
          auto_restart, status) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 'stopped') RETURNING *",
    )
    .bind(&id)
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
    println!("created {} ({})", app.name, app.id);
    Ok(())
}

async fn deploy(state: &AppState, name: &str) -> Result<()> {
    let app = sqlx::query_as::<_, Application>("SELECT * FROM applications WHERE name = ?")
        .bind(name)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("application '{name}'")))?;
    let (_dep_id, out) = apps::deploy_app(state, &app).await?;
    println!(
        "deployed {} -> {} ({})",
        app.name,
        out.state.as_status(),
        out.artifact_hash
    );
    Ok(())
}

async fn rollback(state: &AppState, name: &str) -> Result<()> {
    let app = sqlx::query_as::<_, Application>("SELECT * FROM applications WHERE name = ?")
        .bind(name)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("application '{name}'")))?;
    let hashes: Vec<String> = sqlx::query_scalar(
        "SELECT artifact_hash FROM deployments \
         WHERE application_id = ? AND artifact_hash IS NOT NULL ORDER BY rowid DESC",
    )
    .bind(&app.id)
    .fetch_all(&state.pool)
    .await?;
    let previous = apps::select_previous_artifact(&hashes)
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
    let (_dep_id, out) = apps::deploy_app_source(state, &app, source).await?;
    println!(
        "rolled back {} -> {} ({})",
        app.name,
        previous,
        out.state.as_status()
    );
    Ok(())
}

async fn list(pool: &Pool) -> Result<()> {
    let apps =
        sqlx::query_as::<_, Application>("SELECT * FROM applications ORDER BY created_at DESC")
            .fetch_all(pool)
            .await?;
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

async fn show(state: &AppState, name: &str) -> Result<()> {
    use sqlx::Row;

    let app = sqlx::query_as::<_, Application>("SELECT * FROM applications WHERE name = ?")
        .bind(name)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("application '{name}'")))?;

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
