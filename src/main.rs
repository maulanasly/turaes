//! turaes entry point: CLI parsing, tracing, and process bootstrap.

mod agent;
mod alerts;
mod app;
mod apply;
mod audit;
mod auth;
mod authz;
mod backup;
mod bootstrap;
mod cli;
mod commands;
mod edge;
mod gc;
mod grpc;
mod monitor;
mod routes;
mod secrets;
mod security;
mod serve_static;
mod state;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::{Cli, Command};
use state::AppState;
use turaes_core::config::Config;
use turaes_core::db;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_tracing();

    // serve-static is config-free: it is the supervision target for `static`
    // apps, whose units carry no EnvironmentFile (and must boot with none).
    if let Some(Command::ServeStatic { dir, port }) = &cli.command {
        serve_static::run(dir.clone(), *port)
            .await
            .unwrap_or_else(|e| fatal(e));
        return;
    }

    let cfg = match Config::load(cli.config.as_deref()) {
        Ok(cfg) => Arc::new(cfg),
        Err(e) => fatal(e),
    };

    match cli.command.unwrap_or(Command::Serve) {
        // Handled before Config::load above (config-free by design).
        Command::ServeStatic { .. } => unreachable!("serve-static runs before config load"),
        Command::Migrate => {
            let pool = db::connect(&cfg.database.url)
                .await
                .unwrap_or_else(|e| fatal(e));
            backup::snapshot_before_migrate(&cfg, &pool)
                .await
                .unwrap_or_else(|e| fatal(e));
            db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));
            println!("migrations applied");
        }
        Command::Doctor { json } => doctor(&cfg, json).await.unwrap_or_else(|e| fatal(e)),
        Command::App { cmd, org, json } => {
            let pool = db::connect(&cfg.database.url)
                .await
                .unwrap_or_else(|e| fatal(e));
            db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));
            let state = AppState::new(cfg.clone(), pool);
            commands::run(&state, cmd, org.as_deref(), json)
                .await
                .unwrap_or_else(|e| fatal(e));
        }
        Command::Server { cmd, json } => {
            let pool = db::connect(&cfg.database.url)
                .await
                .unwrap_or_else(|e| fatal(e));
            db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));
            let state = AppState::new(cfg.clone(), pool);
            commands::run_server(&state, cmd, json)
                .await
                .unwrap_or_else(|e| fatal(e));
        }
        Command::Agent {
            control,
            http,
            token,
            name,
            address,
            interval,
        } => {
            agent::run(
                cfg,
                agent::AgentArgs {
                    control,
                    http,
                    token,
                    name,
                    address,
                    interval,
                },
            )
            .await
            .unwrap_or_else(|e| fatal(e));
        }
        Command::Edge {
            control,
            token,
            interval,
        } => {
            edge::run(
                cfg,
                edge::EdgeArgs {
                    control,
                    token,
                    interval,
                },
            )
            .await
            .unwrap_or_else(|e| fatal(e));
        }
        Command::Backup => {
            let pool = db::connect(&cfg.database.url)
                .await
                .unwrap_or_else(|e| fatal(e));
            let (path, pruned) = backup::snapshot_now(&cfg, &pool)
                .await
                .unwrap_or_else(|e| fatal(e));
            println!("snapshot {}", path.display());
            println!(
                "pruned {pruned} old snapshot(s), retaining {}",
                cfg.backup.retain
            );
        }
        Command::Restore { file, force } => {
            if !force {
                fatal(
                    "refusing to restore a live database: stop turaes \
                     (systemctl stop turaes) and re-run with --force",
                );
            }
            let dest = backup::db_path(&cfg.database.url).unwrap_or_else(|e| fatal(e));
            backup::restore_file(&file, &dest).unwrap_or_else(|e| fatal(e));
            println!("restored {} from {}", dest.display(), file.display());
            println!("start turaes again: systemctl start turaes");
        }
        Command::Apply { file, org, dry_run } => {
            let path = file.unwrap_or_else(|| PathBuf::from("turaes.yaml"));
            let pool = db::connect(&cfg.database.url)
                .await
                .unwrap_or_else(|e| fatal(e));
            db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));
            let state = AppState::new(cfg.clone(), pool);
            let org_id = commands::resolve_org(&state.pool, org.as_deref())
                .await
                .unwrap_or_else(|e| fatal(e));
            let (manifest, base_dir) = apply::load_manifest(&path).unwrap_or_else(|e| fatal(e));
            let report = apply::apply_manifest(&state, &org_id, &manifest, &base_dir, dry_run)
                .await
                .unwrap_or_else(|e| fatal(e));
            print!("{report}");
        }
        Command::Secrets { cmd, org } => {
            let pool = db::connect(&cfg.database.url)
                .await
                .unwrap_or_else(|e| fatal(e));
            db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));
            let state = AppState::new(cfg.clone(), pool);
            commands::run_secrets(&state, cmd, org.as_deref())
                .await
                .unwrap_or_else(|e| fatal(e));
        }
        Command::Gc { dry_run } => {
            let pool = db::connect(&cfg.database.url)
                .await
                .unwrap_or_else(|e| fatal(e));
            db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));
            let state = AppState::new(cfg.clone(), pool);
            let report = gc::collect_artifacts(&state, dry_run)
                .await
                .unwrap_or_else(|e| fatal(e));
            if dry_run {
                println!("dry run: nothing deleted");
            }
            println!(
                "artifacts: removed {} blob(s) ({} bytes), swept {} temp upload(s), kept {} blob(s), expired {} orphan row(s)",
                report.blobs_removed,
                report.bytes_freed,
                report.tmps_swept,
                report.blobs_kept,
                report.rows_expired
            );
        }
        Command::Serve => serve(cfg).await,
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("TURAES_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn fatal<E: std::fmt::Display>(err: E) -> ! {
    eprintln!("fatal: {err}");
    std::process::exit(1);
}

