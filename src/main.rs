//! turaes entry point: CLI parsing, tracing, and process bootstrap.

mod agent;
mod app;
mod audit;
mod auth;
mod authz;
mod backup;
mod bootstrap;
mod cli;
mod commands;
mod edge;
mod grpc;
mod monitor;
mod routes;
mod secrets;
mod state;

#[cfg(test)]
mod tests;

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

    let cfg = match Config::load(cli.config.as_deref()) {
        Ok(cfg) => Arc::new(cfg),
        Err(e) => fatal(e),
    };

    match cli.command.unwrap_or(Command::Serve) {
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
        Command::Doctor => doctor(&cfg),
        Command::App { cmd } => {
            let pool = db::connect(&cfg.database.url)
                .await
                .unwrap_or_else(|e| fatal(e));
            db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));
            let state = AppState::new(cfg.clone(), pool);
            commands::run(&state, cmd)
                .await
                .unwrap_or_else(|e| fatal(e));
        }
        Command::Server { cmd } => {
            let pool = db::connect(&cfg.database.url)
                .await
                .unwrap_or_else(|e| fatal(e));
            db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));
            let state = AppState::new(cfg.clone(), pool);
            commands::run_server(&state, cmd)
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
        Command::Secrets { cmd } => match cmd {
            cli::SecretsCommand::Reseal => {
                let pool = db::connect(&cfg.database.url)
                    .await
                    .unwrap_or_else(|e| fatal(e));
                db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));
                let state = AppState::new(cfg.clone(), pool);
                let (env_up, env_total, ssh_up, ssh_total) =
                    secrets::reseal(&state).await.unwrap_or_else(|e| fatal(e));
                println!("env_vars resealed {env_up}/{env_total}; server keys resealed {ssh_up}/{ssh_total}");
                if !cfg.auth.secret_previous.is_empty() {
                    println!(
                        "rotation staged: verify apps still deploy, \
                         then unset TURAES_SECRET_PREVIOUS"
                    );
                }
            }
        },
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

fn doctor(cfg: &Config) {
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
