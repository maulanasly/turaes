//! Alert rules evaluated on every monitor tick, plus webhook dispatch.
//!
//! Rules are stateless queries; the `alerts` table is the state. Firing rows
//! are deduplicated by `key` (one firing row per key, ever), so evaluation is
//! idempotent and cheap. Notifications go to the configured webhook once on
//! fire and once on resolve; with no webhook configured, alerts are still
//! recorded for the dashboard banner.

use std::time::{Duration, SystemTime};

use crate::state::AppState;

/// Fire `key` unless already firing, then notify.
#[allow(clippy::too_many_arguments)]
async fn fire(
    state: &AppState,
    org_id: Option<&str>,
    severity: &str,
    kind: &str,
    key: &str,
    subject: &str,
    detail: Option<&str>,
    application_id: Option<&str>,
) {
    let existing: Option<String> =
        match sqlx::query_scalar("SELECT id FROM alerts WHERE key = ? AND status = 'firing'")
            .bind(key)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(row) => row,
            Err(e) => {
                tracing::warn!(error = %e, "alert lookup failed");
                return;
            }
        };
    if existing.is_some() {
        return;
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = sqlx::query(
        "INSERT INTO alerts \
         (id, org_id, severity, kind, key, subject, detail, status, application_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'firing', ?)",
    )
    .bind(&id)
    .bind(org_id)
    .bind(severity)
    .bind(kind)
    .bind(key)
    .bind(subject)
    .bind(detail)
    .bind(application_id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(error = %e, "alert insert failed");
        return;
    }
    tracing::warn!(kind, subject, "alert firing");
    dispatch(state, &id, severity, kind, "firing", subject, detail).await;
}

/// Resolve the firing row for `key`, notifying if it ever dispatched.
async fn resolve(state: &AppState, key: &str) {
    let row: Option<(String, Option<String>)> = match sqlx::query_as(
        "SELECT id, notified_at FROM alerts WHERE key = ? AND status = 'firing'",
    )
    .bind(key)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::warn!(error = %e, "alert lookup failed");
            return;
        }
    };
    let Some((id, notified)) = row else { return };
    if let Err(e) = sqlx::query(
        "UPDATE alerts SET status = 'resolved', resolved_at = datetime('now') WHERE id = ?",
    )
    .bind(&id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(error = %e, "alert resolve failed");
        return;
    }
    if notified.is_some() {
        let meta: Option<(String, String, String, Option<String>)> =
            sqlx::query_as("SELECT severity, kind, subject, detail FROM alerts WHERE id = ?")
                .bind(&id)
                .fetch_optional(&state.pool)
                .await
                .unwrap_or(None);
        if let Some((severity, kind, subject, detail)) = meta {
            dispatch(
                state,
                &id,
                &severity,
                &kind,
                "resolved",
                &subject,
                detail.as_deref(),
            )
            .await;
        }
    }
}

/// POST the alert to the webhook (Discord/Slack-compatible shape). Success
/// stamps `notified_at`; failures only warn — the next tick retries.
async fn dispatch(
    state: &AppState,
    id: &str,
    severity: &str,
    kind: &str,
    status: &str,
    subject: &str,
    detail: Option<&str>,
) {
    let url = state.cfg.alerts.webhook_url.trim();
    if url.is_empty() {
        return;
    }
    let text = format!("[turaes:{severity}] {status} {kind} — {subject}");
    let body = serde_json::json!({
        "text": text,
        "content": text,
        "severity": severity,
        "kind": kind,
        "status": status,
        "subject": subject,
        "detail": detail,
    });
    match state
        .http
        .post(url)
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let _ = sqlx::query("UPDATE alerts SET notified_at = datetime('now') WHERE id = ?")
                .bind(id)
                .execute(&state.pool)
                .await;
        }
        Ok(resp) => tracing::warn!(status = %resp.status(), "alert webhook rejected"),
        Err(e) => tracing::warn!(error = %e, "alert webhook failed"),
    }
}

