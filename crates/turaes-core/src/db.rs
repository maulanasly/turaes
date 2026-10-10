//! SQLite pool setup and migrations.
//!
//! Migrations are embedded at compile time from the workspace `migrations/`
//! directory, so a release binary can initialise an empty database with no
//! external files.

use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::error::{Error, Result};

/// Shared SQLite connection pool.
pub type Pool = sqlx::SqlitePool;

/// Open (creating if needed) a WAL-mode SQLite pool.
pub async fn connect(url: &str) -> Result<Pool> {
    let options = SqliteConnectOptions::from_str(url)
        .map_err(|e| Error::Config(format!("invalid database url '{url}': {e}")))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await?;
    // The database holds sealed secrets and token hashes: keep it
    // owner-only regardless of umask, on creation and every open (heals
    // pre-existing world-readable files).
    if let Ok(path) = db_file_path(url) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
    Ok(pool)
}

/// Filesystem path for a `sqlite://` URL, if it names a local file.
fn db_file_path(url: &str) -> Result<std::path::PathBuf> {
    let path = url
        .strip_prefix("sqlite://")
        .ok_or_else(|| Error::Config(format!("invalid database url '{url}'")))?;
    let path = path.split('?').next().unwrap_or(path);
    if path.is_empty() || path == ":memory:" {
        return Err(Error::Config(format!("invalid database url '{url}'")));
    }
    Ok(std::path::PathBuf::from(path))
}

/// Apply all pending migrations.
pub async fn migrate(pool: &Pool) -> Result<()> {
    sqlx::migrate!("../../migrations")
        .run(pool)
        .await
        .map_err(|e| Error::Internal(format!("migration failed: {e}")))?;
    Ok(())
}
