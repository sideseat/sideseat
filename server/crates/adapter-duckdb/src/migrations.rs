//! Schema creation: the current schema, created in one step; a store at any other version is refused.
//!
//! See `sideseat_core::schema_version`: there are no upgrades, so there are no migrations.

use duckdb::Connection;
use sideseat_core::schema_version::{SchemaCheck, check};
use sideseat_ports::clock::Clock;

use super::error::DuckdbError;
use super::in_transaction;
use super::schema::{SCHEMA, SCHEMA_VERSION};

/// Create the schema in an empty database, accept the current one, refuse any other.
pub fn ensure_schema(conn: &Connection, clock: &dyn Clock) -> Result<(), DuckdbError> {
    let has_version_table: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM information_schema.tables WHERE table_name = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    let found = if has_version_table {
        conn.query_row(
            "SELECT version FROM schema_version WHERE id = 1",
            [],
            |row| row.get::<_, i32>(0),
        )
        .map(Some)
        .unwrap_or(None)
    } else {
        None
    };
    match check(found, SCHEMA_VERSION).map_err(DuckdbError::UnsupportedSchema)? {
        SchemaCheck::Current => Ok(()),
        SchemaCheck::Create => in_transaction(conn, |conn| {
            conn.execute_batch(SCHEMA)?;
            conn.execute(
                "INSERT INTO schema_version (id, version, applied_at, description) VALUES (1, ?, ?, 'schema')",
                duckdb::params![SCHEMA_VERSION, clock.now().timestamp_nanos_opt().unwrap_or(0)],
            )?;
            Ok(())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestClock;

    #[test]
    fn an_empty_database_gets_the_schema_and_reopening_keeps_it() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn, &TestClock).unwrap();
        ensure_schema(&conn, &TestClock).unwrap();
        let version: i32 = conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn a_database_at_another_version_is_refused_untouched() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (id INTEGER, version INTEGER, applied_at BIGINT, description VARCHAR);
             INSERT INTO schema_version VALUES (1, 1, 0, 'older');
             CREATE TABLE otel_spans (project_id VARCHAR);",
        )
        .unwrap();
        let error = ensure_schema(&conn, &TestClock).unwrap_err();
        assert!(matches!(
            error,
            DuckdbError::UnsupportedSchema(refusal) if refusal.found == 1 && refusal.supported == SCHEMA_VERSION
        ));
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM information_schema.tables",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2, "nothing was created or dropped");
    }
}
