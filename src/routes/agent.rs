//! Agent-facing HTTP endpoints, authenticated by the agent token (bearer),
//! not by a user session.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};

use turaes_core::crypto::token_hash;
use turaes_core::models::Server;
use turaes_core::{Error, Result};

use crate::state::AppState;

/// `GET /agent/artifacts/{hash}` — agent downloads an artifact with its token.
pub async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> Result<Response> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::auth::bearer_scheme_token)
        .unwrap_or("");
    if token.is_empty() {
        return Err(Error::Unauthorized(
            "missing agent token (set TURAES_AGENT_TOKEN to the join token)".into(),
        ));
    }

    let server: Option<Server> =
        sqlx::query_as::<_, Server>("SELECT * FROM servers WHERE agent_token_hash = ?")
            .bind(token_hash(token))
            .fetch_optional(&state.pool)
            .await?;
    let Some(server) = server else {
        return Err(Error::Unauthorized(
            "unknown agent token (re-register the agent to mint a fresh one)".into(),
        ));
    };
    let _ = sqlx::query(
        "UPDATE servers SET status = 'online', last_seen_at = datetime('now') WHERE id = ?",
    )
    .bind(&server.id)
    .execute(&state.pool)
    .await;

    if !state.artifacts.has(&hash) {
        return Err(Error::NotFound(format!("artifact {hash}")));
    }
    // Tenancy: the hash must serve this server — pinned by one of its
    // deployments or releases. A bare store hit is not enough; agents must
    // not exfiltrate other tenants' pre-deploy blobs. Same 404 either way
    // so missing and forbidden are indistinguishable.
    let bare = hash.strip_prefix("sha256:").unwrap_or(&hash);
    let serves: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (
           SELECT 1 FROM deployments d JOIN app_servers p ON p.application_id = d.application_id \
            WHERE (d.artifact_hash = ? OR d.artifact_hash = ?) AND p.server_id = ?
           UNION
           SELECT 1 FROM releases r JOIN artifacts t ON t.id = r.artifact_id \
            JOIN app_servers p ON p.application_id = r.application_id \
            WHERE (t.hash = ? OR t.hash = ?) AND p.server_id = ?
         )",
    )
    .bind(&hash)
    .bind(bare)
    .bind(&server.id)
    .bind(&hash)
    .bind(bare)
    .bind(&server.id)
    .fetch_one(&state.pool)
    .await?;
    if serves == 0 {
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
