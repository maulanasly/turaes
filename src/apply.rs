//! `turaes apply`: declarative app management from `turaes.yaml`.
//!
//! Config-only: creates or updates the application row plus its placement,
//! health checks, domains, env and secrets. Deploying (the blue/green
//! cutover) stays an explicit separate step. Relative `binary`/`publish_dir`
//! paths resolve against the manifest file's directory. `--dry-run` computes
//! the full diff without writing anything. Declarative pruning removes aliases
//! and env keys the file no longer declares (shown in dry-run first).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use turaes_core::manifest::{AppKind, AppManifest};
use turaes_core::models::Application;
use turaes_core::{Error, Result};

use crate::audit;
use crate::routes::apps;
use crate::routes::quotas;
use crate::state::AppState;

/// Outcome of applying one manifest, for CLI display and tests.
#[derive(Debug, Default)]
pub struct ApplyReport {
    /// Application id (new or existing).
    pub app_id: String,
    /// Application name.
    pub app_name: String,
    /// True when the app was created by this run.
    pub created: bool,
    /// Fields changed on update (`created` implies everything is new).
    pub changed: Vec<String>,
    /// Aliases removed that the file no longer declares.
    pub pruned_aliases: usize,
    /// Env keys written (upserted plaintext values).
    pub env_wrote: usize,
    /// Env keys removed that the file no longer declares.
    pub pruned_env: usize,
    /// Secrets minted via `generate: true` this run.
    pub generated_secrets: Vec<String>,
}

impl std::fmt::Display for ApplyReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.created {
            writeln!(f, "created app '{}' ({})", self.app_name, self.app_id)?;
        } else if self.changed.is_empty()
            && self.env_wrote == 0
            && self.pruned_aliases == 0
            && self.pruned_env == 0
            && self.generated_secrets.is_empty()
        {
            writeln!(f, "no changes for '{}'", self.app_name)?;
            return Ok(());
        } else if self.changed.is_empty() {
            writeln!(f, "reconciled '{}' (env/secrets only)", self.app_name)?;
        } else {
            writeln!(
                f,
                "updated app '{}': {}",
                self.app_name,
                self.changed.join(", ")
            )?;
        }
        if self.env_wrote > 0 {
            writeln!(f, "set {} env key(s)", self.env_wrote)?;
        }
        if self.pruned_aliases > 0 {
            writeln!(f, "pruned {} alias(es)", self.pruned_aliases)?;
        }
        if self.pruned_env > 0 {
            writeln!(f, "pruned {} env key(s)", self.pruned_env)?;
        }
        if !self.generated_secrets.is_empty() {
            writeln!(
                f,
                "generated secrets: {}",
                self.generated_secrets.join(", ")
            )?;
        }
        Ok(())
    }
}

/// Resolve a manifest path against the manifest file's directory: absolute
/// paths pass through, relative ones anchor at the file (repo-relative UX).
fn anchor(base_dir: &Path, raw: &str) -> String {
    let p = Path::new(raw);
    if p.is_absolute() {
        return raw.to_string();
    }
    base_dir.join(p).to_string_lossy().into_owned()
}

/// Load + parse + validate a manifest file.
pub fn load_manifest(path: &Path) -> Result<(AppManifest, PathBuf)> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| Error::BadRequest(format!("cannot read {}: {e}", path.display())))?;
    let manifest = AppManifest::parse(&raw)?;
    manifest.validate()?;
    let base = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok((manifest, base))
}

/// Resolve a server name/id to its row id.
async fn resolve_server(state: &AppState, server: Option<&str>) -> Result<String> {
    let name = server.unwrap_or("local");
    sqlx::query_scalar::<_, String>("SELECT id FROM servers WHERE id = ? OR name = ?")
        .bind(name)
        .bind(name)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| Error::BadRequest(format!("unknown server '{name}'")))
}

/// Stored secret values for an app: key -> sealed blob.
async fn stored_secrets(state: &AppState, app_id: &str) -> Result<BTreeMap<String, String>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value_enc FROM env_vars WHERE application_id = ?")
            .bind(app_id)
            .fetch_all(&state.pool)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Split manifest secrets into missing values (exact recovery commands) and
/// keys to mint. Pure so every apply path shares it without double-wrapping
/// the error text.
fn gate_secrets(
    manifest: &AppManifest,
    stored: &BTreeMap<String, String>,
    app_name: &str,
) -> (Vec<String>, Vec<String>) {
    let mut missing = Vec::new();
    let mut to_generate = Vec::new();
    for secret in &manifest.secrets {
        let key = secret.key();
        if stored.contains_key(key) {
            continue;
        }
        if secret.wants_generate() {
            to_generate.push(key.to_string());
        } else {
            missing.push(format!("turaes secrets set {app_name} {key}"));
        }
    }
    (missing, to_generate)
}

