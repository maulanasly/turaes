//! Artifact garbage collection: delete store blobs no deployment references.
//!
//! A blob is *referenced* when its hash appears as `deployments.artifact_hash`
//! (or the never-written `previous_artifact`, included defensively). Anything
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
}

/// Hashes any deployment row still names.
async fn referenced_hashes(state: &AppState) -> Result<HashSet<String>> {
    let rows: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT artifact_hash FROM deployments WHERE artifact_hash IS NOT NULL \
         UNION \
         SELECT previous_artifact FROM deployments WHERE previous_artifact IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut out = HashSet::with_capacity(rows.len());
    for hash in rows.into_iter().flatten() {
        out.insert(hash.strip_prefix("sha256:").unwrap_or(&hash).to_string());
    }
    Ok(out)
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
    if !dry_run && (report.blobs_removed > 0 || report.tmps_swept > 0) {
        let meta = serde_json::json!({
            "blobs_removed": report.blobs_removed,
            "bytes_freed": report.bytes_freed,
            "tmps_swept": report.tmps_swept,
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
