//! Domain aliases for an application (the primary lives on `applications.domain`).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;

use turaes_core::models::Domain;
use turaes_core::{Error, Result};

use crate::audit;
use crate::authz::{self, CurrentUser, Role};
use crate::routes::apps::{fetch_org_app, refresh_proxy_routes};
use crate::state::AppState;

/// Body for adding an alias.
#[derive(Debug, Deserialize)]
pub struct AddDomain {
    /// FQDN, e.g. `www.example.com`.
    pub domain: String,
}

fn validate_domain(domain: &str) -> Result<String> {
    let d = domain.trim().to_ascii_lowercase();
    let ok = !d.is_empty()
        && d.len() <= 253
        && d.contains('.')
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && !d.starts_with('.')
        && !d.ends_with('.');
    if ok {
        Ok(d)
    } else {
        Err(Error::BadRequest("invalid domain".into()))
    }
}

/// `GET /api/v1/orgs/{org}/apps/{id}/domains`
pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    fetch_org_app(&state.pool, &org_id, &id).await?;
    let domains = sqlx::query_as::<_, Domain>(
        "SELECT * FROM domains WHERE application_id = ? ORDER BY domain ASC",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "domains": domains })))
}

/// `POST /api/v1/orgs/{org}/apps/{id}/domains`
pub async fn add(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
    Json(input): Json<AddDomain>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Developer).await?;
    fetch_org_app(&state.pool, &org_id, &id).await?;
    let domain = validate_domain(&input.domain)?;
    let row = sqlx::query_as::<_, Domain>(
        "INSERT INTO domains (id, application_id, domain, is_primary) \
         VALUES (?, ?, ?, 0) RETURNING *",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&id)
    .bind(&domain)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db) = &e {
            if db.message().contains("UNIQUE") {
                return Error::Conflict(format!("domain '{domain}' is already in use"));
            }
        }
        Error::Db(e)
    })?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&id),
        "domain.add",
        Some("domain"),
        Some(&row.id),
        Some(&serde_json::json!({"domain": domain}).to_string()),
    )
    .await?;

    let _ = refresh_proxy_routes(&state).await;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "domain": row })),
    ))
}

/// `DELETE /api/v1/orgs/{org}/apps/{id}/domains/{domain}`
pub async fn delete(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id, domain)): Path<(String, String, String)>,
) -> Result<StatusCode> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Developer).await?;
    fetch_org_app(&state.pool, &org_id, &id).await?;
    let affected = sqlx::query("DELETE FROM domains WHERE application_id = ? AND domain = ?")
        .bind(&id)
        .bind(domain.to_ascii_lowercase())
        .execute(&state.pool)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(Error::NotFound(format!("domain {domain}")));
    }
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&id),
        "domain.remove",
        Some("domain"),
        None,
        Some(&serde_json::json!({"domain": domain.to_ascii_lowercase()}).to_string()),
    )
    .await?;
    let _ = refresh_proxy_routes(&state).await;
    Ok(StatusCode::NO_CONTENT)
}
