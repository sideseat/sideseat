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

/// A datapoint's identity, so a re-delivered metrics payload stops accumulating duplicate rows.
///
/// Existing rows get `''`, which is honest rather than convenient: nothing recorded before this
/// version carries the fields' hash, and a value invented in SQL would not match the one Rust
/// computes. Legacy rows therefore keep the old behaviour among themselves, and every row written
/// from here on has an identity. The 90-day retention window ages the untagged ones out.
///
/// The `NOT NULL` matters: a fresh schema declares it, so without it an upgraded database carried a
/// nullable column and the two schemas differed for one version. Backfilling and stopping is the same
/// half-kept invariant the upgrade tests exist to catch.
///
/// Backfilled by a column `DEFAULT` rather than an `UPDATE`, then the default dropped so the shape matches
/// the fresh schema exactly. An `UPDATE` followed by `SET NOT NULL` in one transaction is refused by
/// DuckDB ("Cannot create index with outstanding updates"), so on a *populated* table the migration failed
/// outright. Only a test that populates the table before migrating finds that.
///
/// The indexes are dropped and recreated around the `ALTER`, because DuckDB refuses to alter a table that
/// has dependents at all: "Cannot alter entry because there are entries that depend on it". Every real v1
/// database has these five indexes, so without this the migration failed on *every* database it exists to
/// serve - and a test against a bare table would never have noticed.
const MIGRATION_V2: &str = r#"DROP INDEX IF EXISTS idx_metrics_project_ts;
DROP INDEX IF EXISTS idx_metrics_project_name;
DROP INDEX IF EXISTS idx_metrics_project_name_ts;
DROP INDEX IF EXISTS idx_metrics_exemplar_trace;
DROP INDEX IF EXISTS idx_metrics_session;
ALTER TABLE otel_metrics ADD COLUMN datapoint_id VARCHAR DEFAULT '';
ALTER TABLE otel_metrics ALTER COLUMN datapoint_id SET NOT NULL;
ALTER TABLE otel_metrics ALTER COLUMN datapoint_id DROP DEFAULT;
ALTER TABLE otel_metrics ADD COLUMN scope_attributes JSON;
ALTER TABLE otel_metrics ADD COLUMN scope_schema_url VARCHAR;
ALTER TABLE otel_metrics ADD COLUMN resource_schema_url VARCHAR;
CREATE INDEX IF NOT EXISTS idx_metrics_project_ts ON otel_metrics(project_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_project_name ON otel_metrics(project_id, metric_name);
CREATE INDEX IF NOT EXISTS idx_metrics_project_name_ts ON otel_metrics(project_id, metric_name, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_exemplar_trace ON otel_metrics(project_id, exemplar_trace_id);
CREATE INDEX IF NOT EXISTS idx_metrics_session ON otel_metrics(project_id, session_id);
"#;

fn apply_migration(conn: &Connection, version: i32) -> Result<(), DuckdbError> {
    match version {
        1 => Ok(()), // Handled by apply_initial_schema
        2 => apply_versioned_migration(conn, 2, "metric_datapoint_identity", MIGRATION_V2),
        _ => Err(DuckdbError::MigrationFailed {
            version,
            name: "unknown".to_string(),
            error: format!("Unknown migration version: {}", version),
        }),
    }
}

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
    use crate::data::duckdb::schema::SCHEMA;

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

    /// A v1 database walked forward has the same columns, **in the same physical order**, as a fresh one.
    ///
    /// Order is the point, and a membership comparison misses it entirely. The writer is DuckDB's
    /// `Appender`, which is positional, and `ALTER TABLE ADD COLUMN` can only append - so a column declared
    /// mid-table in the fresh schema sits at the end of an upgraded one, and the appender then shifts every
    /// value by one column on exactly the databases a migration exists to serve. That is not a subtle
    /// difference in behaviour; metrics ingestion fails outright, or worse, stores each value under its
    /// neighbour's name.
    #[test]
    fn a_v1_database_upgrades_to_the_same_column_order_as_a_fresh_one() {
        fn columns(conn: &Connection, table: &str) -> Vec<(String, String)> {
            // `ORDER BY column_index`, not sorted by name: the physical order is what the appender uses.
            let mut stmt = conn
                .prepare(
                    "SELECT column_name, data_type FROM duckdb_columns() \
                     WHERE table_name = ? ORDER BY column_index",
                )
                .expect("prepare");
            let rows = stmt
                .query_map([table], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .expect("query");
            rows.map(|r| r.expect("row")).collect()
        }

        let fresh = Connection::open_in_memory().expect("fresh");
        fresh.execute_batch(SCHEMA).expect("fresh schema");

        // A v1 database: the current schema minus what migration 2 adds.
        let upgraded = Connection::open_in_memory().expect("upgraded");
        upgraded.execute_batch(SCHEMA).expect("base schema");
        // The indexes depend on the table, so they go first - DuckDB refuses to alter a table with
        // dependents. Recreated after, since the fresh schema declares them.
        upgraded
            .execute_batch(
                "DROP INDEX idx_metrics_project_ts;
                 DROP INDEX idx_metrics_project_name;
                 DROP INDEX idx_metrics_project_name_ts;
                 DROP INDEX idx_metrics_exemplar_trace;
                 DROP INDEX idx_metrics_session;
                 ALTER TABLE otel_metrics DROP COLUMN datapoint_id;
                 ALTER TABLE otel_metrics DROP COLUMN scope_attributes;
                 ALTER TABLE otel_metrics DROP COLUMN scope_schema_url;
                 ALTER TABLE otel_metrics DROP COLUMN resource_schema_url;
                 -- Recreated, because a real v1 database has them and DuckDB refuses to alter a table
                 -- with dependents: the migration has to handle that itself.
                 CREATE INDEX idx_metrics_project_ts ON otel_metrics(project_id, timestamp DESC);
                 CREATE INDEX idx_metrics_project_name ON otel_metrics(project_id, metric_name);
                 CREATE INDEX idx_metrics_project_name_ts ON otel_metrics(project_id, metric_name, timestamp DESC);
                 CREATE INDEX idx_metrics_exemplar_trace ON otel_metrics(project_id, exemplar_trace_id);
                 CREATE INDEX idx_metrics_session ON otel_metrics(project_id, session_id);",
            )
            .expect("reduce to the v1 shape");
        // A pre-existing row, to prove the backfill reaches it rather than leaving a null the NOT NULL
        // would then refuse.
        upgraded
            .execute_batch(
                "INSERT INTO otel_metrics (project_id, metric_name, metric_type, timestamp) \
                 VALUES ('p1', 'legacy.metric', 'gauge', TIMESTAMP '2026-01-01 00:00:00');",
            )
            .expect("legacy row");

        apply_migration(&upgraded, 2).expect("migration 2");

        assert_eq!(
            columns(&upgraded, "otel_metrics"),
            columns(&fresh, "otel_metrics"),
            "an upgraded database's otel_metrics columns differ in name, type or *position* from a fresh \
             one's - and the metrics writer is positional, so a position difference silently writes every \
             value into the wrong column"
        );

        // And the legacy row came through with a value rather than a null.
        let legacy: String = upgraded
            .query_row(
                "SELECT datapoint_id FROM otel_metrics WHERE metric_name = 'legacy.metric'",
                [],
                |row| row.get(0),
            )
            .expect("legacy row survives the migration");
        assert_eq!(
            legacy, "",
            "a legacy datapoint has no computable identity, so it carries the empty one - not a null, \
             which the fresh schema's NOT NULL would refuse"
        );
    }
}
