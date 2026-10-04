//! Organizations and membership management.
//!
//! Any signed-in user may create an organization (they become its owner).
//! Managing members requires `owner`; listing members requires `viewer`.
//! The last owner of an org can neither be demoted nor removed.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;

use turaes_core::models::Organization;
use turaes_core::{Error, Result};

use crate::audit;
use crate::authz::{self, CurrentUser, Role};
use crate::state::AppState;

/// Body for creating an organization.
#[derive(Debug, Deserialize)]
pub struct CreateOrg {
    /// URL-safe unique slug (`default` is taken).
    pub slug: String,
    /// Display name (defaults to the slug).
    pub name: Option<String>,
}

/// Body for inviting a member: either a GitHub login (resolved via the GitHub
/// API) or a raw numeric GitHub user id, plus the starting role.
#[derive(Debug, Deserialize)]
pub struct InviteMember {
    /// GitHub login, e.g. `octocat`.
    pub login: Option<String>,
    /// Numeric GitHub user id (used when `login` is absent).
    pub github_id: Option<i64>,
    /// `viewer`, `developer`, `admin` or `owner` (default `viewer`).
    pub role: Option<String>,
}

/// Body for changing a member's role.
#[derive(Debug, Deserialize)]
pub struct ChangeRole {
    /// New role.
    pub role: String,
}

/// One membership row with the member's identity resolved.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct MemberEntry {
    /// `users.id` (UUID).
    pub user_id: String,
    /// Numeric GitHub user id.
    pub github_id: i64,
    /// GitHub login.
    pub login: String,
    /// Display name, when known.
    pub name: Option<String>,
    /// `owner`, `admin`, `developer` or `viewer`.
    pub role: String,
    /// Membership creation timestamp.
    pub created_at: String,
}

fn validate_slug(slug: &str) -> Result<String> {
    let s = slug.trim().to_ascii_lowercase();
    let valid = !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-');
    if valid {
        Ok(s)
    } else {
        Err(Error::BadRequest(
            "slug must be a lowercase slug of a-z, 0-9 and '-' (max 32 chars)".into(),
        ))
    }
}

fn validate_role(role: Option<&str>) -> Result<Role> {
    match role.unwrap_or("viewer") {
        "viewer" => Ok(Role::Viewer),
        "developer" => Ok(Role::Developer),
        "admin" => Ok(Role::Admin),
        "owner" => Ok(Role::Owner),
        other => Err(Error::BadRequest(format!(
            "role must be one of viewer, developer, admin, owner (got '{other}')"
        ))),
    }
}

/// `GET /api/v1/orgs` — organizations the caller belongs to, with roles.
pub async fn list(
    State(_state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "organizations": user.orgs })))
}

/// `POST /api/v1/orgs` — create an organization; the caller becomes `owner`.
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(input): Json<CreateOrg>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let slug = validate_slug(&input.slug)?;
    let name = input
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| slug.clone());
    let id = uuid::Uuid::new_v4().to_string();
    let org = sqlx::query_as::<_, Organization>(
        "INSERT INTO organizations (id, slug, name) VALUES (?, ?, ?) RETURNING *",
    )
    .bind(&id)
    .bind(&slug)
    .bind(&name)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db) = &e {
            if db.message().contains("UNIQUE") {
                return Error::Conflict(format!("organization '{slug}' already exists"));
            }
        }
        Error::Db(e)
    })?;
    sqlx::query("INSERT INTO memberships (id, org_id, user_id, role) VALUES (?, ?, ?, 'owner')")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&id)
        .bind(&user.id)
        .execute(&state.pool)
        .await?;
    audit::record(
        &state,
        Some(&id),
        Some(&user),
        None,
        "org.create",
        Some("organization"),
        Some(&id),
        Some(&serde_json::json!({"slug": slug}).to_string()),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "organization": org })),
    ))
}

/// `GET /api/v1/orgs/{org}/members` — roster with roles.
pub async fn members(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let rows: Vec<MemberEntry> = sqlx::query_as(
        "SELECT u.id AS user_id, u.github_id, u.login, u.name, m.role, m.created_at \
         FROM memberships m JOIN users u ON u.id = m.user_id \
         WHERE m.org_id = ? ORDER BY m.created_at ASC",
    )
    .bind(&org_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "members": rows })))
}

