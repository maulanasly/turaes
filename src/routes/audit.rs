//! Audit log reads: who did what, in which org, to which target.

use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use serde::Deserialize;
use sqlx::FromRow;

use turaes_core::Result;

use crate::authz::{self, CurrentUser, Role};
use crate::state::AppState;

/// Query for the audit timeline.
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    /// Restrict to one application id.
    pub app: Option<String>,
    /// Restrict to one action verb (e.g. `app.deploy`).
    pub action: Option<String>,
    /// Max rows (default 50, max 200).
    pub limit: Option<i64>,
}

/// One audit row with the actor's login resolved.
#[derive(Debug, serde::Serialize, FromRow)]
pub struct AuditEntry {
    /// UUID.
    pub id: String,
    /// Owning organization, when known.
    pub org_id: Option<String>,
    /// Acting user, when known.
    pub actor_user_id: Option<String>,
    /// Acting user's login (`system` actions have none).
    pub actor_login: Option<String>,
    /// Related application, when any.
    pub application_id: Option<String>,
    /// Verb, e.g. `app.deploy`.
    pub action: String,
    /// Target kind.
    pub target_type: Option<String>,
    /// Target identifier.
    pub target_id: Option<String>,
    /// Operator context (never secrets).
    pub metadata: Option<String>,
    /// Event timestamp.
    pub created_at: String,
}

/// `GET /api/v1/orgs/{org}/audit` — newest-first timeline for the caller's org.
pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let entries: Vec<AuditEntry> = match (&q.app, &q.action) {
        (Some(app), Some(action)) => {
            sqlx::query_as(
                "SELECT l.id, l.org_id, l.actor_user_id, u.login AS actor_login, \
             l.application_id, l.action, l.target_type, l.target_id, l.metadata, l.created_at \
             FROM audit_log l LEFT JOIN users u ON u.id = l.actor_user_id \
             WHERE l.org_id = ? AND l.application_id = ? AND l.action = ? \
             ORDER BY l.created_at DESC, l.rowid DESC LIMIT ?",
            )
            .bind(&org_id)
            .bind(app)
            .bind(action)
            .bind(limit)
            .fetch_all(&state.pool)
            .await?
        }
        (Some(app), None) => {
            sqlx::query_as(
                "SELECT l.id, l.org_id, l.actor_user_id, u.login AS actor_login, \
             l.application_id, l.action, l.target_type, l.target_id, l.metadata, l.created_at \
             FROM audit_log l LEFT JOIN users u ON u.id = l.actor_user_id \
             WHERE l.org_id = ? AND l.application_id = ? \
             ORDER BY l.created_at DESC, l.rowid DESC LIMIT ?",
            )
            .bind(&org_id)
            .bind(app)
            .bind(limit)
            .fetch_all(&state.pool)
            .await?
        }
        (None, Some(action)) => {
            sqlx::query_as(
                "SELECT l.id, l.org_id, l.actor_user_id, u.login AS actor_login, \
             l.application_id, l.action, l.target_type, l.target_id, l.metadata, l.created_at \
             FROM audit_log l LEFT JOIN users u ON u.id = l.actor_user_id \
             WHERE l.org_id = ? AND l.action = ? \
             ORDER BY l.created_at DESC, l.rowid DESC LIMIT ?",
            )
            .bind(&org_id)
            .bind(action)
            .bind(limit)
            .fetch_all(&state.pool)
            .await?
        }
        (None, None) => {
            sqlx::query_as(
                "SELECT l.id, l.org_id, l.actor_user_id, u.login AS actor_login, \
             l.application_id, l.action, l.target_type, l.target_id, l.metadata, l.created_at \
             FROM audit_log l LEFT JOIN users u ON u.id = l.actor_user_id \
             WHERE l.org_id = ? \
             ORDER BY l.created_at DESC, l.rowid DESC LIMIT ?",
            )
            .bind(&org_id)
            .bind(limit)
            .fetch_all(&state.pool)
            .await?
        }
    };
    Ok(Json(serde_json::json!({ "audit": entries })))
}