/// One capability check result for `doctor`.
struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}

fn check_ok(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        ok: true,
        detail: detail.into(),
    }
}

fn check_fail(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        ok: false,
        detail: detail.into(),
    }
}

/// Fail when `path` exists and is readable beyond its owner (want 0600).
fn check_private_file(name: &'static str, path: &str) -> Check {
    let path = std::path::Path::new(path);
    if path.as_os_str().is_empty() || !path.exists() {
        return check_ok(name, "not present".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) if meta.permissions().mode() & 0o077 == 0 => {
                check_ok(name, format!("{} is owner-only", path.display()))
            }
            Ok(_) => check_fail(
                name,
                format!(
                    "{} is readable beyond its owner (want 0600)",
                    path.display()
                ),
            ),
            Err(e) => check_fail(name, format!("cannot stat {}: {e}", path.display())),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        check_ok(name, "mode check is unix-only".to_string())
    }
}

/// Every rendered app env file (`{env_dir}/*.env`) holds decrypted secrets.
fn check_env_file_modes(env_dir: &str) -> Check {
    let mut offenders = Vec::new();
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(env_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "env") {
                count += 1;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = std::fs::metadata(&p) {
                        if meta.permissions().mode() & 0o077 != 0 {
                            offenders.push(p.display().to_string());
                        }
                    }
                }
            }
        }
    }
    if offenders.is_empty() {
        check_ok("env.modes", format!("{count} app env file(s) owner-only"))
    } else {
        check_fail(
            "env.modes",
            format!("world-readable: {}", offenders.join(", ")),
        )
    }
}

/// A directory turaes must be able to create and write.
fn check_writable_dir(name: &'static str, dir: &str) -> Check {
    if let Err(e) = std::fs::create_dir_all(dir) {
        return check_fail(name, format!("cannot create {dir}: {e}"));
    }
    let probe = std::path::Path::new(dir).join(".turaes-writetest");
    match std::fs::write(&probe, b"ok").and_then(|_| std::fs::remove_file(&probe)) {
        Ok(()) => check_ok(name, dir.to_string()),
        Err(e) => check_fail(name, format!("{dir} is not writable: {e}")),
    }
}