/// Resolve an invite target to `(github_id, login, name)`: a raw id is used
/// as-is, while a login is resolved through the public GitHub users API.
async fn resolve_invitee(
    state: &AppState,
    input: &InviteMember,
) -> Result<(i64, String, Option<String>)> {
    if let Some(login) = input
        .login
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        #[derive(Deserialize)]
        struct GhUser {
            id: i64,
            login: String,
            name: Option<String>,
        }
        let resp = state
            .http
            .get(format!("https://api.github.com/users/{login}"))
            .header(axum::http::header::USER_AGENT, "turaes")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|_| Error::Internal("could not reach GitHub to resolve the login".into()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::BadRequest(format!(
                "GitHub user '{login}' not found"
            )));
        }
        if !resp.status().is_success() {
            return Err(Error::Internal("GitHub user lookup failed".into()));
        }
        let gh: GhUser = resp
            .json()
            .await
            .map_err(|_| Error::Internal("could not read GitHub's response".into()))?;
        return Ok((gh.id, gh.login, gh.name));
    }
    match input.github_id {
        Some(id) if id > 0 => Ok((id, format!("github-{id}"), None)),
        _ => Err(Error::BadRequest(
            "invite needs a GitHub login or a numeric github_id".into(),
        )),
    }
}

/// `POST /api/v1/orgs/{org}/members` — invite by GitHub login (or raw id).
pub async fn invite(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Json(input): Json<InviteMember>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Owner).await?;
    let role = validate_role(input.role.as_deref())?;
    let (github_id, login, name) = resolve_invitee(&state, &input).await?;
    let user_id: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE github_id = ?")
        .bind(github_id)
        .fetch_optional(&state.pool)
        .await?;
    let user_id = match user_id {
        Some(id) => {
            sqlx::query("UPDATE users SET login = ?, name = ? WHERE id = ?")
                .bind(&login)
                .bind(&name)
                .bind(&id)
                .execute(&state.pool)
                .await?;
            id
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query("INSERT INTO users (id, github_id, login, name) VALUES (?, ?, ?, ?)")
                .bind(&id)
                .bind(github_id)
                .bind(&login)
                .bind(&name)
                .execute(&state.pool)
                .await?;
            id
        }
    };
    sqlx::query("INSERT INTO memberships (id, org_id, user_id, role) VALUES (?, ?, ?, ?)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&org_id)
        .bind(&user_id)
        .bind(role.as_str())
        .execute(&state.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(db) = &e {
                if db.message().contains("UNIQUE") {
                    return Error::Conflict("that user is already a member".into());
                }
            }
            Error::Db(e)
        })?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        None,
        "member.add",
        Some("membership"),
        Some(&user_id),
        Some(&serde_json::json!({"login": login, "role": role.as_str()}).to_string()),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "user_id": user_id, "role": role.as_str() })),
    ))
}

async fn owner_count(state: &AppState, org_id: &str) -> Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM memberships WHERE org_id = ? AND role = 'owner'")
        .bind(org_id)
        .fetch_one(&state.pool)
        .await
        .map_err(Error::Db)
}

/// `PATCH /api/v1/orgs/{org}/members/{user_id}` — change a member's role.
pub async fn change_role(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, member_id)): Path<(String, String)>,
    Json(input): Json<ChangeRole>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Owner).await?;
    let role = validate_role(Some(&input.role))?;
    let current: Option<String> =
        sqlx::query_scalar("SELECT role FROM memberships WHERE org_id = ? AND user_id = ?")
            .bind(&org_id)
            .bind(&member_id)
            .fetch_optional(&state.pool)
            .await?;
    let current = current.ok_or_else(|| Error::NotFound("membership not found".into()))?;
    if current == "owner" && role != Role::Owner && owner_count(&state, &org_id).await? <= 1 {
        return Err(Error::Conflict(
            "cannot demote the last owner of the organization".into(),
        ));
    }
    sqlx::query("UPDATE memberships SET role = ? WHERE org_id = ? AND user_id = ?")
        .bind(role.as_str())
        .bind(&org_id)
        .bind(&member_id)
        .execute(&state.pool)
        .await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        None,
        "member.change",
        Some("membership"),
        Some(&member_id),
        Some(&serde_json::json!({"from": current, "to": role.as_str()}).to_string()),
    )
    .await?;
    Ok(Json(
        serde_json::json!({ "user_id": member_id, "role": role.as_str() }),
    ))
}

/// `DELETE /api/v1/orgs/{org}/members/{user_id}` — remove a member.
pub async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, member_id)): Path<(String, String)>,
) -> Result<StatusCode> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Owner).await?;
    let current: Option<String> =
        sqlx::query_scalar("SELECT role FROM memberships WHERE org_id = ? AND user_id = ?")
            .bind(&org_id)
            .bind(&member_id)
            .fetch_optional(&state.pool)
            .await?;
    match current.as_deref() {
        None => return Err(Error::NotFound("membership not found".into())),
        Some("owner") if owner_count(&state, &org_id).await? <= 1 => {
            return Err(Error::Conflict(
                "cannot remove the last owner of the organization".into(),
            ))
        }
        _ => {}
    }
    sqlx::query("DELETE FROM memberships WHERE org_id = ? AND user_id = ?")
        .bind(&org_id)
        .bind(&member_id)
        .execute(&state.pool)
        .await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        None,
        "member.remove",
        Some("membership"),
        Some(&member_id),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
