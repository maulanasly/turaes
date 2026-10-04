//! Scoped API tokens for programmatic and CI access (no SSH, no browser).
//!
//! Tokens are bound to one organization with one hierarchical scope (`read`,
//! `deploy` or `admin`, mapping to the viewer/developer/admin floors) and can
//! never reach `owner`. Only the SHA-256 hash is stored; the plaintext is
//! returned exactly once at creation.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;

use turaes_core::crypto::{random_token, token_hash};
use turaes_core::models::ApiToken;
use turaes_core::{Error, Result};

use crate::audit;
use crate::authz::{self, CurrentUser, Role};
use crate::state::AppState;

/// Body for minting a token.
#[derive(Debug, Deserialize)]
pub struct CreateToken {
    /// Human name, e.g. `ci-deploy`.
    pub name: String,
    /// Exactly one of `read`, `deploy`, `admin`.
    pub scopes: String,
}

fn validate_scope(scopes: &str) -> Result<()> {
    if matches!(scopes, "read" | "deploy" | "admin") {
        Ok(())
    } else {
        Err(Error::BadRequest(
            "scopes must be one of 'read', 'deploy' or 'admin'".into(),
        ))
    }
}

/// `POST /api/v1/orgs/{org}/tokens` — mint a token; the plaintext is returned
/// exactly once and never stored.
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Json(input): Json<CreateToken>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let name = input.name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(Error::BadRequest("name is required (max 64 chars)".into()));
    }
    validate_scope(&input.scopes)?;
    let plaintext = format!("turaes_{}", random_token(32));
    let hash = token_hash(&plaintext);
    let token = sqlx::query_as::<_, ApiToken>(
        "INSERT INTO api_tokens (id, org_id, user_id, name, token_hash, scopes) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING *",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&org_id)
    .bind(&user.id)
    .bind(name)
    .bind(&hash)
    .bind(&input.scopes)
    .fetch_one(&state.pool)
    .await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        None,
        "token.create",
        Some("api_token"),
        Some(&token.id),
        Some(&serde_json::json!({"name": name, "scopes": input.scopes}).to_string()),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "token": token, "plaintext": plaintext })),
    ))
}

/// `GET /api/v1/orgs/{org}/tokens` — list live tokens (hashes never included).
pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Query(q): Query<super::LimitQuery>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let tokens = sqlx::query_as::<_, ApiToken>(
        "SELECT * FROM api_tokens WHERE org_id = ? AND revoked_at IS NULL \
         ORDER BY created_at DESC LIMIT ?",
    )
    .bind(&org_id)
    .bind(super::LimitQuery::effective(q.limit))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "tokens": tokens })))
}

/// `DELETE /api/v1/orgs/{org}/tokens/{id}` — revoke a token.
pub async fn revoke(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<StatusCode> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let affected =
        sqlx::query("UPDATE api_tokens SET revoked_at = datetime('now') WHERE id = ? AND org_id = ? AND revoked_at IS NULL")
            .bind(&id)
            .bind(&org_id)
            .execute(&state.pool)
            .await?
            .rows_affected();
    if affected == 0 {
        return Err(Error::NotFound(format!("token {id}")));
    }
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        None,
        "token.revoke",
        Some("api_token"),
        Some(&id),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
