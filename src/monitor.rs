//! Background monitoring loop.
//!
//! Every `monitor.interval_secs`: probe each app's health endpoint, scrape its
//! tonggeret `/metrics`, sample CPU/memory (cgroup v2, or `/proc` for the
//! `proc` runtime), and roll the results into **1-minute buckets** in SQLite.
//! Threshold crossings can trigger a supervised restart.
//!
//! Buckets are upserted each tick, so the current minute's row stays fresh
//! while storage is one row per app (and per region) per minute.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use turaes_core::models::Application;
use turaes_monitor::{health, scrape, stats};
use turaes_runtime::RunState;

use crate::routes::apps::{active_spec, refresh_proxy_routes, runtime_for};
use crate::state::AppState;

/// Running aggregate for one app's current minute.
#[derive(Default)]
struct Bucket {
    /// Bucket start, `YYYY-MM-DD HH:MM:00` (UTC).
    minute: String,
    cpu_sum: f64,
    cpu_count: u64,
    mem_max: u64,
    /// region -> (visits in bucket, latest unique-visitor estimate).
    visits: HashMap<String, (i64, i64)>,
}

/// Per-app state kept in memory between ticks (never persisted).
#[derive(Default)]
struct AppMemo {
    threshold: health::Threshold,
    last_cpu_usec: Option<u64>,
    last_tick: Option<Instant>,
    visit_counters: HashMap<String, i64>,
    bucket: Bucket,
}

fn current_minute() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:00").to_string()
}

/// Run the monitor forever. Intended to be spawned on process start.
pub async fn run(state: AppState) {
    let secs = state.cfg.monitor.interval_secs.max(1);
    let interval = Duration::from_secs(secs);
    let mut memo: HashMap<String, AppMemo> = HashMap::new();
    let mut host_cpu: Option<(u64, u64)> = None;
    tracing::info!(interval_secs = secs, "monitor started");
    loop {
        if let Err(e) = tick(&state, &mut memo, &mut host_cpu).await {
            tracing::warn!(error = %e, "monitor tick failed");
        }
        tokio::time::sleep(interval).await;
    }
}

async fn tick(
    state: &AppState,
    memo: &mut HashMap<String, AppMemo>,
    host_cpu: &mut Option<(u64, u64)>,
) -> turaes_core::Result<()> {
    // Only monitor apps on this host. Remote apps are supervised (health,
    // restart) by their node's agent; the control plane can't reach or restart
    // them locally. (Cross-node metrics land in N2.)
    let apps =
        sqlx::query_as::<_, Application>("SELECT * FROM applications WHERE server_id = 'local'")
            .fetch_all(&state.pool)
            .await?;

    for app in &apps {
        let entry = memo.entry(app.id.clone()).or_default();
        if let Err(e) = watch_app(state, app, entry).await {
            tracing::debug!(app = %app.name, error = %e, "app monitor step failed");
        }
    }

    let alive: std::collections::HashSet<&str> = apps.iter().map(|a| a.id.as_str()).collect();
    memo.retain(|id, _| alive.contains(id.as_str()));

    cleanup(state).await?;

    sample_host(state, host_cpu).await;

    // Alert rules run last: they read the statuses this tick just wrote.
    crate::alerts::evaluate(state).await;
    Ok(())
}

/// Sample this host's CPU/memory into `server_metrics` for the local node.
/// Silent on non-Linux hosts (no `/proc`): remote capacity simply reads back
/// as unknown until agents report host stats.
async fn sample_host(state: &AppState, prev: &mut Option<(u64, u64)>) {
    let Some(reading) = stats::read_host().await else {
        return;
    };
    let cpu_pct = match prev.replace((reading.cpu_idle_ticks, reading.cpu_total_ticks)) {
        Some((prev_idle, prev_total)) => stats::host_cpu_percent(
            prev_idle,
            prev_total,
            reading.cpu_idle_ticks,
            reading.cpu_total_ticks,
        ),
        None => 0.0,
    };
    if let Err(e) = upsert_host_metrics(
        &state.pool,
        "local",
        cpu_pct,
        reading.mem_bytes as i64,
        reading.mem_total_bytes as i64,
        &current_minute(),
    )
    .await
    {
        tracing::warn!(error = %e, "failed to upsert server_metrics");
    }
}

