//! SQLite backups: on-demand snapshots, pre-migration snapshots and restore.
//!
//! Snapshots use `VACUUM INTO`, which copies a live database transactionally
//! without stopping turaes. Files are timestamped (`turaes-<UTC>.db` for manual
//! snapshots, `pre-migration-<UTC>.db` at boot) and pruned to the configured
//! retention. Restore is a file copy and must run while turaes is stopped
//! (the CLI enforces this with `--force`, i.e. "turaes is stopped, proceed").

use std::path::{Path, PathBuf};

use turaes_core::config::Config;
use turaes_core::db::Pool;
use turaes_core::{Error, Result};

/// Magic bytes at the start of every SQLite database file.
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// Resolve the filesystem path of the configured SQLite database.
/// Only `sqlite://<path>[?...]` URLs are supported.
pub fn db_path(database_url: &str) -> Result<PathBuf> {
    let stripped = database_url
        .strip_prefix("sqlite://")
        .ok_or_else(|| Error::Config(format!("unsupported database url '{database_url}'")))?;
    let path = stripped.split('?').next().unwrap_or(stripped);
    if path.is_empty() {
        return Err(Error::Config("database url has an empty path".into()));
    }
    Ok(PathBuf::from(path))
}

/// UTC timestamp compact enough to sort chronologically as a filename.
fn timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

/// Copy the live database at `pool` into `dest` (parent dirs are created).
pub async fn snapshot_to(pool: &Pool, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(Error::Io)?;
        #[cfg(unix)]
        {
            // Snapshots contain sealed secrets and token hashes: owner-only.
            use std::os::unix::fs::PermissionsExt;
            let _ =
                tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).await;
        }
    }
    // The destination is operator configuration, not user input; quote it for
    // SQL by doubling single quotes (bound parameters are not accepted here).
    let quoted = dest.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{quoted}'"))
        .execute(pool)
        .await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(Error::Io)?;
    }
    Ok(())
}

/// Take a timestamped snapshot into the configured backup dir, then prune.
/// Returns the snapshot path and the number of pruned files.
pub async fn snapshot_now(cfg: &Config, pool: &Pool) -> Result<(PathBuf, usize)> {
    let dest = Path::new(&cfg.backup.dir).join(format!("turaes-{}.db", timestamp()));
    snapshot_to(pool, &dest).await?;
    let pruned = prune_dir(Path::new(&cfg.backup.dir), cfg.backup.retain)?;
    tracing::info!(path = %dest.display(), pruned, "database snapshot written");
    Ok((dest, pruned))
}

/// Snapshot taken at boot before migrations run (fail-closed: a boot that
/// cannot snapshot must not run destructive migrations).
pub async fn snapshot_before_migrate(cfg: &Config, pool: &Pool) -> Result<PathBuf> {
    let dest = Path::new(&cfg.backup.dir).join(format!("pre-migration-{}.db", timestamp()));
    snapshot_to(pool, &dest).await?;
    let pruned = prune_dir(Path::new(&cfg.backup.dir), cfg.backup.retain)?;
    tracing::info!(path = %dest.display(), pruned, "pre-migration snapshot written");
    Ok(dest)
}

/// Delete all but the newest `retain` `*.db` snapshots in `dir`.
/// Returns the number of files removed. Always keeps at least one.
pub fn prune_dir(dir: &Path, retain: usize) -> Result<usize> {
    let keep = retain.max(1);
    let entries = std::fs::read_dir(dir).map_err(Error::Io)?;
    let mut snaps: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "db"))
        .collect();
    snaps.sort();
    if snaps.len() <= keep {
        return Ok(0);
    }
    let mut removed = 0;
    for old in snaps.iter().take(snaps.len() - keep) {
        match std::fs::remove_file(old) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(path = %old.display(), error = %e, "failed to prune snapshot"),
        }
    }
    Ok(removed)
}

/// Verify `path` looks like a SQLite database (magic header).
pub fn verify_sqlite(path: &Path) -> Result<()> {
    let bytes = std::fs::read(path).map_err(Error::Io)?;
    if bytes.len() >= SQLITE_MAGIC.len() && &bytes[..SQLITE_MAGIC.len()] == SQLITE_MAGIC {
        Ok(())
    } else {
        Err(Error::BadRequest(format!(
            "{} is not a SQLite database",
            path.display()
        )))
    }
}

