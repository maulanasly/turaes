//! Router assembly and process bootstrap.

use std::sync::Arc;

use axum::http::{header, HeaderValue};
use axum::routing::{delete, get, patch, post};
use axum::Router;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::auth;
use crate::routes;
use crate::security;
use crate::state::AppState;

/// Build the full Axum router.
pub fn build_router(state: AppState) -> Router {
    // Tenant-scoped surface, nested under `/api/v1/orgs/{org}`. The `{org}`
    // capture accepts an organization id or slug; every handler enforces a
    // role floor for it (see `authz::authorize_org`).
    let org_api = Router::new()
        .route("/apps", get(routes::apps::list).post(routes::apps::create))
        .route("/apps/preflight", post(routes::apps::preflight))
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
        .route("/apps/{id}/maintenance", post(routes::apps::maintenance))
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
        .route("/catalog", get(routes::catalog::get))
        .route(
            "/registry/links",
            post(routes::registry::create_link).delete(routes::registry::delete_link),
        )
        // Raw-bytes upload: disable the repo-wide 2 MiB default; the
        // handler enforces the 256 MiB registry ceiling while streaming.
        .route(
            "/registry/artifacts",
            post(routes::registry::upload).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route("/registry/releases", post(routes::registry::create_release))
        .route(
            "/registry/releases/{id}/promote",
            post(routes::registry::promote),
        )
        .route("/registry/releases/{id}/yank", post(routes::registry::yank))
        .route("/registry/{app}/links", get(routes::registry::list_links))
        .route(
            "/registry/{app}/releases",
            get(routes::registry::list_releases),
        )
        .route("/registry/{app}/resolve", get(routes::registry::resolve))
        .route("/alerts", get(routes::alerts::list))
        .route("/alerts/{id}/resolve", post(routes::alerts::resolve))
        .route("/quota", get(routes::quotas::get).put(routes::quotas::put))
        .route(
            "/tokens",
            get(routes::tokens::list).post(routes::tokens::create),
        )
        .route("/tokens/{id}", delete(routes::tokens::revoke));

    // Authenticated API surface: tenant-scoped routes plus the global
    // identity (`/me`), the caller's organizations, tenant-gated artifacts
    // and operator-gated servers.
    let api = Router::new()
        .nest("/orgs/{org}", org_api)
        .route("/me", get(routes::me::me))
        .route("/orgs", get(routes::orgs::list).post(routes::orgs::create))
        .route(
            "/orgs/{org}/members",
            get(routes::orgs::members).post(routes::orgs::invite),
        )
        .route(
            "/orgs/{org}/members/{user_id}",
            patch(routes::orgs::change_role).delete(routes::orgs::remove),
        )
        .route("/artifacts/{hash}", get(routes::artifacts::download))
        .route(
            "/servers",
            get(routes::servers::list).post(routes::servers::create),
        )
        .route(
            "/servers/{id}",
            get(routes::servers::get).delete(routes::servers::delete),
        )
        .route("/servers/{id}/stats", get(routes::servers::stats))
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

    // Perimeter, innermost first: CSRF binding, then global rate limits, then
    // the request timeout. (Layers added later run earlier, so tracing stays
    // outermost.) Security headers ride on every response, including the
    // dashboard and health checks. HSTS is only emitted for https origins,
    // and without includeSubDomains so sibling hosts are never affected.
    // No CSP: the zero-build UI boots from an inline script by design.
    let limiter = Arc::new(security::RateLimiter::defaults());
    let mut router = Router::new()
        .nest("/api/v1", api)
        .merge(public)
        .fallback(routes::static_assets::static_handler)
        .layer(axum::middleware::from_fn(security::csrf))
        .layer(axum::middleware::from_fn_with_state(
            limiter,
            security::limit,
        ))
        .layer(axum::middleware::from_fn(security::timeout))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        ));
    if state.cfg.secure_cookies() {
        router = router.layer(SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        ));
    }
    router.layer(TraceLayer::new_for_http())
}
