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
    Ok(pool)
}

/// Apply all pending migrations.
pub async fn migrate(pool: &Pool) -> Result<()> {
    sqlx::migrate!("../../migrations")
        .run(pool)
        .await
        .map_err(|e| Error::Internal(format!("migration failed: {e}")))?;
    Ok(())
}
