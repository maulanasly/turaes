//! Secret rotation tooling: re-seal every stored value with the primary cipher.
//!
//! Run once after upgrading (migrates pre-HKDF legacy blobs to `v1$`), and
//! again as the second half of a rotation: point `TURAES_JWT_SECRET` at the new
//! secret, keep the old one in `TURAES_SECRET_PREVIOUS` so stored values still
//! decrypt, run `turaes secrets reseal`, verify apps still deploy, then unset
//! `TURAES_SECRET_PREVIOUS`. Undecryptable rows abort the run loudly — nothing
//! is half-migrated silently.

use turaes_core::crypto::SecretBox;
use turaes_core::{Error, Result};

use crate::audit;
use crate::state::AppState;

/// Re-seal stored secrets. Without a previous secret configured, only
/// legacy-format values are rewritten; with one, every value moves to the new
/// primary. Returns `(env_updated, env_total, ssh_updated, ssh_total)`.
pub async fn reseal(state: &AppState) -> Result<(usize, usize, usize, usize)> {
    let rewrite_all = !state.cfg.auth.secret_previous.is_empty();

    let env_rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, value_enc FROM env_vars ORDER BY id")
            .fetch_all(&state.pool)
            .await?;
    let mut env_updated = 0;
    for (id, sealed) in &env_rows {
        if !rewrite_all && !SecretBox::is_legacy_format(sealed) {
            continue;
        }
        let plain = state.secrets.open(sealed).map_err(|e| {
            Error::Internal(format!("env var {id} is undecryptable, aborting: {e}"))
        })?;
        let fresh = state.secrets.seal(&plain)?;
        sqlx::query("UPDATE env_vars SET value_enc = ? WHERE id = ?")
            .bind(&fresh)
            .bind(id)
            .execute(&state.pool)
            .await?;
        env_updated += 1;
    }

    let ssh_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, ssh_key_enc FROM servers WHERE ssh_key_enc IS NOT NULL ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut ssh_updated = 0;
    for (id, sealed) in &ssh_rows {
        if !rewrite_all && !SecretBox::is_legacy_format(sealed) {
            continue;
        }
        let plain = state.secrets.open(sealed).map_err(|e| {
            Error::Internal(format!("server {id} key is undecryptable, aborting: {e}"))
        })?;
        let fresh = state.secrets.seal(&plain)?;
        sqlx::query("UPDATE servers SET ssh_key_enc = ? WHERE id = ?")
            .bind(&fresh)
            .bind(id)
            .execute(&state.pool)
            .await?;
        ssh_updated += 1;
    }

    let meta =
        serde_json::json!({"env_updated": env_updated, "ssh_updated": ssh_updated}).to_string();
    audit::record(
        state,
        None,
        None,
        None,
        "secrets.reseal",
        Some("secret"),
        None,
        Some(&meta),
    )
    .await?;

    Ok((env_updated, env_rows.len(), ssh_updated, ssh_rows.len()))
}