async fn doctor(cfg: &Config, json: bool) -> turaes_core::Result<()> {
    // Capability checks first: both output modes share them, and `--json`
    // must emit pure JSON (no human lines) for scripting.
    let mut checks: Vec<Check> = Vec::new();
    match db::connect(&cfg.database.url).await {
        Ok(pool) => {
            match sqlx::query_scalar::<_, i64>("SELECT 1")
                .fetch_one(&pool)
                .await
            {
                Ok(_) => checks.push(check_ok("database", "opens and answers")),
                Err(e) => checks.push(check_fail("database", format!("query failed: {e}"))),
            }
        }
        Err(e) => checks.push(check_fail("database", format!("cannot open: {e}"))),
    }
    for (name, dir) in [
        ("dir.backup", cfg.backup.dir.as_str()),
        ("dir.artifacts", cfg.runtime.artifact_dir.as_str()),
        ("dir.units", cfg.runtime.unit_dir.as_str()),
        ("dir.bin", cfg.runtime.bin_dir.as_str()),
        ("dir.state", cfg.runtime.state_dir.as_str()),
        ("dir.env", cfg.runtime.env_dir.as_str()),
        ("dir.acme", cfg.proxy.acme_webroot.as_str()),
    ] {
        checks.push(check_writable_dir(name, dir));
    }
    // Secrets at rest must stay owner-only: the live database (sealed blobs,
    // token hashes) and every rendered app env file (decrypted secrets).
    checks.push(check_private_file(
        "db.mode",
        &backup::db_path(&cfg.database.url)
            .unwrap_or_default()
            .to_string_lossy(),
    ));
    checks.push(check_env_file_modes(&cfg.runtime.env_dir));
    if cfg.proxy.enabled {
        match cfg.dashboard_host() {
            Some(host) => {
                let cert = format!("{}/{host}/fullchain.pem", cfg.proxy.cert_dir);
                if std::path::Path::new(&cert).is_file() {
                    checks.push(check_ok("tls.cert", format!("{cert} present")));
                } else {
                    checks.push(check_fail(
                        "tls.cert",
                        format!("{cert} missing (certbot has not issued for {host})"),
                    ));
                }
            }
            None => checks.push(check_fail(
                "tls.cert",
                "no dashboard hostname derivable from server.public_url".to_string(),
            )),
        }
    }

    let failed = checks.iter().filter(|c| !c.ok).count();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "ok": failed == 0,
                "checks": checks
                    .iter()
                    .map(|c| serde_json::json!({"name": c.name, "ok": c.ok, "detail": c.detail}))
                    .collect::<Vec<_>>(),
            })
        );
        return if failed > 0 {
            Err(turaes_core::Error::Internal(format!(
                "doctor: {failed} check(s) failed"
            )))
        } else {
            Ok(())
        };
    }

    println!("turaes {}", env!("CARGO_PKG_VERSION"));
    println!(
        "  listen            {}:{}",
        cfg.server.host, cfg.server.port
    );
    println!("  base_domain       {}", cfg.server.base_domain);
    println!("  public_url        {}", cfg.server.public_url);
    println!("  app_origin        {}", cfg.auth.app_origin);
    println!("  database          {}", cfg.database.url);
    println!(
        "  jwt_secret        {} (len {})",
        redact(&cfg.auth.jwt_secret),
        cfg.auth.jwt_secret.len()
    );
    println!(
        "  github_client_id  {}",
        if cfg.auth.github_client_id.is_empty() {
            "(unset)"
        } else {
            "set"
        }
    );
    println!("  allowlist         {:?}", cfg.auth.allowed_github_ids);
    println!("  runtime.driver    {}", cfg.runtime.driver);
    println!("  runtime.unit_dir  {}", cfg.runtime.unit_dir);
    println!("  runtime.bin_dir   {}", cfg.runtime.bin_dir);
    println!("  runtime.artifact  {}", cfg.runtime.artifact_dir);
    println!("  monitor.interval  {}s", cfg.monitor.interval_secs);
    println!(
        "  backup            {} (retain {}, {})",
        cfg.backup.dir,
        cfg.backup.retain,
        latest_snapshot(&cfg.backup.dir)
    );
    let (artifact_files, artifact_bytes) = gc::store_usage(&cfg.runtime.artifact_dir);
    println!(
        "  artifacts         {artifact_files} file(s), {} in store",
        human_bytes(artifact_bytes)
    );
    println!(
        "  seal              v1 HKDF-SHA256 (previous secret {})",
        if cfg.auth.secret_previous.is_empty() {
            "unset"
        } else {
            "STAGED FOR ROTATION"
        }
    );
    if cfg.auth.jwt_secret == turaes_core::config::DEFAULT_JWT_PLACEHOLDER {
        println!("  WARNING           default jwt_secret placeholder is active");
    }
    if cfg.auth.allowed_github_ids.is_empty() && !cfg.auth.allow_open_signin {
        println!("  WARNING           sign-in allowlist is empty (open to all GitHub users)");
    }
    println!("  proxy.enabled     {}", cfg.proxy.enabled);
    println!(
        "  grpc.enabled      {} ({}:{})",
        cfg.grpc.enabled, cfg.grpc.host, cfg.grpc.port
    );
    println!(
        "  agent.join_token  {}",
        if cfg.agent.join_token.is_empty() {
            "(unset)"
        } else {
            "set"
        }
    );
    println!("  proxy.pingora     {}", turaes_proxy::pingora_enabled());
    println!("  secure_cookies    {}", cfg.secure_cookies());

    for c in &checks {
        println!(
            "  check.{:<12} {} {}",
            c.name,
            if c.ok { "ok  " } else { "FAIL" },
            c.detail
        );
    }
    if failed > 0 {
        return Err(turaes_core::Error::Internal(format!(
            "doctor: {failed} check(s) failed"
        )));
    }
    Ok(())
}

