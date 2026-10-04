//! Shared application state handed to every request handler.

use std::sync::Arc;

use sqlx::SqlitePool;

use turaes_core::artifact::ArtifactStore;
use turaes_core::config::Config;
use turaes_core::crypto::{SecretBox, TokenIssuer};

/// Cheap-to-clone state (`Arc` + pooled handles).
#[derive(Clone)]
pub struct AppState {
    /// Resolved configuration.
    pub cfg: Arc<Config>,
    /// SQLite pool.
    pub pool: SqlitePool,
    /// Content-addressed artifact store.
    pub artifacts: ArtifactStore,
    /// Session token issuer.
    pub issuer: TokenIssuer,
    /// Secret sealer for app environment variables.
    pub secrets: SecretBox,
    /// Shared HTTP client (health probes, OAuth calls).
    pub http: reqwest::Client,
    /// Reverse-proxy routing table, when the proxy is enabled.
    pub proxy_router: Option<std::sync::Arc<turaes_proxy::Router>>,
    /// When true (debug builds + `AUTH_DISABLED=1`), requests run as a dev user.
    pub auth_disabled: bool,
}

impl AppState {
    /// Build state from configuration and a database pool.
    pub fn new(cfg: Arc<Config>, pool: SqlitePool) -> Self {
        let auth_disabled = cfg!(debug_assertions)
            && matches!(
                std::env::var("AUTH_DISABLED").as_deref(),
                Ok("1") | Ok("true")
            );
        if auth_disabled {
            tracing::warn!(
                "AUTH_DISABLED=1 — every request runs as the `dev` user (debug builds only)"
            );
        }
        Self {
            artifacts: ArtifactStore::new(&cfg.runtime.artifact_dir),
            issuer: TokenIssuer::new(&cfg.auth.jwt_secret, cfg.auth.session_ttl_days),
            secrets: if cfg.auth.secret_previous.is_empty() {
                SecretBox::new(&cfg.auth.jwt_secret)
            } else {
                tracing::warn!(
                    "TURAES_SECRET_PREVIOUS is set: old ciphers still decrypt; \
                     run `turaes secrets reseal`, verify, then unset it"
                );
                SecretBox::with_previous(&cfg.auth.jwt_secret, &cfg.auth.secret_previous)
            },
            http: reqwest::Client::new(),
            proxy_router: None,
            auth_disabled,
            cfg,
            pool,
        }
    }

    /// Build state with auth forced off (test helper — no env mutation).
    #[cfg(test)]
    pub fn for_test(cfg: Arc<Config>, pool: SqlitePool) -> Self {
        let mut state = Self::new(cfg, pool);
        state.auth_disabled = true;
        state
    }
}
