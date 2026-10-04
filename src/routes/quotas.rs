//! Per-organization resource quotas.
//!
//! Quotas bound what an org may *claim*: app count, summed memory/CPU limits
//! and hostname count. Memory/CPU accounting sums explicit limits only —
//! unlimited apps count toward `max_apps` but not the budgets.

use axum::extract::{Path, State};
use axum::{Extension, Json};
use serde::Deserialize;

use turaes_core::db::Pool;
use turaes_core::models::OrgQuota;
use turaes_core::{Error, Result};

use crate::authz::{self, CurrentUser, Role};
use crate::state::AppState;

/// Body for replacing quota ceilings (all fields optional; omitted keeps).
#[derive(Debug, Deserialize)]
pub struct QuotaBody {
    /// Maximum applications.
    pub max_apps: Option<i64>,
    /// Maximum summed `mem_limit_mb`.
    pub max_mem_mb: Option<i64>,
    /// Maximum summed `cpu_quota_pct`.
    pub max_cpu_pct: Option<i64>,
    /// Maximum hostnames (primary domains + aliases).
    pub max_domains: Option<i64>,
}

/// Current consumption of an org's budgets.
#[derive(Debug, serde::Serialize)]
pub struct QuotaUsage {
    /// Applications owned.
    pub apps: i64,
    /// Summed `mem_limit_mb` of limited apps.
    pub mem_mb: i64,
    /// Summed `cpu_quota_pct` of limited apps.
    pub cpu_pct: i64,
    /// Hostnames claimed (primaries + aliases).
    pub domains: i64,
}

fn validate_body(input: &QuotaBody) -> Result<()> {
    for (name, v) in [
        ("max_apps", input.max_apps),
        ("max_mem_mb", input.max_mem_mb),
        ("max_cpu_pct", input.max_cpu_pct),
        ("max_domains", input.max_domains),
    ] {
        if let Some(n) = v {
            if n < 0 {
                return Err(Error::BadRequest(format!("{name} must be >= 0")));
            }
        }
    }
    Ok(())
}

/// Fetch the org's quota row, seeding defaults on first use.
pub(crate) async fn quota_row(pool: &Pool, org_id: &str) -> Result<OrgQuota> {
    sqlx::query("INSERT OR IGNORE INTO org_quotas (org_id) VALUES (?)")
        .bind(org_id)
        .execute(pool)
        .await?;
    sqlx::query_as::<_, OrgQuota>("SELECT * FROM org_quotas WHERE org_id = ?")
        .bind(org_id)
        .fetch_one(pool)
        .await
        .map_err(Error::Db)
}

/// Measure current consumption of the org's budgets.
pub(crate) async fn usage(pool: &Pool, org_id: &str) -> Result<QuotaUsage> {
    let apps: i64 = sqlx::query_scalar("SELECT count(*) FROM applications WHERE org_id = ?")
        .bind(org_id)
        .fetch_one(pool)
        .await?;
    let mem_mb: Option<i64> =
        sqlx::query_scalar("SELECT SUM(mem_limit_mb) FROM applications WHERE org_id = ?")
            .bind(org_id)
            .fetch_one(pool)
            .await?;
    let cpu_pct: Option<i64> =
        sqlx::query_scalar("SELECT SUM(cpu_quota_pct) FROM applications WHERE org_id = ?")
            .bind(org_id)
            .fetch_one(pool)
            .await?;
    let primaries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM applications WHERE org_id = ? AND domain IS NOT NULL AND domain != ''",
    )
    .bind(org_id)
    .fetch_one(pool)
    .await?;
    let aliases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM domains d JOIN applications a ON a.id = d.application_id \
         WHERE a.org_id = ?",
    )
    .bind(org_id)
    .fetch_one(pool)
    .await?;
    Ok(QuotaUsage {
        apps,
        mem_mb: mem_mb.unwrap_or(0),
        cpu_pct: cpu_pct.unwrap_or(0),
        domains: primaries + aliases,
    })
}

