//! SQLite connection pool setup and migrations.
//!
//! Opens (creating if necessary) the sqlite database and runs the embedded
//! migrations (`crates/server/migrations/`). Uses `sqlx`'s runtime-checked
//! query API (`sqlx::query`/`query_as`), not the compile-time macros (which
//! would require a live `DATABASE_URL` / `.sqlx` cache at build time).

use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: u32 = 8;
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// Open (creating if necessary) the sqlite database at `sqlite_path`, run
/// migrations, and return a ready-to-use pool.
pub async fn init_pool(sqlite_path: &str) -> anyhow::Result<SqlitePool> {
    if let Some(parent) = Path::new(sqlite_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating sqlite parent dir {parent:?}"))?;
        }
    }

    let options = SqliteConnectOptions::from_str(&format!("sqlite://{sqlite_path}"))
        .with_context(|| format!("parsing sqlite path {sqlite_path}"))?
        .create_if_missing(true)
        // WAL lets readers proceed concurrently with a writer; NORMAL is the
        // standard safe pairing with WAL.
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(BUSY_TIMEOUT);

    let pool = SqlitePoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .connect_with(options)
        .await
        .with_context(|| format!("connecting to sqlite database at {sqlite_path}"))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("running database migrations")?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn init_pool_creates_db_and_runs_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = init_pool(db_path.to_str().unwrap()).await.unwrap();

        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, 0);

        pool.close().await;
    }

    #[tokio::test]
    async fn init_pool_enables_wal_journal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("wal.db");
        let pool = init_pool(db_path.to_str().unwrap()).await.unwrap();

        let (mode,): (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");

        pool.close().await;
    }
}