/// Minute-bucket upsert shared by the monitor tick (and tests).
pub(crate) async fn upsert_host_metrics(
    pool: &turaes_core::db::Pool,
    server_id: &str,
    cpu_pct: f64,
    mem_bytes: i64,
    mem_total_bytes: i64,
    minute: &str,
) -> turaes_core::Result<()> {
    sqlx::query(
        "INSERT INTO server_metrics (id, server_id, cpu_pct, mem_bytes, mem_total_bytes, recorded_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(server_id, recorded_at) DO UPDATE SET \
           cpu_pct = excluded.cpu_pct, mem_bytes = excluded.mem_bytes, \
           mem_total_bytes = excluded.mem_total_bytes",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(server_id)
    .bind(cpu_pct)
    .bind(mem_bytes)
    .bind(mem_total_bytes)
    .bind(minute)
    .execute(pool)
    .await?;
    Ok(())
}

async fn watch_app(
    state: &AppState,
    app: &Application,
    memo: &mut AppMemo,
) -> turaes_core::Result<()> {
    // Probe and scrape the live blue/green slot, not the base port (which is
    // dead while the other slot serves).
    let base = format!("http://127.0.0.1:{}", live_port(app));
    let elapsed = memo
        .last_tick
        .map(|t| t.elapsed().as_secs_f64())
        .unwrap_or(0.0);
    memo.last_tick = Some(Instant::now());

    let minute = current_minute();
    if memo.bucket.minute != minute {
        memo.bucket = Bucket {
            minute,
            ..Default::default()
        };
    }

    if app.kind == "worker" {
        // Workers expose no HTTP surface: mirror supervisor state instead of
        // probing. A dead worker with auto_restart is restarted (systemd
        // already does this itself; the proc supervisor needs the nudge).
        watch_worker(state, app).await?;
    } else {
        probe_health(state, app, memo).await?;
        scrape_metrics(state, app, &base, memo).await;
    }
    sample_resources(state, app, memo, elapsed).await;
    flush_bucket(state, app, memo).await;

    Ok(())
}

/// Mirror a worker's supervisor state into the database.
async fn watch_worker(state: &AppState, app: &Application) -> turaes_core::Result<()> {
    use crate::routes::apps::{active_spec, runtime_for};

    let runtime = runtime_for(&state.cfg, &app.runtime);
    let spec = active_spec(&state.cfg, app);
    let observed = runtime.status(&spec).await.unwrap_or(RunState::Unknown);
    let desired = observed.as_status();
    if app.status != desired && app.status != "stopped" {
        set_status(state, &app.id, desired).await?;
        record_event(
            state,
            Some(app.id.as_str()),
            "worker",
            &format!("{} is {}", app.name, desired),
        )
        .await?;
    }
    // Never touch a manually stopped (or never-started) worker: no restart
    // spam against missing units, no status churn.
    if observed != RunState::Running && app.auto_restart && app.status != "stopped" {
        if let Err(e) = runtime.restart(&spec).await {
            tracing::warn!(app = %app.name, error = %e, "worker auto-restart failed");
        }
    }
    Ok(())
}

async fn probe_health(
    state: &AppState,
    app: &Application,
    memo: &mut AppMemo,
) -> turaes_core::Result<()> {
    let cfg = health::HealthConfig {
        path: app.health_path.clone(),
        ..Default::default()
    };
    let url = format!("http://127.0.0.1:{}{}", live_port(app), app.health_path);
    let outcome = health::probe(&state.http, &url, Duration::from_secs(cfg.timeout_secs)).await;

    sqlx::query(
        "INSERT INTO health_results \
         (id, application_id, status, status_code, response_time_ms, error_message, checked_at) \
         VALUES (?, ?, ?, ?, ?, ?, datetime('now'))",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&app.id)
    .bind(outcome.status)
    .bind(outcome.status_code.map(i64::from))
    .bind(outcome.response_time_ms as i64)
    .bind(&outcome.error_message)
    .execute(&state.pool)
    .await?;

    match memo.threshold.record(&outcome, &cfg) {
        Some(health::Transition::BecameUnhealthy) => {
            set_status(state, &app.id, "unhealthy").await?;
            // Park the hostnames (maintenance page) until recovery.
            let _ = refresh_proxy_routes(state).await;
            record_event(
                state,
                Some(app.id.as_str()),
                "health",
                &format!("{} became unhealthy", app.name),
            )
            .await?;
            if app.auto_restart {
                let runtime = runtime_for(&state.cfg, &app.runtime);
                // Restart the live slot — never the legacy unslotted unit,
                // which would resurrect a stale listener on the base port.
                let spec = active_spec(&state.cfg, app);
                if let Err(e) = runtime.restart(&spec).await {
                    tracing::warn!(app = %app.name, error = %e, "auto-restart failed");
                } else {
                    record_event(
                        state,
                        Some(app.id.as_str()),
                        "restart",
                        "auto-restarted after unhealthy",
                    )
                    .await?;
                }
            }
        }
        Some(health::Transition::BecameHealthy) => {
            set_status(state, &app.id, "running").await?;
            // Restore the routes the unhealthy transition parked.
            let _ = refresh_proxy_routes(state).await;
            record_event(
                state,
                Some(app.id.as_str()),
                "health",
                &format!("{} recovered", app.name),
            )
            .await?;
        }
        None => {
            let desired = if memo.threshold.is_healthy() {
                "running"
            } else {
                "unhealthy"
            };
            if app.status != desired && app.status != "stopped" {
                set_status(state, &app.id, desired).await?;
                // Reconciled flips change routability; republish if so.
                let _ = refresh_proxy_routes(state).await;
            }
        }
    }
    Ok(())
}

async fn scrape_metrics(state: &AppState, app: &Application, base: &str, memo: &mut AppMemo) {
    let Some(path) = app.metrics_path.as_deref() else {
        return;
    };
    let url = format!("{base}{path}");
    let body = match state
        .http
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(text) => text,
            Err(_) => return,
        },
        _ => return,
    };
    let samples = scrape::parse(&body);
    for visit in scrape::visitor_samples(&samples) {
        let prev = memo.visit_counters.get(&visit.region).copied().unwrap_or(0);
        let delta = if visit.visits >= prev {
            visit.visits - prev
        } else {
            visit.visits
        };
        memo.visit_counters
            .insert(visit.region.clone(), visit.visits);
        let entry = memo.bucket.visits.entry(visit.region).or_insert((0, 0));
        entry.0 += delta;
        entry.1 = entry.1.max(visit.uniques);
    }
}