/// Sustained unhealthy apps (local only — remote health belongs to agents).
async fn rule_unhealthy_apps(state: &AppState) {
    let apps: Vec<(String, String, String, String)> = match sqlx::query_as(
        "SELECT id, org_id, name, status FROM applications WHERE server_id = 'local'",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(apps) => apps,
        Err(e) => {
            tracing::warn!(error = %e, "alert rule query failed");
            return;
        }
    };
    for (id, org_id, name, status) in apps {
        let key = format!("app.unhealthy:{id}");
        if status == "unhealthy" {
            let last_error: Option<String> = sqlx::query_scalar(
                "SELECT error_message FROM health_results \
                 WHERE application_id = ? ORDER BY rowid DESC LIMIT 1",
            )
            .bind(&id)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None)
            .flatten();
            let detail = last_error
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "health checks failing".into());
            fire(
                state,
                Some(&org_id),
                "warning",
                "app.unhealthy",
                &key,
                &format!("{name} is unhealthy"),
                Some(&detail),
                Some(&id),
            )
            .await;
        } else {
            resolve(state, &key).await;
        }
    }
}

/// Deployments that failed recently (last 24h). A newer success for the same
/// app resolves the alert; ancient failures never fire.
async fn rule_failed_deploys(state: &AppState) {
    let rows: Vec<(String, String, String, String)> = match sqlx::query_as(
        "SELECT d.id, d.application_id, a.org_id, a.name FROM deployments d \
         JOIN applications a ON a.id = d.application_id \
         WHERE d.status = 'failed' AND d.finished_at > datetime('now', '-24 hours')",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "alert rule query failed");
            return;
        }
    };
    for (dep_id, app_id, org_id, name) in rows {
        let key = format!("deploy.failed:{dep_id}");
        let superseded: Option<i64> = sqlx::query_scalar(
            "SELECT count(*) FROM deployments \
             WHERE application_id = ? AND status = 'running' AND rowid > (SELECT rowid FROM deployments WHERE id = ?)",
        )
        .bind(&app_id)
        .bind(&dep_id)
        .fetch_optional(&state.pool)
        .await
        .unwrap_or(None)
        .flatten()
        .filter(|&n| n > 0);
        if superseded.is_some() {
            resolve(state, &key).await;
        } else {
            fire(
                state,
                Some(&org_id),
                "warning",
                "deploy.failed",
                &key,
                &format!("deploy of {name} failed"),
                None,
                Some(&app_id),
            )
            .await;
        }
    }
}

/// No database snapshot within the configured window (platform-global).
async fn rule_backup_stale(state: &AppState) {
    let key = "backup.stale";
    let dir = std::path::Path::new(&state.cfg.backup.dir);
    let mut newest: Option<std::time::SystemTime> = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            if path.extension().is_some_and(|e| e == "db") {
                if let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) {
                    newest = Some(newest.map_or(mtime, |n: std::time::SystemTime| n.max(mtime)));
                }
            }
        }
    }
    let stale = match newest {
        None => true,
        Some(t) => SystemTime::now()
            .duration_since(t)
            .map(|age| age > Duration::from_secs(state.cfg.alerts.backup_stale_hours * 3600))
            .unwrap_or(true),
    };
    if stale {
        fire(
            state,
            None,
            "critical",
            "backup.stale",
            key,
            &format!(
                "no database snapshot in {}h",
                state.cfg.alerts.backup_stale_hours
            ),
            Some(&format!("expected snapshots in {}", state.cfg.backup.dir)),
            None,
        )
        .await;
    } else {
        resolve(state, key).await;
    }
}

/// Evaluate every rule. Internal failures are logged, never fatal, so one
/// broken rule cannot take down the monitor tick.
pub async fn evaluate(state: &AppState) {
    rule_unhealthy_apps(state).await;
    rule_failed_deploys(state).await;
    rule_backup_stale(state).await;
}
