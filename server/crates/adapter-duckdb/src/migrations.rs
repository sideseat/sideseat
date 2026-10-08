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
        SchemaCheck::Current => check_layout(conn),
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

/// Refuse a store at the current version whose layout is not the one [`SCHEMA`] creates.
///
/// The version says which build created a store, not that its tables match this build's: the layout changes
/// within a version too, and a store created before such a change would be read and written as if it had the
/// new columns and indexes - writes failing on a missing column, reads scanning tables whose indexes were never
/// built. So the store's tables, columns and indexes are compared with a fresh schema's, built in memory.
fn check_layout(conn: &Connection) -> Result<(), DuckdbError> {
    let expected = {
        let fresh = Connection::open_in_memory()?;
        fresh.execute_batch(SCHEMA)?;
        layout(&fresh)?
    };
    let found = layout(conn)?;
    let differences: Vec<String> = expected
        .difference(&found)
        .map(|item| format!("missing {item}"))
        .chain(
            found
                .difference(&expected)
                .map(|item| format!("unexpected {item}")),
        )
        .take(5)
        .collect();
    if differences.is_empty() {
        return Ok(());
    }
    Err(DuckdbError::LayoutMismatch(
        sideseat_core::schema_version::LayoutMismatch {
            version: SCHEMA_VERSION,
            differences,
        },
    ))
}

/// Every column with its type, and every index with its definition.
fn layout(conn: &Connection) -> Result<std::collections::BTreeSet<String>, DuckdbError> {
    let mut items = std::collections::BTreeSet::new();
    for sql in [
        // The position too: writes append by position, so a column in another place is another layout.
        "SELECT 'column ' || table_name || '.' || column_name || ' ' || data_type || ' at ' || ordinal_position \
         FROM information_schema.columns WHERE table_schema = 'main'",
        "SELECT 'index ' || index_name || ' ' || coalesce(sql, '') FROM duckdb_indexes() \
         WHERE schema_name = 'main'",
    ] {
        let mut statement = conn.prepare(sql)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            items.insert(row?);
        }
    }
    Ok(items)
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

    /// A store at this version created with an earlier layout - here, the term table before it carried
    /// `ingested_at` - is refused with the reset instruction rather than written to.
    #[test]
    fn a_store_with_an_earlier_layout_of_this_version_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn, &TestClock).unwrap();
        conn.execute_batch(
            "ALTER TABLE span_terms DROP COLUMN ingested_at; DROP INDEX idx_spans_span;",
        )
        .unwrap();
        let error = ensure_schema(&conn, &TestClock).unwrap_err();
        let DuckdbError::LayoutMismatch(mismatch) = error else {
            panic!("expected a layout refusal, got {error}");
        };
        assert_eq!(mismatch.version, SCHEMA_VERSION);
        let message = mismatch.to_string();
        assert!(message.contains("span_terms.ingested_at"), "{message}");
        assert!(message.contains("idx_spans_span"), "{message}");
        assert!(message.contains("sideseat system prune"), "{message}");
    }

    /// The same columns in another order are another layout: writes append by position.
    #[test]
    fn a_store_whose_columns_are_in_another_order_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn, &TestClock).unwrap();
        conn.execute_batch(
            "DROP TABLE log_terms; CREATE TABLE log_terms (log_digest VARCHAR NOT NULL, project_id VARCHAR NOT NULL, \
             ordinal UINTEGER NOT NULL, field VARCHAR NOT NULL, term VARCHAR NOT NULL, truncated BOOLEAN NOT NULL, \
             ingested_at TIMESTAMP NOT NULL);",
        )
        .unwrap();
        assert!(matches!(
            ensure_schema(&conn, &TestClock),
            Err(DuckdbError::LayoutMismatch(_))
        ));
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
