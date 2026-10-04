//! Artifact download. Used by remote agents (N1) and for manual retrieval.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Extension;

use turaes_core::{Error, Result};

use crate::authz::CurrentUser;
use crate::state::AppState;

/// `GET /api/v1/artifacts/{hash}` — stream a stored artifact by hash. Only
/// downloadable when the hash backs a deployment of an app in one of the
/// caller's organizations (artifacts are content-addressed but tenant-gated).
pub async fn download(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(hash): Path<String>,
) -> Result<Response> {
    if !state.artifacts.has(&hash) {
        return Err(Error::NotFound(format!("artifact {hash}")));
    }
    let orgs = user.org_ids();
    if orgs.is_empty() {
        return Err(Error::NotFound(format!("artifact {hash}")));
    }
    let placeholders = orgs.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT count(*) FROM deployments d JOIN applications a ON a.id = d.application_id \
         WHERE d.artifact_hash = ? AND a.org_id IN ({placeholders})"
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql).bind(&hash);
    for org in &orgs {
        q = q.bind(org);
    }
    let hits: i64 = q.fetch_one(&state.pool).await?;
    if hits == 0 {
        return Err(Error::NotFound(format!("artifact {hash}")));
    }
    let path = state.artifacts.path_for(&hash)?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| Error::Internal(format!("failed to read artifact: {e}")))?;
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from(bytes),
    )
        .into_response())
}