async fn sample_resources(
    state: &AppState,
    app: &Application,
    memo: &mut AppMemo,
    elapsed_secs: f64,
) {
    let instance = active_instance(&state.cfg, app);
    let reading = match app.runtime.as_str() {
        "proc" => read_proc_reading(state, app, &instance).await,
        _ => {
            let dir = format!("/sys/fs/cgroup/system.slice/{instance}.service");
            stats::read_cgroup(std::path::Path::new(&dir)).await
        }
    };
    let Some(reading) = reading else {
        return;
    };
    let cpu_pct = match memo.last_cpu_usec.replace(reading.cpu_usage_usec) {
        Some(prev) => stats::cpu_percent(prev, reading.cpu_usage_usec, elapsed_secs),
        None => 0.0,
    };
    memo.bucket.cpu_sum += cpu_pct;
    memo.bucket.cpu_count += 1;
    memo.bucket.mem_max = memo.bucket.mem_max.max(reading.mem_bytes);
}

async fn flush_bucket(state: &AppState, app: &Application, memo: &AppMemo) {
    let bucket = &memo.bucket;

    if bucket.cpu_count > 0 {
        let cpu_avg = bucket.cpu_sum / bucket.cpu_count as f64;
        if let Err(e) = sqlx::query(
            "INSERT INTO app_metrics (id, application_id, cpu_pct, mem_bytes, recorded_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(application_id, recorded_at) DO UPDATE SET \
               cpu_pct = excluded.cpu_pct, mem_bytes = excluded.mem_bytes",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&app.id)
        .bind(cpu_avg)
        .bind(bucket.mem_max as i64)
        .bind(&bucket.minute)
        .execute(&state.pool)
        .await
        {
            tracing::warn!(app = %app.name, error = %e, "failed to upsert app_metrics");
        }
    }

    for (region, (visits, uniques)) in &bucket.visits {
        if *visits == 0 && *uniques == 0 {
            continue;
        }
        if let Err(e) = sqlx::query(
            "INSERT INTO visit_metrics (id, application_id, region, visits, uniques, recorded_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(application_id, region, recorded_at) DO UPDATE SET \
               visits = excluded.visits, uniques = excluded.uniques",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&app.id)
        .bind(region)
        .bind(*visits)
        .bind(*uniques)
        .bind(&bucket.minute)
        .execute(&state.pool)
        .await
        {
            tracing::warn!(app = %app.name, region = %region, error = %e, "failed to upsert visit_metrics");
        }
    }
}