/// Fail closed on missing secret values.
fn require_secrets(missing: &[String]) -> Result<()> {
    if missing.is_empty() {
        return Ok(());
    }
    Err(Error::BadRequest(format!(
        "manifest needs {} unset secret(s); run first:\n  {}",
        missing.len(),
        missing.join("\n  ")
    )))
}

/// Upsert the health_checks row for an app from the manifest block.
async fn upsert_health(
    state: &AppState,
    app_id: &str,
    manifest: &AppManifest,
    health_path: &str,
    dry_run: bool,
) -> Result<()> {
    let h = manifest.health.as_ref();
    let interval = h
        .and_then(|h| h.interval.as_ref())
        .map(|d| d.seconds())
        .transpose()?
        .map(|n| n as i64)
        .unwrap_or(15);
    let timeout = h
        .and_then(|h| h.timeout.as_ref())
        .map(|d| d.seconds())
        .transpose()?
        .map(|n| n as i64)
        .unwrap_or(5);
    let healthy = h
        .and_then(|h| h.healthy_threshold)
        .map(|n| n as i64)
        .unwrap_or(2);
    let unhealthy = h
        .and_then(|h| h.unhealthy_threshold)
        .map(|n| n as i64)
        .unwrap_or(3);
    let path = h
        .and_then(|h| h.path.clone())
        .unwrap_or_else(|| health_path.to_string());
    if dry_run {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO health_checks \
         (id, application_id, path, interval_seconds, timeout_seconds, healthy_threshold, unhealthy_threshold) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(application_id) DO UPDATE SET path = excluded.path, \
         interval_seconds = excluded.interval_seconds, timeout_seconds = excluded.timeout_seconds, \
         healthy_threshold = excluded.healthy_threshold, unhealthy_threshold = excluded.unhealthy_threshold",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(app_id)
    .bind(&path)
    .bind(interval)
    .bind(timeout)
    .bind(healthy)
    .bind(unhealthy)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Sync aliases: add missing, prune undeclared. Returns pruned count.
async fn sync_aliases(
    state: &AppState,
    app_id: &str,
    aliases: &[String],
    dry_run: bool,
) -> Result<usize> {
    let desired: Vec<String> = {
        let mut v: Vec<String> = aliases
            .iter()
            .map(|d| d.trim().to_ascii_lowercase())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let existing: Vec<(String, String)> =
        sqlx::query_as("SELECT id, domain FROM domains WHERE application_id = ?")
            .bind(app_id)
            .fetch_all(&state.pool)
            .await?;
    let mut pruned = 0;
    for (id, domain) in &existing {
        if !desired.contains(domain) {
            pruned += 1;
            if !dry_run {
                sqlx::query("DELETE FROM domains WHERE id = ?")
                    .bind(id)
                    .execute(&state.pool)
                    .await?;
            }
        }
    }
    for domain in &desired {
        if existing.iter().any(|(_, d)| d == domain) {
            continue;
        }
        if dry_run {
            continue;
        }
        sqlx::query(
            "INSERT INTO domains (id, application_id, domain, is_primary) VALUES (?, ?, ?, 0)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(app_id)
        .bind(domain)
        .execute(&state.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(db) = &e {
                if db.message().contains("UNIQUE") {
                    return Error::Conflict(format!("domain '{domain}' is already in use"));
                }
            }
            Error::Db(e)
        })?;
    }
    Ok(pruned)
}

/// Sync env: upsert manifest plaintext, ensure secret presence (minting
/// `generate` keys), prune undeclared keys. Returns (wrote, pruned, generated).
#[allow(clippy::too_many_arguments)]
async fn sync_env(
    state: &AppState,
    org_id: &str,
    app: &Application,
    manifest: &AppManifest,
    to_generate: &[String],
    dry_run: bool,
) -> Result<(usize, usize, Vec<String>)> {
    let stored = stored_secrets(state, &app.id).await?;
    let mut declared: Vec<String> = manifest.env.keys().cloned().collect();
    for secret in &manifest.secrets {
        declared.push(secret.key().to_string());
    }
    declared.sort();
    declared.dedup();

    // Prune keys the file no longer declares (values included: declarative).
    let mut pruned = 0;
    for key in stored.keys() {
        if !declared.contains(key) {
            pruned += 1;
            if !dry_run {
                sqlx::query("DELETE FROM env_vars WHERE application_id = ? AND key = ?")
                    .bind(&app.id)
                    .bind(key)
                    .execute(&state.pool)
                    .await?;
            }
        }
    }

    // Upsert plaintext env.
    let mut wrote = 0;
    for (key, value) in &manifest.env {
        let same = stored
            .get(key)
            .and_then(|sealed| state.secrets.open(sealed).ok())
            .is_some_and(|plain| &plain == value);
        if same {
            continue;
        }
        wrote += 1;
        if dry_run {
            continue;
        }
        let sealed = state.secrets.seal(value)?;
        sqlx::query(
            "INSERT INTO env_vars (id, application_id, key, value_enc) VALUES (?, ?, ?, ?) \
             ON CONFLICT(application_id, key) DO UPDATE SET value_enc = excluded.value_enc",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&app.id)
        .bind(key)
        .bind(&sealed)
        .execute(&state.pool)
        .await?;
    }

    // Mint generate-once secrets.
    let mut generated = Vec::new();
    for key in to_generate {
        generated.push(key.clone());
        if dry_run {
            continue;
        }
        let value = turaes_core::crypto::random_token(32);
        let sealed = state.secrets.seal(&value)?;
        sqlx::query(
            "INSERT INTO env_vars (id, application_id, key, value_enc) VALUES (?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&app.id)
        .bind(key)
        .bind(&sealed)
        .execute(&state.pool)
        .await?;
    }

    // Audit the wiring (values never recorded).
    if !dry_run {
        audit::record(
            state,
            Some(org_id),
            None,
            Some(&app.id),
            "env.set",
            Some("env_var"),
            None,
            Some(&serde_json::json!({"via": "apply"}).to_string()),
        )
        .await?;
    }
    Ok((wrote, pruned, generated))
}

/// Apply a parsed manifest into `org_id`. Config-only: no deploy happens here.
pub async fn apply_manifest(
    state: &AppState,
    org_id: &str,
    manifest: &AppManifest,
    base_dir: &Path,
    dry_run: bool,
) -> Result<ApplyReport> {
    manifest.validate()?;
    let runtime = manifest
        .runtime
        .clone()
        .unwrap_or_else(|| state.cfg.runtime.driver.clone());
    if !matches!(runtime.as_str(), "systemd" | "proc") {
        return Err(Error::BadRequest(
            "runtime must be 'systemd' or 'proc'".into(),
        ));
    }
    let server_id = resolve_server(state, manifest.server.as_deref()).await?;
    let kind = manifest.kind.as_str().to_string();
    let port: i64 = match manifest.kind {
        AppKind::Worker => 0,
        _ => manifest.port.unwrap() as i64,
    };
    let existing: Option<Application> =
        sqlx::query_as("SELECT * FROM applications WHERE name = ? AND org_id = ?")
            .bind(&manifest.name)
            .bind(org_id)
            .fetch_optional(&state.pool)
            .await?;
    // Capacity checks exclude the app being updated (else it conflicts with
    // itself); creates exclude nobody.
    let except_id = existing.as_ref().map(|a| a.id.clone()).unwrap_or_default();
    if port != 0 && port != existing.as_ref().map(|a| a.port).unwrap_or(-1) {
        apps::ensure_port_free(
            &state.pool,
            port,
            state.cfg.runtime.slot_offset as i64,
            &except_id,
        )
        .await?;
    }
    apps::validate_limits(
        manifest.resources.as_ref().and_then(|r| r.memory_mb),
        manifest.resources.as_ref().and_then(|r| r.cpu_percent),
        &runtime,
    )?;
    quotas::ensure_capacity(
        &state.pool,
        org_id,
        &except_id,
        manifest.resources.as_ref().and_then(|r| r.memory_mb),
        manifest.resources.as_ref().and_then(|r| r.cpu_percent),
    )
    .await?;
    let existing_domain = existing.as_ref().and_then(|a| a.domain.clone());
    if manifest.domain.as_ref().is_some_and(|d| !d.is_empty()) && manifest.domain != existing_domain
    {
        quotas::ensure_domain_capacity(&state.pool, org_id).await?;
    }

    // Resolve file-anchored paths now so dry-run and real runs agree.
    let binary_path = match &manifest.command {
        Some(argv) => argv[0].clone(),
        None if kind == "static" => anchor(
            base_dir,
            manifest.publish_dir.as_deref().unwrap_or_default(),
        ),
        None => anchor(base_dir, manifest.binary.as_deref().unwrap_or_default()),
    };
    let workdir = manifest.workdir.as_deref().map(|w| anchor(base_dir, w));
    let publish_dir = manifest.publish_dir.as_deref().map(|p| anchor(base_dir, p));
    let command_json = manifest
        .command
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| Error::BadRequest(format!("invalid command argv: {e}")))?;
    let health_path = manifest
        .health
        .as_ref()
        .and_then(|h| h.path.clone())
        .unwrap_or_else(|| "/health".into());
    let metrics_path = match manifest.kind {
        AppKind::Worker => None,
        _ => Some(
            manifest
                .metrics_path
                .clone()
                .unwrap_or_else(|| "/metrics".into()),
        ),
    };

    // The secrets gate runs before any write on the update path (the app
    // exists, so its secrets are settable). See the create path below for why
    // new apps gate after the shell row exists.
    if let Some(app) = existing {
        let stored = stored_secrets(state, &app.id).await?;
        let (missing, to_generate) = gate_secrets(manifest, &stored, &manifest.name);
        require_secrets(&missing)?;
        return apply_update(
            state,
            org_id,
            &app,
            manifest,
            &runtime,
            &server_id,
            &kind,
            port,
            &binary_path,
            command_json,
            workdir,
            publish_dir,
            &health_path,
            metrics_path,
            &to_generate,
            dry_run,
        )
        .await;
    }

    // New apps cannot gate first: `secrets set` needs the app to exist. Gate
    // against the empty store for dry-run previews (a miss still aborts).
    if dry_run {
        let (missing, to_generate) = gate_secrets(manifest, &BTreeMap::new(), &manifest.name);
        require_secrets(&missing)?;
        let mut report = ApplyReport {
            app_id: String::new(),
            app_name: manifest.name.clone(),
            created: true,
            ..Default::default()
        };
        report.generated_secrets = to_generate;
        return Ok(report);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let app = sqlx::query_as::<_, Application>(
        "INSERT INTO applications \
         (id, org_id, name, description, binary_path, args, port, health_path, metrics_path, domain, \
          server_id, runtime, auto_restart, mem_limit_mb, cpu_quota_pct, kind, command, workdir, \
          publish_dir, status, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'stopped', datetime('now'), datetime('now')) \
         RETURNING *",
    )
    .bind(&id)
    .bind(org_id)
    .bind(&manifest.name)
    .bind(&manifest.description)
    .bind(&binary_path)
    .bind(port)
    .bind(&health_path)
    .bind(&metrics_path)
    .bind(&manifest.domain)
    .bind(&server_id)
    .bind(&runtime)
    .bind(manifest.auto_restart.unwrap_or(true) as i64)
    .bind(
        manifest
            .resources
            .as_ref()
            .and_then(|r| r.memory_mb),
    )
    .bind(
        manifest
            .resources
            .as_ref()
            .and_then(|r| r.cpu_percent),
    )
    .bind(&kind)
    .bind(&command_json)
    .bind(&workdir)
    .bind(&publish_dir)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db) = &e {
            if db.message().contains("UNIQUE") {
                return Error::Conflict(format!("application '{}' already exists", manifest.name));
            }
        }
        Error::Db(e)
    })?;
    sqlx::query(
        "INSERT OR IGNORE INTO app_servers (id, application_id, server_id, port) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&app.id)
    .bind(&app.server_id)
    .bind(app.port)
    .execute(&state.pool)
    .await?;
    // Gate now that the shell exists: `secrets set` works, so a miss aborts
    // with the shell in place for a retry (re-running apply completes it).
    let (missing, to_generate) = gate_secrets(manifest, &BTreeMap::new(), &manifest.name);
    if !missing.is_empty() {
        return Err(Error::BadRequest(format!(
            "manifest needs {} unset secret(s); run first:\n  {}\n(shell app '{}' created; re-run apply after setting them)",
            missing.len(),
            missing.join("\n  "),
            manifest.name
        )));
    }
    upsert_health(state, &app.id, manifest, &health_path, false).await?;
    sync_aliases(state, &app.id, &manifest.aliases, false).await?;
    let (env_wrote, pruned_env, generated) =
        sync_env(state, org_id, &app, manifest, &to_generate, false).await?;
    audit::record(
        state,
        Some(org_id),
        None,
        Some(&app.id),
        "app.create",
        Some("application"),
        Some(&app.id),
        Some(&serde_json::json!({"name": app.name, "via": "apply"}).to_string()),
    )
    .await?;
    Ok(ApplyReport {
        app_id: app.id,
        app_name: app.name,
        created: true,
        pruned_aliases: 0,
        env_wrote,
        pruned_env,
        generated_secrets: generated,
        ..Default::default()
    })
}

#[allow(clippy::too_many_arguments)]
async fn apply_update(
    state: &AppState,
    org_id: &str,
    app: &Application,
    manifest: &AppManifest,
    runtime: &str,
    server_id: &str,
    kind: &str,
    port: i64,
    binary_path: &str,
    command_json: Option<String>,
    workdir: Option<String>,
    publish_dir: Option<String>,
    health_path: &str,
    metrics_path: Option<String>,
    to_generate: &[String],
    dry_run: bool,
) -> Result<ApplyReport> {
    if app.kind != kind {
        return Err(Error::BadRequest(format!(
            "kind cannot change from '{}' to '{kind}' (recreate the app instead)",
            app.kind
        )));
    }
    let mut changed: Vec<String> = Vec::new();
    let mut check = |field: &str, same: bool| {
        if !same {
            changed.push(field.to_string());
        }
    };
    check("description", manifest.description == app.description);
    check("binary_path", binary_path == app.binary_path);
    check("port", port == app.port);
    check("health_path", health_path == app.health_path);
    check("metrics_path", metrics_path == app.metrics_path);
    check("domain", manifest.domain == app.domain);
    check("runtime", runtime == app.runtime);
    check("server_id", server_id == app.server_id);
    check(
        "auto_restart",
        manifest.auto_restart.unwrap_or(true) == app.auto_restart,
    );
    check(
        "mem_limit_mb",
        manifest.resources.as_ref().and_then(|r| r.memory_mb) == app.mem_limit_mb,
    );
    check(
        "cpu_quota_pct",
        manifest.resources.as_ref().and_then(|r| r.cpu_percent) == app.cpu_quota_pct,
    );
    check("command", command_json == app.command);
    check("workdir", workdir == app.workdir);
    check("publish_dir", publish_dir == app.publish_dir);

    // Capacity was checked in apply_manifest (excluding this app).
    let mut report = ApplyReport {
        app_id: app.id.clone(),
        app_name: app.name.clone(),
        changed: changed.clone(),
        ..Default::default()
    };
    if !dry_run && !changed.is_empty() {
        sqlx::query(
            "UPDATE applications SET description = ?, binary_path = ?, port = ?, health_path = ?, \
             metrics_path = ?, domain = ?, runtime = ?, auto_restart = ?, server_id = ?, \
             mem_limit_mb = ?, cpu_quota_pct = ?, command = ?, workdir = ?, publish_dir = ?, \
             updated_at = datetime('now') WHERE id = ?",
        )
        .bind(&manifest.description)
        .bind(binary_path)
        .bind(port)
        .bind(health_path)
        .bind(&metrics_path)
        .bind(&manifest.domain)
        .bind(runtime)
        .bind(manifest.auto_restart.unwrap_or(true) as i64)
        .bind(server_id)
        .bind(manifest.resources.as_ref().and_then(|r| r.memory_mb))
        .bind(manifest.resources.as_ref().and_then(|r| r.cpu_percent))
        .bind(&command_json)
        .bind(&workdir)
        .bind(&publish_dir)
        .bind(&app.id)
        .execute(&state.pool)
        .await?;
        sqlx::query("UPDATE app_servers SET server_id = ?, port = ? WHERE application_id = ?")
            .bind(server_id)
            .bind(port)
            .bind(&app.id)
            .execute(&state.pool)
            .await?;
        audit::record(
            state,
            Some(org_id),
            None,
            Some(&app.id),
            "app.update",
            Some("application"),
            Some(&app.id),
            Some(&serde_json::json!({"via": "apply", "changed": changed}).to_string()),
        )
        .await?;
    }
    upsert_health(state, &app.id, manifest, health_path, dry_run).await?;
    report.pruned_aliases = sync_aliases(state, &app.id, &manifest.aliases, dry_run).await?;

    // Reload for env sync (fresh row when we just wrote).
    let current: Application = sqlx::query_as("SELECT * FROM applications WHERE id = ?")
        .bind(&app.id)
        .fetch_one(&state.pool)
        .await?;
    let (env_wrote, pruned_env, generated) =
        sync_env(state, org_id, &current, manifest, to_generate, dry_run).await?;
    report.env_wrote = env_wrote;
    report.pruned_env = pruned_env;
    report.generated_secrets = generated;
    Ok(report)
}
