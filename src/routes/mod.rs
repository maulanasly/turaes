//! HTTP route modules.

use serde::Deserialize;

/// Shared `?limit` for list endpoints: optional client cap, max 200 rows.
/// Absent means "everything" (current dashboard behavior); callers that page
/// should always send it.
#[derive(Debug, Deserialize)]
pub struct LimitQuery {
    /// Max rows (clamped to 1..=200 when present).
    pub limit: Option<i64>,
}

impl LimitQuery {
    /// Effective `LIMIT` value: the client cap clamped to 1..=200, or all rows.
    pub fn effective(limit: Option<i64>) -> i64 {
        limit.map(|l| l.clamp(1, 200)).unwrap_or(i64::MAX)
    }
}

pub mod agent;
pub mod alerts;
pub mod app_validation;
pub mod apps;
pub mod artifacts;
pub mod audit;
pub mod catalog;
pub mod deployments;
pub mod domains;
pub mod env;
pub mod health;
pub mod logs;
pub mod me;
pub mod orgs;
pub mod quotas;
pub mod servers;
pub mod static_assets;
pub mod tokens;
