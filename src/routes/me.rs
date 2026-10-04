//! `GET /api/v1/me` — the resolved tenant principal: user, organizations
//! and roles. Handlers and the UI scope everything else off `orgs`.

use axum::Extension;
use axum::Json;
use serde::Serialize;

use turaes_core::Result;

use crate::authz::{CurrentUser, OrgMembership};

/// Response shape for `GET /api/v1/me`.
#[derive(Debug, Serialize)]
pub struct MeResponse {
    /// `users.id` (UUID).
    pub id: String,
    /// GitHub numeric id.
    pub github_id: i64,
    /// GitHub login.
    pub login: String,
    /// Display name, when known.
    pub name: Option<String>,
    /// Organizations the user belongs to, with roles.
    pub orgs: Vec<OrgMembership>,
}

/// Return the resolved principal attached by [`crate::auth::require_auth`].
pub async fn me(Extension(user): Extension<CurrentUser>) -> Result<Json<MeResponse>> {
    Ok(Json(MeResponse {
        id: user.id.clone(),
        github_id: user.github_id,
        login: user.login.clone(),
        name: user.name.clone(),
        orgs: user.orgs.clone(),
    }))
}
