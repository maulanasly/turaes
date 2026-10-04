//! Durable audit log writes for mutating actions.
//!
//! Every mutating HTTP handler records who did what, in which org, to which
//! target. Writes are strict (failures propagate): an action the platform
//! cannot account for should surface loudly, not vanish silently. Metadata is
//! operator context only — never secrets or env values.

use turaes_core::Result;

use crate::authz::CurrentUser;
use crate::state::AppState;

/// Record one audit row. `org_id` is `None` for platform-global actions
/// (fleet management); `actor` is `None` for system/CLI actions.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    state: &AppState,
    org_id: Option<&str>,
    actor: Option<&CurrentUser>,
    application_id: Option<&str>,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    metadata: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_log \
         (id, org_id, actor_user_id, application_id, action, target_type, target_id, metadata) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(org_id)
    .bind(actor.map(|u| u.id.clone()))
    .bind(application_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(metadata)
    .execute(&state.pool)
    .await?;
    Ok(())
}