/// Refuse an app create/update that would exceed the org's budgets.
/// `exclude_app_id` is the app being updated ("" on create).
pub(crate) async fn ensure_capacity(
    pool: &Pool,
    org_id: &str,
    exclude_app_id: &str,
    mem_mb: Option<i64>,
    cpu_pct: Option<i64>,
) -> Result<()> {
    let quota = quota_row(pool, org_id).await?;
    let apps: i64 =
        sqlx::query_scalar("SELECT count(*) FROM applications WHERE org_id = ? AND id != ?")
            .bind(org_id)
            .bind(exclude_app_id)
            .fetch_one(pool)
            .await?;
    if apps + 1 > quota.max_apps {
        return Err(Error::Conflict(format!(
            "application quota exceeded ({}/{})",
            apps, quota.max_apps
        )));
    }
    let used_mem: Option<i64> = sqlx::query_scalar(
        "SELECT SUM(mem_limit_mb) FROM applications WHERE org_id = ? AND id != ?",
    )
    .bind(org_id)
    .bind(exclude_app_id)
    .fetch_one(pool)
    .await?;
    if used_mem.unwrap_or(0) + mem_mb.unwrap_or(0) > quota.max_mem_mb {
        return Err(Error::Conflict(format!(
            "memory quota exceeded ({}MB/{})",
            used_mem.unwrap_or(0),
            quota.max_mem_mb
        )));
    }
    let used_cpu: Option<i64> = sqlx::query_scalar(
        "SELECT SUM(cpu_quota_pct) FROM applications WHERE org_id = ? AND id != ?",
    )
    .bind(org_id)
    .bind(exclude_app_id)
    .fetch_one(pool)
    .await?;
    if used_cpu.unwrap_or(0) + cpu_pct.unwrap_or(0) > quota.max_cpu_pct {
        return Err(Error::Conflict(format!(
            "CPU quota exceeded ({}%/{}%)",
            used_cpu.unwrap_or(0),
            quota.max_cpu_pct
        )));
    }
    Ok(())
}

/// Refuse a domain claim that would exceed the org's hostname budget.
pub(crate) async fn ensure_domain_capacity(pool: &Pool, org_id: &str) -> Result<()> {
    let quota = quota_row(pool, org_id).await?;
    let used = usage(pool, org_id).await?;
    if used.domains + 1 > quota.max_domains {
        return Err(Error::Conflict(format!(
            "domain quota exceeded ({}/{})",
            used.domains, quota.max_domains
        )));
    }
    Ok(())
}

/// `GET /api/v1/orgs/{org}/quota` — ceilings plus current usage.
pub async fn get(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let quota = quota_row(&state.pool, &org_id).await?;
    let used = usage(&state.pool, &org_id).await?;
    Ok(Json(serde_json::json!({ "quota": quota, "usage": used })))
}

/// `PUT /api/v1/orgs/{org}/quota` — replace ceilings (owner only).
pub async fn put(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Json(input): Json<QuotaBody>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Owner).await?;
    validate_body(&input)?;
    let mut quota = quota_row(&state.pool, &org_id).await?;
    if let Some(v) = input.max_apps {
        quota.max_apps = v;
    }
    if let Some(v) = input.max_mem_mb {
        quota.max_mem_mb = v;
    }
    if let Some(v) = input.max_cpu_pct {
        quota.max_cpu_pct = v;
    }
    if let Some(v) = input.max_domains {
        quota.max_domains = v;
    }
    sqlx::query(
        "UPDATE org_quotas SET max_apps = ?, max_mem_mb = ?, max_cpu_pct = ?, max_domains = ? \
         WHERE org_id = ?",
    )
    .bind(quota.max_apps)
    .bind(quota.max_mem_mb)
    .bind(quota.max_cpu_pct)
    .bind(quota.max_domains)
    .bind(&org_id)
    .execute(&state.pool)
    .await?;
    let used = usage(&state.pool, &org_id).await?;
    Ok(Json(serde_json::json!({ "quota": quota, "usage": used })))
}