/// The running blue/green instance name (`name`, `name-a`, or `name-b`).
/// Port serving live traffic: the active blue/green slot, else the base port.
fn live_port(app: &Application) -> i64 {
    app.active_port.unwrap_or(app.port)
}

fn active_instance(cfg: &turaes_core::config::Config, app: &Application) -> String {
    let offset = cfg.runtime.slot_offset as i64;
    match app.active_port {
        Some(ap) if ap == app.port + offset => format!("{}-b", app.name),
        Some(_) => format!("{}-a", app.name),
        None => app.name.clone(),
    }
}

async fn read_proc_reading(
    state: &AppState,
    app: &Application,
    instance: &str,
) -> Option<stats::ResourceStats> {
    // ProcRuntime writes the pid at {state_dir}/{app}/{instance}.pid.
    let pid_path = format!(
        "{}/{}/{}.pid",
        state.cfg.runtime.state_dir, app.name, instance
    );
    let pid = tokio::fs::read_to_string(pid_path).await.ok()?;
    let body = tokio::fs::read_to_string(format!("/proc/{}/stat", pid.trim()))
        .await
        .ok()?;
    let parsed = stats::parse_proc_stat(&body)?;
    Some(stats::proc_to_stats(parsed, 100, 4096))
}

async fn set_status(state: &AppState, id: &str, status: &str) -> turaes_core::Result<()> {
    sqlx::query("UPDATE applications SET status = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(status)
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

async fn record_event(
    state: &AppState,
    app_id: Option<&str>,
    kind: &str,
    message: &str,
) -> turaes_core::Result<()> {
    sqlx::query("INSERT INTO events (id, application_id, kind, message, created_at) VALUES (?, ?, ?, ?, datetime('now'))")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(app_id)
        .bind(kind)
        .bind(message)
        .execute(&state.pool)
        .await?;
    Ok(())
}

async fn cleanup(state: &AppState) -> turaes_core::Result<()> {
    let cutoff = format!("-{} days", state.cfg.monitor.retention_days.max(1));
    // (table, timestamp column) — health_results uses checked_at, the rest
    // use recorded_at.
    for (table, ts) in [
        ("app_metrics", "recorded_at"),
        ("visit_metrics", "recorded_at"),
        ("health_results", "checked_at"),
        ("server_metrics", "recorded_at"),
    ] {
        let sql = format!("DELETE FROM {table} WHERE {ts} < datetime('now', ?)");
        sqlx::query(&sql).bind(&cutoff).execute(&state.pool).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(port: i64, active_port: Option<i64>) -> Application {
        Application {
            id: "a".into(),
            name: "beruang".into(),
            description: None,
            binary_path: "/bin/true".into(),
            args: None,
            port,
            active_port,
            health_path: "/health".into(),
            metrics_path: None,
            domain: None,
            server_id: "local".into(),
            org_id: "default".into(),
            mem_limit_mb: None,
            cpu_quota_pct: None,
            kind: "service".into(),
            command: None,
            workdir: None,
            publish_dir: None,
            runtime: "systemd".into(),
            auto_restart: true,
            status: "running".into(),
            maintenance: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn live_port_follows_the_active_slot() {
        assert_eq!(live_port(&app(8000, None)), 8000);
        assert_eq!(live_port(&app(8000, Some(8000))), 8000);
        assert_eq!(live_port(&app(8000, Some(18000))), 18000);
    }
}
