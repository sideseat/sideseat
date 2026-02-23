//! PostgreSQL migration management
//!
//! Handles schema initialization and versioned migrations.

use sqlx::PgPool;

use super::error::PostgresError;
use super::schema::{DEFAULT_DATA, SCHEMA, SCHEMA_VERSION};

/// Run all pending migrations.
///
/// Uses `pg_advisory_lock` to prevent concurrent migration execution
/// across multiple application instances.
pub async fn run_migrations(pool: &PgPool) -> Result<(), PostgresError> {
    // Acquire advisory lock to prevent concurrent migrations.
    // Lock ID 0x5364_5365_6174 ("SdSeat" in hex) avoids collision with other apps.
    const MIGRATION_LOCK_ID: i64 = 0x5364_5365;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK_ID)
        .execute(pool)
        .await?;

    let result = run_migrations_inner(pool).await;

    // Always release the advisory lock, even on error
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_ID)
        .execute(pool)
        .await;

    result
}

async fn run_migrations_inner(pool: &PgPool) -> Result<(), PostgresError> {
    // Check if schema_version table exists
    let table_exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = 'public'
            AND table_name = 'schema_version'
        )
        "#,
    )
    .fetch_one(pool)
    .await?;

    if !table_exists {
        // Fresh database - apply initial schema
        tracing::debug!("Applying initial PostgreSQL schema v{}", SCHEMA_VERSION);
        apply_initial_schema(pool).await?;
        return Ok(());
    }

    // Get current version
    let current_version: Option<i32> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
            .fetch_optional(pool)
            .await?;

    match current_version {
        None => {
            // Table exists but no version row - apply schema
            tracing::debug!("Applying initial PostgreSQL schema v{}", SCHEMA_VERSION);
            apply_initial_schema(pool).await?;
        }
        Some(v) if v < SCHEMA_VERSION => {
            // Run incremental migrations
            tracing::debug!(
                "Migrating PostgreSQL schema from v{} to v{}",
                v,
                SCHEMA_VERSION
            );
            for version in (v + 1)..=SCHEMA_VERSION {
                apply_versioned_migration(pool, version).await?;
            }
        }
        Some(v) if v > SCHEMA_VERSION => {
            tracing::warn!(
                "PostgreSQL schema version {} is newer than application version {}. This may cause issues.",
                v,
                SCHEMA_VERSION
            );
        }
        _ => {
            tracing::debug!("PostgreSQL schema is up to date (v{})", SCHEMA_VERSION);
        }
    }

    Ok(())
}

/// Apply the initial schema
async fn apply_initial_schema(pool: &PgPool) -> Result<(), PostgresError> {
    let now = chrono::Utc::now().timestamp();

    let mut tx = pool.begin().await?;

    // Apply schema (split by semicolons — PostgreSQL doesn't support multiple statements per query)
    for statement in SCHEMA.split(';').filter(|s| !s.trim().is_empty()) {
        sqlx::query(statement.trim()).execute(&mut *tx).await?;
    }

    // Apply default data
    for statement in DEFAULT_DATA.split(';').filter(|s| !s.trim().is_empty()) {
        sqlx::query(statement.trim()).execute(&mut *tx).await?;
    }

    // Record schema version
    sqlx::query(
        "INSERT INTO schema_version (id, version, applied_at, description)
         VALUES (1, $1, $2, 'Initial schema')
         ON CONFLICT (id) DO UPDATE SET version = $1, applied_at = $2",
    )
    .bind(SCHEMA_VERSION)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    tracing::debug!("PostgreSQL schema v{} applied successfully", SCHEMA_VERSION);
    Ok(())
}

/// Apply a specific versioned migration within a transaction.
///
/// PostgreSQL supports DDL inside transactions, so the entire migration
/// (DDL + metadata update) is atomic. Uses IF NOT EXISTS for idempotency.
async fn apply_versioned_migration(pool: &PgPool, version: i32) -> Result<(), PostgresError> {
    let start = std::time::Instant::now();
    let now = chrono::Utc::now().timestamp();

    // Add future migrations here as match arms:
    let (name, sql): (&str, &str) = match version {
        2 => (
            "add_hash_algo_to_files",
            "ALTER TABLE files ADD COLUMN IF NOT EXISTS hash_algo TEXT NOT NULL DEFAULT 'sha256'",
        ),
        _ => {
            return Err(PostgresError::MigrationFailed {
                version,
                name: "unknown".to_string(),
                error: format!("No migration defined for version {}", version),
            });
        }
    };

    let mut tx = pool.begin().await?;

    sqlx::query(sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| PostgresError::MigrationFailed {
            version,
            name: name.to_string(),
            error: e.to_string(),
        })?;

    let elapsed = start.elapsed().as_millis() as i64;

    // Record migration
    sqlx::query(
        "INSERT INTO schema_migrations (version, name, applied_at, checksum, execution_time_ms, success)
         VALUES ($1, $2, $3, $4, $5, TRUE)
         ON CONFLICT (version) DO NOTHING",
    )
    .bind(version)
    .bind(name)
    .bind(now)
    .bind(compute_checksum(sql))
    .bind(elapsed)
    .execute(&mut *tx)
    .await?;

    // Update schema version
    sqlx::query("UPDATE schema_version SET version = $1, applied_at = $2 WHERE id = 1")
        .bind(version)
        .bind(now)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    tracing::debug!(
        "PostgreSQL migration v{} ({}) applied in {}ms",
        version,
        name,
        elapsed
    );
    Ok(())
}

fn compute_checksum(sql: &str) -> String {
    crate::utils::crypto::sha256_hex(sql)
}
