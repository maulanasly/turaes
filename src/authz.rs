//! Role-based tenancy: resolving the signed-in principal to a persisted user
//! plus organization memberships, and enforcing per-org role floors. floors.
//!
//! Roles, weakest first: `viewer` (read) › `developer` (deploy, lifecycle,
//! env, domains) › `admin` (apps, servers, tokens) › `owner` (members, org).
//! Bootstrap: the first user ever to sign in becomes `owner` of the `default`
//! organization; later users start as `viewer` until an owner raises them.

use serde::{Deserialize, Serialize};

use turaes_core::{Error, Result};

use crate::state::AppState;

// Only users without memberships take this lock; normal authenticated
// requests never serialize here. It makes first-owner bootstrap atomic so
// concurrent first logins cannot both become owner.
static BOOTSTRAP_MEMBERSHIP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Organization role. Declaration order is the privilege order (`Viewer` is
/// weakest), so `role >= floor` checks work via the derived `Ord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Read-only.
    Viewer,
    /// Deploy, lifecycle, env vars, domains.
    Developer,
    /// Apps, servers, API tokens.
    Admin,
    /// Members and org settings.
    Owner,
}

impl Role {
    /// Parse a stored role string.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "viewer" => Ok(Role::Viewer),
            "developer" => Ok(Role::Developer),
            "admin" => Ok(Role::Admin),
            "owner" => Ok(Role::Owner),
            other => Err(Error::Internal(format!("unknown role '{other}'"))),
        }
    }

    /// Canonical lowercase name.
    // Used by the scoped routes and member management landing in T1b/T1e.
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Developer => "developer",
            Role::Admin => "admin",
            Role::Owner => "owner",
        }
    }
}

/// One organization the current user belongs to, with their role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMembership {
    /// Organization id.
    pub org_id: String,
    /// Organization slug.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// The user's role in this organization.
    pub role: Role,
}

/// The resolved principal for a request: persisted user plus memberships.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    /// `users.id` (UUID).
    pub id: String,
    /// GitHub numeric id (`0` for the dev user).
    pub github_id: i64,
    /// GitHub login.
    pub login: String,
    /// Display name, when known.
    pub name: Option<String>,
    /// Organizations the user belongs to.
    pub orgs: Vec<OrgMembership>,
}

impl CurrentUser {
    /// The user's role in `org_id`, if they are a member.
    // Exercised by tests and the scoped routes landing in T1b.
    #[allow(dead_code)]
    pub fn role_in(&self, org_id: &str) -> Option<Role> {
        self.orgs
            .iter()
            .find(|o| o.org_id == org_id)
            .map(|o| o.role)
    }

    /// Organization ids the user may act in (for `IN (...)` queries).
    // Used by the scoped routes landing in T1b.
    #[allow(dead_code)]
    pub fn org_ids(&self) -> Vec<String> {
        self.orgs.iter().map(|o| o.org_id.clone()).collect()
    }

    /// Enforce a minimum role in `org_id`. Non-members get `Forbidden` (never
    /// leak the org's existence through a different status).
    pub fn require(&self, org_id: &str, floor: Role) -> Result<Role> {
        match self.role_in(org_id) {
            Some(role) if role >= floor => Ok(role),
            _ => Err(Error::Forbidden("insufficient role".into())),
        }
    }
}

/// Upsert the `users` row for a GitHub identity and ensure a bootstrap
/// membership, then return the resolved principal.
pub async fn resolve(
    state: &AppState,
    github_id: i64,
    login: &str,
    name: Option<&str>,
) -> Result<CurrentUser> {
    let id: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE github_id = ?")
        .bind(github_id)
        .fetch_optional(&state.pool)
        .await?;
    let id = match id {
        Some(id) => {
            sqlx::query(
                "UPDATE users SET login = ?, name = ?, last_seen_at = datetime('now') WHERE id = ?",
            )
            .bind(login)
            .bind(name)
            .bind(&id)
            .execute(&state.pool)
            .await?;
            id
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT OR IGNORE INTO users (id, github_id, login, name, last_seen_at) \
                 VALUES (?, ?, ?, ?, datetime('now'))",
            )
            .bind(&id)
            .bind(github_id)
            .bind(login)
            .bind(name)
            .execute(&state.pool)
            .await?;
            // Another request for the same GitHub identity may have won the
            // unique-key race; always use the canonical row id.
            sqlx::query_scalar("SELECT id FROM users WHERE github_id = ?")
                .bind(github_id)
                .fetch_one(&state.pool)
                .await?
        }
    };

    ensure_bootstrap_membership(state, &id).await?;
    let orgs = memberships(state, &id).await?;
    Ok(CurrentUser {
        id,
        github_id,
        login: login.to_string(),
        name: name.map(str::to_string),
        orgs,
    })
}

