//! Alert timeline reads and manual resolution.
//!
//! App alerts are org-scoped (viewer reads). Platform-global alerts (NULL org)
//! additionally surface to org admins, who operate the platform for the org.

use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use serde::Deserialize;

use turaes_core::models::Alert;
use turaes_core::{Error, Result};

use crate::audit;
use crate::authz::{self, CurrentUser, Role};
use crate::state::AppState;

/// Query for the alert timeline.
#[derive(Debug, Deserialize)]
pub struct AlertsQuery {
    /// `firing` (default) or `resolved`.
    pub status: Option<String>,
    /// Max rows (default 50, max 200).
    pub limit: Option<i64>,
}

/// `GET /api/v1/orgs/{org}/alerts` — newest-first timeline.
pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Query(q): Query<AlertsQuery>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let status = q.status.unwrap_or_else(|| "firing".into());
    if status != "firing" && status != "resolved" {
        return Err(Error::BadRequest(
            "status must be 'firing' or 'resolved'".into(),
        ));
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    // Platform alerts (NULL org) are operator business: include them for
    // admins and above.
    let include_platform = user.require(&org_id, Role::Admin).is_ok();
    let alerts: Vec<Alert> = if include_platform {
        sqlx::query_as(
            "SELECT * FROM alerts WHERE (org_id = ? OR org_id IS NULL) AND status = ? \
             ORDER BY fired_at DESC, rowid DESC LIMIT ?",
        )
        .bind(&org_id)
        .bind(&status)
        .bind(limit)
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT * FROM alerts WHERE org_id = ? AND status = ? \
             ORDER BY fired_at DESC, rowid DESC LIMIT ?",
        )
        .bind(&org_id)
        .bind(&status)
        .bind(limit)
        .fetch_all(&state.pool)
        .await?
    };
    Ok(Json(serde_json::json!({ "alerts": alerts })))
}

/// `POST /api/v1/orgs/{org}/alerts/{id}/resolve` — manually resolve.
pub async fn resolve(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Developer).await?;
    // Platform-global alerts are operator business: resolving them needs an
    // admin floor, and other orgs' alerts stay invisible (404).
    let alert: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT org_id, status FROM alerts WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;
    match alert {
        Some((None, status)) if status == "firing" => {
            user.require(&org_id, Role::Admin)?;
        }
        Some((Some(alert_org), status)) if status == "firing" && alert_org == org_id => {}
        _ => return Err(Error::NotFound(format!("alert {id}"))),
    }
    sqlx::query(
        "UPDATE alerts SET status = 'resolved', resolved_at = datetime('now') WHERE id = ?",
    )
    .bind(&id)
    .execute(&state.pool)
    .await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        None,
        "alert.resolve",
        Some("alert"),
        Some(&id),
        None,
    )
    .await?;
    Ok(Json(serde_json::json!({ "id": id, "status": "resolved" })))
}
