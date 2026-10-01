//! Liveness endpoint (no database or auth required).

use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::state::AppState;

/// `GET /health` → process liveness plus build/runtime capabilities.
pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "runtime": state.cfg.runtime.driver,
        "proxy": turaes_proxy::pingora_enabled(),
    }))
}
