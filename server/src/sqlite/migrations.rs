//! Database migration system
//!
//! Handles schema versioning and incremental migrations.
//! Version 1 is the initial schema - future migrations will be added here.

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tracing::debug;

use super::error::SqliteError;
use super::schema::{SCHEMA, SCHEMA_VERSION};

/// Run all pending migrations
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), SqliteError> {
    // Check if this is a fresh database
    let table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_version'",
    )
    .fetch_one(pool)
    .await?;

    if !table_exists {
        debug!("Initializing new database with schema version {}", SCHEMA_VERSION);
        apply_initial_schema(pool).await?;
        return Ok(());
    }

    // Get current version
    let current_version: i32 =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
            .fetch_optional(pool)
            .await?
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

/// Apply the initial schema (version 1)
async fn apply_initial_schema(pool: &SqlitePool) -> Result<(), SqliteError> {
    let start = std::time::Instant::now();

    // Execute schema in a transaction
    let mut tx = pool.begin().await?;

    sqlx::query(SCHEMA).execute(&mut *tx).await?;

    // Record version
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    sqlx::query(
        "INSERT INTO schema_version (id, version, applied_at, description) VALUES (1, ?, ?, 'Initial schema')",
    )
    .bind(SCHEMA_VERSION)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    // Record migration
    let checksum = sha256_hash(SCHEMA);
    let elapsed_ms = start.elapsed().as_millis() as i64;
    sqlx::query(
        "INSERT INTO schema_migrations (version, name, applied_at, checksum, execution_time_ms, success) VALUES (?, ?, ?, ?, ?, 1)",
    )
    .bind(SCHEMA_VERSION)
    .bind("initial_schema")
    .bind(now)
    .bind(&checksum)
    .bind(elapsed_ms)
    .execute(&mut *tx)
    .await?;

    // Initialize storage stats
    sqlx::query(
        "INSERT OR IGNORE INTO storage_stats (id, total_traces, total_spans, total_parquet_bytes, total_parquet_files, last_updated) VALUES (1, 0, 0, 0, 0, ?)",
    )
    .bind(now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    debug!("Applied initial schema in {}ms", elapsed_ms);
    Ok(())
}

/// Apply a specific migration version
async fn apply_migration(_pool: &SqlitePool, version: i32) -> Result<(), SqliteError> {
    match version {
        1 => {
            // Already handled by initial schema
            Ok(())
        }
        // Future migrations go here:
        // 2 => apply_versioned_migration(pool, 2, "migration_name", MIGRATION_V2).await,
        _ => Err(SqliteError::MigrationFailed {
            version,
            name: "unknown".to_string(),
            error: format!("Unknown migration version: {}", version),
        }),
    }
}

/// Apply a versioned migration with tracking
#[allow(dead_code)]
async fn apply_versioned_migration(
    pool: &SqlitePool,
    version: i32,
    name: &str,
    sql: &str,
) -> Result<(), SqliteError> {
    let start = std::time::Instant::now();

    let mut tx = pool.begin().await?;

    // Execute migration SQL (split by semicolons for SQLite compatibility)
    for statement in sql.split(';').filter(|s| !s.trim().is_empty()) {
        let trimmed = statement.trim();
        if !trimmed.is_empty() {
            sqlx::query(trimmed).execute(&mut *tx).await.map_err(|e| {
                SqliteError::MigrationFailed {
                    version,
                    name: name.to_string(),
                    error: format!(
                        "Failed at statement: {} - {}",
                        &trimmed[..trimmed.len().min(50)],
                        e
                    ),
                }
            })?;
        }
    }

    // Update version
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    sqlx::query(
        "UPDATE schema_version SET version = ?, applied_at = ?, description = ? WHERE id = 1",
    )
    .bind(version)
    .bind(now)
    .bind(name)
    .execute(&mut *tx)
    .await?;

    // Record migration
    let checksum = sha256_hash(sql);
    let elapsed_ms = start.elapsed().as_millis() as i64;
    sqlx::query(
        "INSERT INTO schema_migrations (version, name, applied_at, checksum, execution_time_ms, success) VALUES (?, ?, ?, ?, ?, 1)",
    )
    .bind(version)
    .bind(name)
    .bind(now)
    .bind(&checksum)
    .bind(elapsed_ms)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    debug!("Applied migration v{} ({}) in {}ms", version, name, elapsed_ms);
    Ok(())
}

/// Calculate SHA256 hash of migration SQL
fn sha256_hash(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}