/// Restore the database from a snapshot file. The destination must be the
/// configured database path; callers must stop turaes first.
pub fn restore_file(src: &Path, dest: &Path) -> Result<()> {
    verify_sqlite(src)?;
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }
    }
    // Refuse to clobber the destination with itself.
    if same_file(src, dest) {
        return Err(Error::BadRequest(
            "snapshot and destination are the same file".into(),
        ));
    }
    // Refuse while the service holds the database: a live writer's WAL would
    // replay stale pages over the restored file on next boot. Best-effort on
    // non-systemd platforms (the --force contract covers the rest).
    #[cfg(target_os = "linux")]
    {
        if std::process::Command::new("systemctl")
            .arg("is-active")
            .arg("--quiet")
            .arg("turaes")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Err(Error::BadRequest(
                "turaes is still running (systemctl stop turaes first)".into(),
            ));
        }
    }
    std::fs::copy(src, dest).map_err(Error::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o600))
            .map_err(Error::Io)?;
    }
    // Drop WAL sidecars next to the destination: they belong to the
    // pre-restore generation and would replay over the restored file.
    for ext in ["-wal", "-shm", "-journal"] {
        let mut sidecar = dest.as_os_str().to_owned();
        sidecar.push(ext);
        let _ = std::fs::remove_file(std::path::Path::new(&sidecar));
    }
    Ok(())
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) => ma.dev() == mb.dev() && ma.ino() == mb.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(_a: &Path, _b: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool(dir: &Path) -> Pool {
        let url = format!("sqlite://{}/t.db?mode=rwc", dir.display());
        let pool = turaes_core::db::connect(&url).await.expect("connect");
        sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t (v) VALUES ('hello')")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[test]
    fn db_path_strips_scheme_and_query() {
        assert_eq!(
            db_path("sqlite:///var/lib/turaes/turaes.db?mode=rwc").unwrap(),
            PathBuf::from("/var/lib/turaes/turaes.db")
        );
        assert!(db_path("postgres://x").is_err());
        assert!(db_path("sqlite://?mode=rwc").is_err());
    }

    #[tokio::test]
    async fn snapshot_round_trips_rows() {
        let dir = tempfile::tempdir().unwrap();
        let pool = test_pool(dir.path()).await;
        let dest = dir.path().join("backups").join("turaes-test.db");
        snapshot_to(&pool, &dest).await.unwrap();
        assert!(dest.is_file());

        let url = format!("sqlite://{}", dest.display());
        let check = turaes_core::db::connect(&url).await.unwrap();
        let v: String = sqlx::query_scalar("SELECT v FROM t WHERE id = 1")
            .fetch_one(&check)
            .await
            .unwrap();
        assert_eq!(v, "hello");
    }

    #[test]
    fn prune_keeps_newest_n() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "turaes-20260101T000000Z.db",
            "turaes-20260102T000000Z.db",
            "turaes-20260103T000000Z.db",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let removed = prune_dir(dir.path(), 2).unwrap();
        assert_eq!(removed, 1);
        assert!(!dir.path().join("turaes-20260101T000000Z.db").exists());
        assert!(dir.path().join("turaes-20260102T000000Z.db").exists());
        assert!(dir.path().join("turaes-20260103T000000Z.db").exists());
        assert!(dir.path().join("notes.txt").exists());
    }

    #[test]
    fn verify_rejects_non_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.db");
        std::fs::write(&bad, b"not a database").unwrap();
        assert!(verify_sqlite(&bad).is_err());
    }

    #[tokio::test]
    async fn restore_replaces_destination() {
        let dir = tempfile::tempdir().unwrap();
        let pool = test_pool(dir.path()).await;
        let snap = dir.path().join("snap.db");
        snapshot_to(&pool, &snap).await.unwrap();

        let dest = dir.path().join("restored.db");
        std::fs::write(&dest, b"junk").unwrap();
        restore_file(&snap, &dest).unwrap();
        verify_sqlite(&dest).unwrap();

        // Restoring a non-database is refused and leaves the file alone.
        let bad = dir.path().join("bad.db");
        std::fs::write(&bad, b"junk").unwrap();
        assert!(restore_file(&bad, &dest).is_err());
        verify_sqlite(&dest).unwrap();
    }

    #[cfg(unix)]
    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn snapshots_are_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let pool = test_pool(dir.path()).await;
        let dest = dir.path().join("backups").join("turaes-test.db");
        snapshot_to(&pool, &dest).await.unwrap();
        assert_eq!(mode_of(&dest), 0o600);
    }

    #[test]
    #[cfg(unix)]
    fn restore_sets_owner_only_and_drops_wal_sidecars() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        // Minimal valid SQLite header so verify_sqlite passes.
        let snap = dir.path().join("snap.db");
        {
            let mut f = std::fs::File::create(&snap).unwrap();
            f.write_all(b"SQLite format 3\0").unwrap();
            f.write_all(&[0u8; 100]).unwrap();
        }
        let dest = dir.path().join("live.db");
        std::fs::write(&dest, b"junk").unwrap();
        // Stale sidecars from the pre-restore generation must not survive.
        std::fs::write(dir.path().join("live.db-wal"), b"stale").unwrap();
        std::fs::write(dir.path().join("live.db-shm"), b"stale").unwrap();
        restore_file(&snap, &dest).unwrap();
        assert_eq!(mode_of(&dest), 0o600);
        assert!(!dir.path().join("live.db-wal").exists());
        assert!(!dir.path().join("live.db-shm").exists());
    }
}
