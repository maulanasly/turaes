//! Deployment history detail.

use axum::extract::{Path, State};
use axum::{Extension, Json};

use turaes_core::models::Deployment;
use turaes_core::{Error, Result};

use crate::authz::{self, CurrentUser, Role};
use crate::state::AppState;

/// `GET /api/v1/orgs/{org}/deployments/{id}` — full deployment record incl.
/// log. Only visible when the deployment's app is in the caller's org.
pub async fn get(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let dep = sqlx::query_as::<_, Deployment>(
        "SELECT d.* FROM deployments d JOIN applications a ON a.id = d.application_id \
         WHERE d.id = ? AND a.org_id = ?",
    )
    .bind(&id)
    .bind(&org_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| Error::NotFound(format!("deployment {id}")))?;
    Ok(Json(serde_json::json!({ "deployment": dep })))
}
