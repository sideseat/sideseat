//! PostgreSQL migration management
//!
//! Handles schema initialization and versioned migrations.

use sqlx::PgPool;

use super::error::PostgresError;
use super::schema::{DEFAULT_DATA, SCHEMA, SCHEMA_VERSION};

/// Run all pending migrations
pub async fn run_migrations(pool: &PgPool) -> Result<(), PostgresError> {
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

    // Apply schema
    sqlx::query(SCHEMA).execute(pool).await?;

    // Apply default data
    sqlx::query(DEFAULT_DATA).execute(pool).await?;

    // Record schema version
    sqlx::query(
        "INSERT INTO schema_version (id, version, applied_at, description)
         VALUES (1, $1, $2, 'Initial schema')
         ON CONFLICT (id) DO UPDATE SET version = $1, applied_at = $2",
    )
    .bind(SCHEMA_VERSION)
    .bind(now)
    .execute(pool)
    .await?;

    tracing::debug!("PostgreSQL schema v{} applied successfully", SCHEMA_VERSION);
    Ok(())
}

/// Apply a specific versioned migration
///
/// Add new migrations here as the schema evolves.
/// Currently no versioned migrations exist - schema v1 is applied via SCHEMA constant.
#[allow(unused_variables, clippy::match_single_binding)]
async fn apply_versioned_migration(pool: &PgPool, version: i32) -> Result<(), PostgresError> {
    let start = std::time::Instant::now();
    let now = chrono::Utc::now().timestamp();

    // Add future migrations here as match arms:
    let (name, sql): (&str, &str) = match version {
        // Example:
        // 2 => ("add_some_table", "CREATE TABLE ..."),
        _ => {
            return Err(PostgresError::MigrationFailed {
                version,
                name: "unknown".to_string(),
                error: format!("No migration defined for version {}", version),
            });
        }
    };

    // Execute migration (unreachable until we add migrations above)
    #[allow(unreachable_code)]
    {
        sqlx::query(sql)
            .execute(pool)
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
             VALUES ($1, $2, $3, $4, $5, TRUE)",
        )
        .bind(version)
        .bind(name)
        .bind(now)
        .bind(compute_checksum(sql))
        .bind(elapsed)
        .execute(pool)
        .await?;

        // Update schema version
        sqlx::query("UPDATE schema_version SET version = $1, applied_at = $2 WHERE id = 1")
            .bind(version)
            .bind(now)
            .execute(pool)
            .await?;

        tracing::debug!(
            "PostgreSQL migration v{} ({}) applied in {}ms",
            version,
            name,
            elapsed
        );
        Ok(())
    }
}

fn compute_checksum(sql: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sql.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}