/// Bootstrap rule for a user with no memberships: the very first user becomes
/// `owner` of `default`; everyone after starts as `viewer`.
async fn ensure_bootstrap_membership(state: &AppState, user_id: &str) -> Result<()> {
    let owned: i64 = sqlx::query_scalar("SELECT count(*) FROM memberships WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(&state.pool)
        .await?;
    if owned > 0 {
        return Ok(());
    }
    let _guard = BOOTSTRAP_MEMBERSHIP_LOCK.lock().await;
    // A concurrent request for this same user may have populated membership
    // while this request waited.
    let owned: i64 = sqlx::query_scalar("SELECT count(*) FROM memberships WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(&state.pool)
        .await?;
    if owned > 0 {
        return Ok(());
    }
    let default_members: i64 =
        sqlx::query_scalar("SELECT count(*) FROM memberships WHERE org_id = 'default'")
            .fetch_one(&state.pool)
            .await?;
    let role = if default_members == 0 {
        "owner"
    } else {
        "viewer"
    };
    sqlx::query(
        "INSERT OR IGNORE INTO memberships (id, org_id, user_id, role) \
         VALUES (?, 'default', ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(user_id)
    .bind(role)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Resolve a bearer API token to a principal restricted to the token's org.
/// The effective role is capped by both the token scope and the creator's
/// *current* membership, so demoting or removing the creator attenuates the
/// token. Revoked tokens, unknown hashes and ownerless tokens are rejected.
/// Tokens can never reach `owner`: member management stays human-only.
pub async fn resolve_token(state: &AppState, token: &str) -> Result<CurrentUser> {
    let hash = turaes_core::crypto::token_hash(token);
    let row: Option<(String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, org_id, user_id, scopes FROM api_tokens \
         WHERE token_hash = ? AND revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await?;
    let (token_id, org_id, user_id, scopes) =
        row.ok_or_else(|| Error::Unauthorized("invalid API token".into()))?;
    let cap = match scopes.as_str() {
        "read" => Role::Viewer,
        "deploy" => Role::Developer,
        "admin" => Role::Admin,
        _ => return Err(Error::Unauthorized("API token has an unknown scope".into())),
    };
    let user_id = user_id.ok_or_else(|| Error::Unauthorized("API token owner is gone".into()))?;
    let user: Option<(String, i64, String, Option<String>)> =
        sqlx::query_as("SELECT id, github_id, login, name FROM users WHERE id = ?")
            .bind(&user_id)
            .fetch_optional(&state.pool)
            .await?;
    let (uid, github_id, login, name) =
        user.ok_or_else(|| Error::Unauthorized("API token owner is gone".into()))?;
    let mem: Option<(String, String, String)> = sqlx::query_as(
        "SELECT m.role, o.slug, o.name FROM memberships m \
         JOIN organizations o ON o.id = m.org_id \
         WHERE m.user_id = ? AND m.org_id = ?",
    )
    .bind(&user_id)
    .bind(&org_id)
    .fetch_optional(&state.pool)
    .await?;
    let (role_str, slug, org_name) =
        mem.ok_or_else(|| Error::Unauthorized("API token owner left the organization".into()))?;
    let role = std::cmp::min(cap, Role::parse(&role_str)?);
    // Throttled touch: per-request writes contend with monitor ticks on the
    // single-writer SQLite file, so only record uses older than five minutes.
    sqlx::query(
        "UPDATE api_tokens SET last_used_at = datetime('now') WHERE id = ? \
         AND (last_used_at IS NULL OR last_used_at < datetime('now', '-5 minutes'))",
    )
    .bind(&token_id)
    .execute(&state.pool)
    .await?;
    Ok(CurrentUser {
        id: uid,
        github_id,
        login,
        name,
        orgs: vec![OrgMembership {
            org_id,
            slug,
            name: org_name,
            role,
        }],
    })
}

/// Resolve an org path segment (id or slug) and enforce a role floor.
///
/// Unknown orgs and orgs the caller is not a member of both yield
/// `NotFound`, so slugs cannot be enumerated by status code; members below
/// the floor still get `Forbidden`.
pub async fn authorize_org(
    state: &AppState,
    user: &CurrentUser,
    org_ref: &str,
    floor: Role,
) -> Result<String> {
    let org_id: Option<String> =
        sqlx::query_scalar("SELECT id FROM organizations WHERE id = ? OR slug = ?")
            .bind(org_ref)
            .bind(org_ref)
            .fetch_optional(&state.pool)
            .await?;
    match org_id {
        Some(id) => {
            if user.role_in(&id).is_none() {
                return Err(Error::NotFound(format!("organization {org_ref}")));
            }
            user.require(&id, floor)?;
            Ok(id)
        }
        None => Err(Error::NotFound(format!("organization {org_ref}"))),
    }
}

/// Fleet management (servers) is platform-global: it needs an operator, i.e.
/// `admin` or above in any organization. Node-level RBAC is a later step.
pub fn require_operator(user: &CurrentUser) -> Result<()> {
    if user.orgs.iter().any(|o| o.role >= Role::Admin) {
        Ok(())
    } else {
        Err(Error::Forbidden("operator role required".into()))
    }
}

/// Load a user's memberships with organization details.
async fn memberships(state: &AppState, user_id: &str) -> Result<Vec<OrgMembership>> {
    let rows = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT m.org_id, o.slug, o.name, m.role FROM memberships m \
         JOIN organizations o ON o.id = m.org_id \
         WHERE m.user_id = ? ORDER BY o.slug",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (org_id, slug, name, role) in rows {
        out.push(OrgMembership {
            org_id,
            slug,
            name,
            role: Role::parse(&role)?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_ordering_matches_privilege() {
        assert!(Role::Owner > Role::Admin);
        assert!(Role::Admin > Role::Developer);
        assert!(Role::Developer > Role::Viewer);
    }

    #[test]
    fn role_round_trips() {
        for role in [Role::Viewer, Role::Developer, Role::Admin, Role::Owner] {
            assert_eq!(Role::parse(role.as_str()).unwrap(), role);
        }
        assert!(Role::parse("root").is_err());
    }

    #[test]
    fn require_enforces_floor() {
        let user = CurrentUser {
            id: "u".into(),
            github_id: 1,
            login: "a".into(),
            name: None,
            orgs: vec![OrgMembership {
                org_id: "o".into(),
                slug: "o".into(),
                name: "o".into(),
                role: Role::Developer,
            }],
        };
        assert_eq!(user.require("o", Role::Viewer).unwrap(), Role::Developer);
        assert_eq!(user.require("o", Role::Developer).unwrap(), Role::Developer);
        assert!(user.require("o", Role::Admin).is_err());
        assert!(user.require("other", Role::Viewer).is_err());
    }
}
