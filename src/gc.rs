//! Artifact garbage collection: delete store blobs nothing references.
//!
//! A blob is *referenced* when its hash appears as `deployments.artifact_hash`
//! (or the never-written `previous_artifact`, included defensively) or is
//! pinned by the registry metadata table. Anything else under
//! else under `{artifact_dir}/sha256/` is unreachable — rollback can never
//! name it — and is safe to delete. Two guards keep collection conservative:
//! files younger than a grace period are skipped (a deploy stores the blob
//! *before* writing its deployment row), as are non-regular files. Crashed
//! uploads leave `.tmp-*` files behind; stale ones are swept too.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use turaes_core::Result;

use crate::audit;
use crate::state::AppState;

/// Minimum file age before it becomes collectible.
const GRACE: Duration = Duration::from_secs(3600);

/// Artifact metadata rows survive this long with no release pinning them
/// (pushed-but-never-released, superseded members) before their blob and row
/// expire. Yanked releases keep their rows, so yanked bytes stay pinned.
const ORPHAN_DAYS: i64 = 30;

/// Outcome of a collection run.
#[derive(Debug, Default)]
pub struct GcReport {
    /// Store blobs deleted.
    pub blobs_removed: usize,
    /// Bytes reclaimed from blobs.
    pub bytes_freed: u64,
    /// Stale `.tmp-*` uploads swept.
    pub tmps_swept: usize,
    /// Blobs kept (referenced or too fresh).
    pub blobs_kept: usize,
    /// Expired unpinned artifact metadata rows deleted.
    pub rows_expired: usize,
}

/// Hashes any deployment row still names, plus every blob a release pins
/// (primary artifact + bundle `files_json` members). Yanked releases keep
/// their rows, so yanked bytes stay pinned by design.
async fn referenced_hashes(state: &AppState) -> Result<HashSet<String>> {
    let mut rows: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT artifact_hash FROM deployments WHERE artifact_hash IS NOT NULL \
         UNION \
         SELECT previous_artifact FROM deployments WHERE previous_artifact IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await?;
    let release_hashes: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT hash FROM artifacts WHERE id IN (SELECT artifact_id FROM releases)",
    )
    .fetch_all(&state.pool)
    .await?;
    rows.extend(release_hashes);
    let files: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT files_json FROM releases WHERE files_json IS NOT NULL AND files_json != ''",
    )
    .fetch_all(&state.pool)
    .await?;
    for doc in files.into_iter().flatten() {
        if let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&doc) {
            for item in items {
                if let Some(h) = item.get("hash").and_then(|v| v.as_str()) {
                    rows.push(Some(h.to_string()));
                }
            }
        }
    }
    let mut out = HashSet::with_capacity(rows.len());
    for hash in rows.into_iter().flatten() {
        out.insert(hash.strip_prefix("sha256:").unwrap_or(&hash).to_string());
    }
    Ok(out)
}

/// Expire artifact metadata rows nothing pins (no release references the row
/// as primary artifact or bundle member) older than `ORPHAN_DAYS`, deleting
/// their blobs when unreferenced. Returns (rows, bytes).
async fn expire_orphans(
    state: &AppState,
    referenced: &HashSet<String>,
    dry_run: bool,
) -> Result<(usize, u64)> {
    let cutoff = format!("-{ORPHAN_DAYS} days");
    let rows: Vec<(String, String)> = sqlx::query_as(&format!(
        "SELECT id, hash FROM artifacts WHERE created_at < datetime('now', '{cutoff}')"
    ))
    .fetch_all(&state.pool)
    .await?;
    let dir = PathBuf::from(&state.cfg.runtime.artifact_dir).join("sha256");
    let mut rows_expired = 0;
    let mut bytes_freed = 0;
    for (id, hash) in rows {
        let bare = hash.strip_prefix("sha256:").unwrap_or(&hash);
        if referenced.contains(bare) {
            continue;
        }
        let path = dir.join(bare);
        let stale = path
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| !fresh_enough(t))
            .unwrap_or(true);
        if !stale {
            continue;
        }
        let bytes = path.metadata().map(|m| m.len()).unwrap_or(0);
        if !dry_run {
            let _ = std::fs::remove_file(&path);
            sqlx::query("DELETE FROM artifacts WHERE id = ?")
                .bind(&id)
                .execute(&state.pool)
                .await?;
        }
        rows_expired += 1;
        bytes_freed += bytes;
    }
    Ok((rows_expired, bytes_freed))
}

fn fresh_enough(mtime: SystemTime) -> bool {
    SystemTime::now()
        .duration_since(mtime)
        .is_ok_and(|age| age < GRACE)
}

/// `(files, bytes)` currently sitting in the store (regular files only).
pub fn store_usage(artifact_dir: &str) -> (usize, u64) {
    let dir = Path::new(artifact_dir).join("sha256");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return (0, 0),
    };
    let mut files = 0;
    let mut bytes = 0;
    for entry in entries.filter_map(|e| e.ok()) {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    (files, bytes)
}

/// Collect unreferenced blobs (and stale temp uploads). With `dry_run`, count
/// and measure but delete nothing.
pub async fn collect_artifacts(state: &AppState, dry_run: bool) -> Result<GcReport> {
    let referenced = referenced_hashes(state).await?;
    let dir = PathBuf::from(&state.cfg.runtime.artifact_dir).join("sha256");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(GcReport::default()),
        Err(e) => return Err(turaes_core::Error::Io(e)),
    };
    let mut report = GcReport::default();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) if m.is_file() => m,
            _ => continue,
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Sweep crashed uploads regardless of references.
        if name.starts_with(".tmp-") {
            let stale = meta.modified().map(|t| !fresh_enough(t)).unwrap_or(true);
            if stale {
                report.tmps_swept += 1;
                if !dry_run {
                    if let Err(e) = std::fs::remove_file(&path) {
                        tracing::warn!(path = %path.display(), error = %e, "failed to sweep temp artifact");
                        report.tmps_swept -= 1;
                    }
                }
            }
            continue;
        }
        if referenced.contains(&name) {
            report.blobs_kept += 1;
            continue;
        }
        if meta.modified().is_ok_and(fresh_enough) {
            report.blobs_kept += 1;
            continue;
        }
        report.blobs_removed += 1;
        report.bytes_freed += meta.len();
        if !dry_run {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(path = %path.display(), error = %e, "failed to collect artifact");
                report.blobs_removed -= 1;
                report.bytes_freed -= meta.len();
            }
        }
    }
    let (rows_expired, orphan_bytes) = expire_orphans(state, &referenced, dry_run).await?;
    report.rows_expired = rows_expired;
    report.bytes_freed += orphan_bytes;
    if !dry_run && (report.blobs_removed > 0 || report.tmps_swept > 0 || report.rows_expired > 0) {
        let meta = serde_json::json!({
            "blobs_removed": report.blobs_removed,
            "bytes_freed": report.bytes_freed,
            "tmps_swept": report.tmps_swept,
            "rows_expired": report.rows_expired,
        })
        .to_string();
        audit::record(
            state,
            None,
            None,
            None,
            "artifacts.gc",
            Some("artifact"),
            None,
            Some(&meta),
        )
        .await?;
    }
    Ok(report)
}
