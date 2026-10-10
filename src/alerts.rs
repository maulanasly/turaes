//! Alert rules evaluated on every monitor tick, plus webhook dispatch.
//!
//! Rules are stateless queries; the `alerts` table is the state. Firing rows
//! are deduplicated by `key` (one firing row per key, ever), so evaluation is
//! idempotent and cheap. Webhook events live in a durable outbox and retry
//! with backoff; with no webhook configured, alerts are still recorded for
//! the dashboard banner.

use std::time::{Duration, SystemTime};

use crate::state::AppState;

const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(10);
const OUTBOX_BATCH: i64 = 20;
const MAX_BACKOFF_SECS: i64 = 3600;

async fn enqueue_delivery(
    state: &AppState,
    alert_id: &str,
    event: &str,
    severity: &str,
    kind: &str,
    subject: &str,
    detail: Option<&str>,
) {
    if state.cfg.alerts.webhook_url.trim().is_empty() {
        return;
    }
    let text = format!("[turaes:{severity}] {event} {kind}: {subject}");
    let payload = serde_json::json!({
        "text": text,
        "content": text,
        "severity": severity,
        "kind": kind,
        "status": event,
        "subject": subject,
        "detail": detail,
    })
    .to_string();
    if let Err(e) = sqlx::query(
        "INSERT OR IGNORE INTO alert_deliveries (id, alert_id, event, payload) VALUES (?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(alert_id)
    .bind(event)
    .bind(payload)
    .execute(&state.pool)
    .await
    {
        tracing::error!(alert_id, event, error = %e, "failed to enqueue alert webhook");
    }
}

/// Deliver due outbox rows. This is at-least-once: a crash after the remote
/// webhook accepts a request but before we mark it delivered can resend it.
/// The stable Idempotency-Key lets receivers deduplicate where supported.
pub async fn drain_outbox(state: &AppState) {
    if state.cfg.alerts.webhook_url.trim().is_empty() {
        return;
    }
    let rows: Vec<(String, String, String, String, i64)> = match sqlx::query_as(
        "SELECT d.id, d.alert_id, d.event, d.payload, d.attempts FROM alert_deliveries d \
         JOIN alerts a ON a.id = d.alert_id \
         WHERE d.delivered_at IS NULL AND d.next_attempt_at <= datetime('now') \
           AND (d.lease_until IS NULL OR d.lease_until <= datetime('now')) \
           AND (d.event != 'resolved' OR NOT EXISTS ( \
             SELECT 1 FROM alert_deliveries f WHERE f.alert_id = d.alert_id \
               AND f.event = 'firing' AND f.delivered_at IS NULL)) \
         ORDER BY d.created_at, d.rowid LIMIT ?",
    )
    .bind(OUTBOX_BATCH)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "alert outbox query failed");
            return;
        }
    };
    let url = state.cfg.alerts.webhook_url.trim();
    for (id, _alert_id, event, payload, attempts) in rows {
        // Lease atomically before the network request. If another evaluator
        // is draining concurrently, only one wins this compare-and-set.
        let claimed = sqlx::query(
            "UPDATE alert_deliveries SET lease_until = datetime('now', '+60 seconds') \
             WHERE id = ? AND delivered_at IS NULL AND next_attempt_at <= datetime('now') \
               AND (lease_until IS NULL OR lease_until <= datetime('now'))",
        )
        .bind(&id)
        .execute(&state.pool)
        .await
        .map(|r| r.rows_affected() == 1)
        .unwrap_or(false);
        if !claimed {
            continue;
        }
        let sent = state
            .http
            .post(url)
            .header("Idempotency-Key", &id)
            .header("X-Turaes-Alert-Event", &id)
            .header("content-type", "application/json")
            .body(payload)
            .timeout(WEBHOOK_TIMEOUT)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if sent {
            if let Err(e) = sqlx::query(
                "UPDATE alert_deliveries SET delivered_at = datetime('now'), \
                 lease_until = NULL, last_error = NULL WHERE id = ?",
            )
            .bind(&id)
            .execute(&state.pool)
            .await
            {
                tracing::warn!(delivery_id = %id, error = %e, "failed to mark alert delivered");
            }
            if event == "firing" {
                let _ = sqlx::query(
                    "UPDATE alerts SET notified_at = datetime('now') WHERE id = (SELECT alert_id FROM alert_deliveries WHERE id = ?)",
                )
                .bind(&id)
                .execute(&state.pool)
                .await;
            }
        } else {
            let next = attempts.saturating_add(1);
            let backoff = 2_i64
                .saturating_pow(next.min(20) as u32)
                .min(MAX_BACKOFF_SECS);
            let _ = sqlx::query(
                "UPDATE alert_deliveries SET attempts = ?, lease_until = NULL, \
                 next_attempt_at = datetime('now', ?), last_error = 'webhook request failed' \
                 WHERE id = ? AND delivered_at IS NULL",
            )
            .bind(next)
            .bind(format!("+{backoff} seconds"))
            .bind(&id)
            .execute(&state.pool)
            .await;
            tracing::warn!(delivery_id = %id, attempts = next, backoff, "alert webhook failed; queued for retry");
        }
    }
}

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
    if let Some(id) = existing {
        enqueue_delivery(state, &id, "firing", severity, kind, subject, detail).await;
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
        // Concurrent evaluation can lose the unique active-key race. The
        // winner still owns this firing event; queue its delivery if needed.
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM alerts WHERE key = ? AND status = 'firing'")
                .bind(key)
                .fetch_optional(&state.pool)
                .await
                .unwrap_or(None);
        if let Some(id) = existing {
            enqueue_delivery(state, &id, "firing", severity, kind, subject, detail).await;
        }
        return;
    }
    tracing::warn!(kind, subject, "alert firing");
    enqueue_delivery(state, &id, "firing", severity, kind, subject, detail).await;
}

/// Queue a resolved webhook event for a row transitioned by the HTTP handler.
/// The outbox unique key makes duplicate calls harmless.
pub async fn queue_resolved(state: &AppState, id: &str) {
    let row: Option<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT severity, kind, subject, detail FROM alerts WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);
    if let Some((severity, kind, subject, detail)) = row {
        enqueue_delivery(
            state,
            id,
            "resolved",
            &severity,
            &kind,
            &subject,
            detail.as_deref(),
        )
        .await;
    }
}

/// Resolve the firing row for `key` and queue its resolution event.
async fn resolve(state: &AppState, key: &str) {
    let row: Option<(String, String, String, String, Option<String>)> = match sqlx::query_as(
        "SELECT id, severity, kind, subject, detail FROM alerts WHERE key = ? AND status = 'firing'",
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
    let Some((id, severity, kind, subject, detail)) = row else {
        return;
    };
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
    enqueue_delivery(
        state,
        &id,
        "resolved",
        &severity,
        &kind,
        &subject,
        detail.as_deref(),
    )
    .await;
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
    drain_outbox(state).await;
}
