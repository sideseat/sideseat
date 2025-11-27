//! Database migration system

use sqlx::SqlitePool;
use tracing::debug;

use super::schema::{INITIAL_SCHEMA, SCHEMA_VERSION};
use crate::otel::error::OtelError;

/// Run all pending migrations
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), OtelError> {
    // Check if this is a fresh database
    let table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_version'",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to check schema: {}", e)))?;

    if !table_exists {
        debug!("Initializing new database with schema version {}", SCHEMA_VERSION);
        apply_initial_schema(pool).await?;
        return Ok(());
    }

    // Get current version
    let current_version: i32 =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
            .fetch_optional(pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to get schema version: {}", e)))?
            .unwrap_or(0);

    if current_version >= SCHEMA_VERSION {
        debug!("Database schema is up to date (version {})", current_version);
        return Ok(());
    }

    // Apply incremental migrations
    for version in (current_version + 1)..=SCHEMA_VERSION {
        debug!("Applying migration to version {}", version);
        apply_migration(pool, version).await?;
    }

    Ok(())
}

/// Apply the initial schema
async fn apply_initial_schema(pool: &SqlitePool) -> Result<(), OtelError> {
    let start = std::time::Instant::now();

    // Execute schema in a transaction
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to begin transaction: {}", e)))?;

    sqlx::query(INITIAL_SCHEMA)
        .execute(&mut *tx)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to apply schema: {}", e)))?;

    // Record version
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    sqlx::query(
        "INSERT INTO schema_version (id, version, applied_at, description) VALUES (1, ?, ?, 'Initial schema')"
    )
    .bind(SCHEMA_VERSION)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to record version: {}", e)))?;

    // Record migration
    let checksum = sha256_hash(INITIAL_SCHEMA);
    let elapsed_ms = start.elapsed().as_millis() as i64;
    sqlx::query(
        "INSERT INTO schema_migrations (version, name, applied_at, checksum, execution_time_ms, success) VALUES (?, ?, ?, ?, ?, 1)"
    )
    .bind(SCHEMA_VERSION)
    .bind("initial_schema")
    .bind(now)
    .bind(&checksum)
    .bind(elapsed_ms)
    .execute(&mut *tx)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to record migration: {}", e)))?;

    // Initialize storage stats
    sqlx::query(
        "INSERT OR IGNORE INTO storage_stats (id, total_traces, total_spans, total_parquet_bytes, total_parquet_files, last_updated) VALUES (1, 0, 0, 0, 0, ?)"
    )
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to init stats: {}", e)))?;

    tx.commit().await.map_err(|e| OtelError::StorageError(format!("Failed to commit: {}", e)))?;

    debug!("Applied initial schema in {}ms", elapsed_ms);
    Ok(())
}

/// Apply a specific migration version
async fn apply_migration(_pool: &SqlitePool, version: i32) -> Result<(), OtelError> {
    // For now, we only have the initial schema
    // Future migrations would be added here
    match version {
        1 => {
            // Already handled by initial schema
            Ok(())
        }
        _ => Err(OtelError::StorageError(format!("Unknown migration version: {}", version))),
    }
}

/// Calculate SHA256 hash of migration SQL
fn sha256_hash(sql: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}