/// Human-readable backup freshness for `doctor`: newest snapshot + age, or
/// why there is nothing to report.
fn latest_snapshot(dir: &str) -> String {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return "no backup dir yet".into(),
    };
    let mut snaps: Vec<(std::path::PathBuf, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "db"))
        .filter_map(|p| {
            std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .ok()
                .map(|t| (p, t))
        })
        .collect();
    if snaps.is_empty() {
        return "no snapshots yet".into();
    }
    snaps.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    let (path, mtime) = &snaps[0];
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match mtime.elapsed() {
        Ok(age) => {
            let hours = age.as_secs() / 3600;
            format!("newest {name} ({hours}h ago, {} total)", snaps.len())
        }
        Err(_) => format!("newest {name}"),
    }
}

/// Compact byte counts for `doctor` (`1.5 MB`, `512 B`).
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn redact(secret: &str) -> String {
    if secret.len() <= 8 {
        "********".into()
    } else {
        format!("{}…{}", &secret[..4], &secret[secret.len() - 4..])
    }
}

async fn serve(cfg: Arc<Config>) {
    let pool = db::connect(&cfg.database.url)
        .await
        .unwrap_or_else(|e| fatal(e));
    // Fail-closed: snapshot before running (possibly destructive) migrations.
    backup::snapshot_before_migrate(&cfg, &pool)
        .await
        .unwrap_or_else(|e| fatal(e));
    db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));

    let mut state = AppState::new(cfg.clone(), pool);

    if cfg.proxy.enabled {
        let proxy_router = Arc::new(turaes_proxy::Router::new(
            cfg.server.base_domain.clone(),
            Default::default(),
        ));
        state.proxy_router = Some(proxy_router.clone());
        if let Err(e) = routes::apps::refresh_proxy_routes(&state).await {
            tracing::warn!(error = %e, "failed to build initial proxy routes");
        }
        let proxy_state = turaes_proxy::state(&cfg.proxy, proxy_router, cfg.dashboard_host());
        turaes_proxy::service::spawn(cfg.proxy.clone(), proxy_state);

        // Rebuild routes periodically so DB-driven changes made by other
        // processes (e.g. a CLI deploy or blue/green cutover) propagate to the
        // running proxy.
        let refresh_state = state.clone();
        let secs = cfg.monitor.interval_secs.max(5);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                if let Err(e) = routes::apps::refresh_proxy_routes(&refresh_state).await {
                    tracing::warn!(error = %e, "periodic proxy route refresh failed");
                }
            }
        });
    }
    tokio::spawn(monitor::run(state.clone()));

    if cfg.grpc.enabled {
        let grpc_state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = grpc::serve(grpc_state).await {
                tracing::error!(error = %e, "gRPC server stopped");
            }
        });
    }

    let router = app::build_router(state);

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| fatal(format!("failed to bind {addr}: {e}")));
    tracing::info!(%addr, "turaes listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|e| fatal(e));
}

/// Resolve on SIGTERM/SIGINT so the API drains in-flight requests on restart.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
    tracing::info!("shutdown signal received; draining in-flight requests");
}
