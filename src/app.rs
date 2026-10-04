//! Router assembly and process bootstrap.

use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::auth;
use crate::routes;
use crate::state::AppState;

/// Build the full Axum router.
pub fn build_router(state: AppState) -> Router {
    // Tenant-scoped surface, nested under `/api/v1/orgs/{org}`. The `{org}`
    // capture accepts an organization id or slug; every handler enforces a
    // role floor for it (see `authz::authorize_org`).
    let org_api = Router::new()
        .route("/apps", get(routes::apps::list).post(routes::apps::create))
        .route(
            "/apps/{id}",
            get(routes::apps::get)
                .delete(routes::apps::delete)
                .patch(routes::apps::update),
        )
        .route("/apps/{id}/deploy", post(routes::apps::deploy))
        .route("/apps/{id}/rollback", post(routes::apps::rollback))
        .route("/apps/{id}/stop", post(routes::apps::stop))
        .route("/apps/{id}/start", post(routes::apps::start))
        .route("/apps/{id}/restart", post(routes::apps::restart))
        .route("/apps/{id}/stats", get(routes::apps::stats))
        .route("/apps/{id}/visitors", get(routes::apps::visitors))
        .route("/apps/{id}/deployments", get(routes::apps::deployments))
        .route(
            "/apps/{id}/domains",
            get(routes::domains::list).post(routes::domains::add),
        )
        .route(
            "/apps/{id}/domains/{domain}",
            delete(routes::domains::delete),
        )
        .route("/apps/{id}/logs", get(routes::logs::stream))
        .route("/apps/{id}/env", get(routes::env::list))
        .route(
            "/apps/{id}/env/{key}",
            axum::routing::put(routes::env::put).delete(routes::env::delete),
        )
        .route("/deployments/{id}", get(routes::deployments::get))
        .route("/audit", get(routes::audit::list))
        .route(
            "/tokens",
            get(routes::tokens::list).post(routes::tokens::create),
        )
        .route("/tokens/{id}", delete(routes::tokens::revoke));

    // Authenticated API surface: tenant-scoped routes plus the global
    // identity (`/me`), tenant-gated artifacts and operator-gated servers.
    let api = Router::new()
        .nest("/orgs/{org}", org_api)
        .route("/me", get(routes::me::me))
        .route("/artifacts/{hash}", get(routes::artifacts::download))
        .route(
            "/servers",
            get(routes::servers::list).post(routes::servers::create),
        )
        .route(
            "/servers/{id}",
            get(routes::servers::get).delete(routes::servers::delete),
        )
        .route("/servers/{id}/validate", post(routes::servers::validate))
        .route("/servers/{id}/bootstrap", post(routes::servers::bootstrap))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ))
        .with_state(state.clone());

    // Public surface: liveness + OAuth handshake.
    let public = Router::new()
        .route("/health", get(routes::health::health))
        .route("/auth/login", get(auth::login))
        .route("/auth/callback", get(auth::callback))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/agent/artifacts/{hash}", get(routes::agent::download))
        .with_state(state.clone());

    Router::new()
        .nest("/api/v1", api)
        .merge(public)
        .fallback(routes::static_assets::static_handler)
        .layer(TraceLayer::new_for_http())
}
