//! Database schema initialization and migrations
//!
//! Handles schema version tracking and incremental migrations.

use duckdb::Connection;

use super::error::DuckdbError;
use super::in_transaction;
use super::schema::{SCHEMA, SCHEMA_VERSION};
use crate::utils::crypto::sha256_hex;

/// Initialize database schema or run pending migrations
pub fn run_migrations(conn: &Connection) -> Result<(), DuckdbError> {
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM information_schema.tables WHERE table_name = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !table_exists {
        tracing::debug!(
            "Initializing database with schema version {}",
            SCHEMA_VERSION
        );
        apply_initial_schema(conn)?;
        return Ok(());
    }

    let current_version: i32 = conn
        .query_row(
            "SELECT version FROM schema_version WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if current_version > SCHEMA_VERSION {
        return Err(DuckdbError::MigrationFailed {
            version: current_version,
            name: "version_check".to_string(),
            error: format!(
                "Database schema version {} is newer than application version {}. Upgrade the application.",
                current_version, SCHEMA_VERSION
            ),
        });
    }

    if current_version == SCHEMA_VERSION {
        tracing::debug!(
            "Database schema is up to date (version {})",
            current_version
        );
        return Ok(());
    }

    for version in (current_version + 1)..=SCHEMA_VERSION {
        tracing::debug!("Applying migration to version {}", version);
        apply_migration(conn, version)?;
    }

    Ok(())
}

fn apply_initial_schema(conn: &Connection) -> Result<(), DuckdbError> {
    let start = std::time::Instant::now();

    in_transaction(conn, |conn| {
        conn.execute_batch(SCHEMA)?;

        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        conn.execute(
            "INSERT INTO schema_version (id, version, applied_at, description) VALUES (1, ?, ?, 'Initial schema')",
            duckdb::params![SCHEMA_VERSION, now],
        )?;

        tracing::debug!(
            "Applied initial schema in {}ms",
            start.elapsed().as_millis()
        );
        Ok(())
    })
}

fn apply_migration(_conn: &Connection, version: i32) -> Result<(), DuckdbError> {
    match version {
        1 => Ok(()), // Handled by apply_initial_schema
        _ => Err(DuckdbError::MigrationFailed {
            version,
            name: "unknown".to_string(),
            error: format!("Unknown migration version: {}", version),
        }),
    }
}

#[allow(dead_code)]
/// Apply a versioned migration with transaction safety and audit logging.
///
/// Use this function in `apply_migration` match arms for incremental schema changes.
///
/// Example:
///
/// ```text
/// fn apply_migration(conn: &Connection, version: i32) -> Result<(), DuckdbError> {
///     match version {
///         1 => Ok(()),
///         2 => apply_versioned_migration(conn, 2, "add_user_index", "CREATE INDEX ..."),
///         _ => Err(...)
///     }
/// }
/// ```
fn apply_versioned_migration(
    conn: &Connection,
    version: i32,
    name: &str,
    sql: &str,
) -> Result<(), DuckdbError> {
    let start = std::time::Instant::now();

    in_transaction(conn, |conn| {
        conn.execute_batch(sql)
            .map_err(|e| DuckdbError::MigrationFailed {
                version,
                name: name.to_string(),
                error: e.to_string(),
            })?;

        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        conn.execute(
            "UPDATE schema_version SET version = ?, applied_at = ?, description = ? WHERE id = 1",
            duckdb::params![version, now, name],
        )?;

        let checksum = sha256_hex(sql);
        tracing::debug!(
            "Applied migration v{} ({}) checksum={} in {}ms",
            version,
            name,
            &checksum[..8],
            start.elapsed().as_millis()
        );
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_db() -> Connection {
        Connection::open_in_memory().expect("Failed to create in-memory database")
    }

    #[test]
    fn test_run_migrations_fresh_database() {
        let conn = create_test_db();
        let result = run_migrations(&conn);
        assert!(
            result.is_ok(),
            "Migrations should succeed on fresh database"
        );

        let version: i32 = conn
            .query_row(
                "SELECT version FROM schema_version WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .expect("Should be able to read schema version");
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn test_run_migrations_idempotent() {
        let conn = create_test_db();
        run_migrations(&conn).expect("First migration should succeed");
        let result = run_migrations(&conn);
        assert!(result.is_ok(), "Running migrations twice should succeed");
    }

    #[test]
    fn test_schema_version_recorded() {
        let conn = create_test_db();
        run_migrations(&conn).expect("Migrations should succeed");

        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |row| row.get(0))
            .expect("Should count schema_version rows");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_apply_migration_unknown_version() {
        let conn = create_test_db();
        run_migrations(&conn).expect("Initial migrations should succeed");

        let result = apply_migration(&conn, 999);
        assert!(result.is_err());

        if let Err(DuckdbError::MigrationFailed { version, .. }) = result {
            assert_eq!(version, 999);
        } else {
            panic!("Expected MigrationFailed error");
        }
    }
}
