//! turaes entry point: CLI parsing, tracing, and process bootstrap.

mod app;
mod auth;
mod cli;
mod routes;
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
            db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));
            println!("migrations applied");
        }
        Command::Doctor => doctor(&cfg),
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
    println!("  monitor.interval  {}s", cfg.monitor.interval_secs);
    println!("  proxy.enabled     {}", cfg.proxy.enabled);
    println!("  proxy.pingora     {}", turaes_proxy::pingora_enabled());
    println!("  secure_cookies    {}", cfg.secure_cookies());
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
    db::migrate(&pool).await.unwrap_or_else(|e| fatal(e));

    let state = AppState::new(cfg.clone(), pool);
    let router = app::build_router(state);

    if cfg.proxy.enabled {
        let proxy_router =
            turaes_proxy::Router::new(cfg.server.base_domain.clone(), Default::default());
        let proxy_state = turaes_proxy::state(&cfg.proxy, proxy_router);
        let proxy_cfg = cfg.proxy.clone();
        tokio::spawn(async move {
            if let Err(e) = turaes_proxy::service::serve(&proxy_cfg, proxy_state).await {
                tracing::error!(error = %e, "proxy stopped");
            }
        });
    }

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| fatal(format!("failed to bind {addr}: {e}")));
    tracing::info!(%addr, "turaes listening");
    axum::serve(listener, router)
        .await
        .unwrap_or_else(|e| fatal(e));
}
