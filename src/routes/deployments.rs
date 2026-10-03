//! Deployment history detail.

use axum::extract::{Path, State};
use axum::Json;

use turaes_core::models::Deployment;
use turaes_core::{Error, Result};

use crate::state::AppState;

/// `GET /api/v1/deployments/{id}` — full deployment record incl. log.
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let dep = sqlx::query_as::<_, Deployment>("SELECT * FROM deployments WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("deployment {id}")))?;
    Ok(Json(serde_json::json!({ "deployment": dep })))
}
