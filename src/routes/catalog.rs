//! Catalog: curated starter templates plus platform version.
//!
//! Templates are platform-global (the same rows for every org); the endpoint
//! is org-scoped only so membership is enforced like every other read.
//! P1 will add the per-app releases roll-up to this response.

use axum::extract::{Path, State};
use axum::{Extension, Json};

use turaes_core::models::CatalogTemplate;
use turaes_core::{Error, Result};

use crate::authz::{self, CurrentUser, Role};
use crate::state::AppState;

/// `GET /api/v1/orgs/{org}/catalog` — starter templates + platform version.
pub async fn get(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let _org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let templates = sqlx::query_as::<_, CatalogTemplate>(
        "SELECT * FROM catalog_templates ORDER BY sort_order ASC, slug ASC",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(Error::Db)?;
    let templates: Vec<serde_json::Value> = templates
        .into_iter()
        .map(|t| {
            let defaults: serde_json::Value =
                serde_json::from_str(&t.defaults_json).unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "slug": t.slug,
                "name": t.name,
                "description": t.description,
                "kind": t.kind,
                "defaults": defaults,
                "sort_order": t.sort_order,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "templates": templates,
        "platform": { "version": env!("CARGO_PKG_VERSION") },
    })))
}
