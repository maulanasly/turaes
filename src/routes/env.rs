//! Per-application environment variables (values sealed at rest, never returned).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use turaes_core::{Error, Result};

use crate::routes::apps::fetch_app;
use crate::state::AppState;

/// Body for setting a variable.
#[derive(Debug, Deserialize)]
pub struct PutEnv {
    /// The value (encrypted before storage).
    pub value: String,
}

fn validate_key(key: &str) -> Result<()> {
    let valid = !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .enumerate()
            .all(|(i, c)| c.is_ascii_alphabetic() || c == '_' || (i > 0 && c.is_ascii_digit()));
    if valid {
        Ok(())
    } else {
        Err(Error::BadRequest(
            "key must match [A-Za-z_][A-Za-z0-9_]*".into(),
        ))
    }
}

/// `GET /api/v1/apps/{id}/env` — list keys only (values are never returned).
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    fetch_app(&state.pool, &id).await?;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, created_at FROM env_vars WHERE application_id = ? ORDER BY key ASC",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await?;
    let keys: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(key, created_at)| serde_json::json!({ "key": key, "created_at": created_at }))
        .collect();
    Ok(Json(serde_json::json!({ "env": keys })))
}

/// `PUT /api/v1/apps/{id}/env/{key}` — create or replace a variable.
pub async fn put(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
    Json(body): Json<PutEnv>,
) -> Result<StatusCode> {
    validate_key(&key)?;
    fetch_app(&state.pool, &id).await?;
    let sealed = state.secrets.seal(&body.value)?;
    sqlx::query(
        "INSERT INTO env_vars (id, application_id, key, value_enc) VALUES (?, ?, ?, ?) \
         ON CONFLICT(application_id, key) DO UPDATE SET value_enc = excluded.value_enc",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&id)
    .bind(&key)
    .bind(&sealed)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/apps/{id}/env/{key}`
pub async fn delete(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
) -> Result<StatusCode> {
    fetch_app(&state.pool, &id).await?;
    let affected = sqlx::query("DELETE FROM env_vars WHERE application_id = ? AND key = ?")
        .bind(&id)
        .bind(&key)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(Error::NotFound(format!("env var {key}")));
    }
    Ok(StatusCode::NO_CONTENT)
}
