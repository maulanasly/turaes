//! Artifact download. Used by remote agents (N1) and for manual retrieval.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};

use turaes_core::{Error, Result};

use crate::state::AppState;

/// `GET /api/v1/artifacts/{hash}` — stream a stored artifact by hash.
pub async fn download(State(state): State<AppState>, Path(hash): Path<String>) -> Result<Response> {
    if !state.artifacts.has(&hash) {
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
