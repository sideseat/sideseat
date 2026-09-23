//! Database schema initialization and migrations.
//!
//! Handles schema version tracking and incremental migrations.

use duckdb::Connection;

use super::error::DuckdbError;
use super::in_transaction;
use super::schema::{SCHEMA, SCHEMA_VERSION};
use sideseat_core::migration::{MigrationRun, plan_migrations};
use sideseat_core::utils::crypto::sha256_hex;
use sideseat_ports::clock::Clock;

/// Initialize database schema or run pending migrations
pub fn run_migrations(conn: &Connection, clock: &dyn Clock) -> Result<(), DuckdbError> {
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
        apply_initial_schema(conn, clock)?;
        return Ok(());
    }

    let current_version: i32 = conn
        .query_row(
            "SELECT version FROM schema_version WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    match migration_run(Some(current_version))? {
        MigrationRun::Initialize { .. } => apply_initial_schema(conn, clock)?,
        MigrationRun::UpToDate { version } => {
            tracing::debug!("Database schema is up to date (version {})", version);
        }
        MigrationRun::Apply(steps) => {
            for step in steps {
                tracing::debug!(
                    "Applying migration to version {} ({})",
                    step.version,
                    step.name
                );
                apply_migration(conn, step.version, clock)?;
            }
        }
    }

    Ok(())
}

fn apply_initial_schema(conn: &Connection, clock: &dyn Clock) -> Result<(), DuckdbError> {
    let start = std::time::Instant::now();

    in_transaction(conn, |conn| {
        conn.execute_batch(SCHEMA)?;

        let now = clock.now().timestamp_nanos_opt().unwrap_or(0);
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

/// The one migration: v1 to current, directly.
///
/// Nothing above v1 was ever deployed, so the intermediate steps this file briefly carried (metric
/// datapoint identity as v2, exemplars as v3, span scope as v4) described upgrades no real database
/// could need - and each step replayed its own index dance. Consolidated on the project's standing
/// rule: the schema is at version 2, and there is one migration.
///
/// The index drop/recreate around each table's `ALTER`s is load-bearing: DuckDB refuses to alter a
/// table with dependents at all, and every real v1 database has these indexes - a test against a bare
/// table would never notice. `datapoint_id` gets the DEFAULT/`SET NOT NULL`/`DROP DEFAULT` dance
/// because DuckDB refuses `UPDATE` + `SET NOT NULL` in one transaction.
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
ALTER TABLE otel_metrics ADD COLUMN exemplars JSON;
CREATE INDEX IF NOT EXISTS idx_metrics_project_ts ON otel_metrics(project_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_project_name ON otel_metrics(project_id, metric_name);
CREATE INDEX IF NOT EXISTS idx_metrics_project_name_ts ON otel_metrics(project_id, metric_name, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_exemplar_trace ON otel_metrics(project_id, exemplar_trace_id);
CREATE INDEX IF NOT EXISTS idx_metrics_session ON otel_metrics(project_id, session_id);
DROP INDEX IF EXISTS idx_spans_project_trace;
DROP INDEX IF EXISTS idx_spans_project_ts;
DROP INDEX IF EXISTS idx_spans_project_ingest;
DROP INDEX IF EXISTS idx_spans_detail;
DROP INDEX IF EXISTS idx_spans_project_session;
DROP INDEX IF EXISTS idx_spans_project_span;
ALTER TABLE otel_spans ADD COLUMN scope_name VARCHAR;
ALTER TABLE otel_spans ADD COLUMN scope_version VARCHAR;
CREATE INDEX IF NOT EXISTS idx_spans_project_trace ON otel_spans(project_id, trace_id);
CREATE INDEX IF NOT EXISTS idx_spans_project_ts ON otel_spans(project_id, timestamp_start DESC);
CREATE INDEX IF NOT EXISTS idx_spans_project_ingest ON otel_spans(project_id, ingested_at DESC);
CREATE INDEX IF NOT EXISTS idx_spans_detail ON otel_spans(project_id, trace_id, span_id);
CREATE INDEX IF NOT EXISTS idx_spans_project_session ON otel_spans(project_id, session_id);
CREATE INDEX IF NOT EXISTS idx_spans_project_span ON otel_spans(project_id, span_id);
"#;

/// v2 to v3: the metrics version column.
///
/// A **new version**, not an addition to `MIGRATION_V2`, and that distinction is the whole point.
/// `MIGRATION_V2` takes a v1 database to v2; a database *already* at v2 - which is every database created by
/// any build after v1.0.13 - never runs it again. Putting the column there meant those databases never
/// received it while the metrics `Appender` had already started writing it: a binder error on the first
/// metric ingested, on every existing installation.
///
/// Declared **last**, matching the fresh schema, for the positional-`Appender` reason every appended column
/// has. Nullable, because DuckDB refuses `SET NOT NULL` on a TIMESTAMP added in the same transaction - see
/// the fresh schema's note. Existing rows take the epoch, below any real receipt time, so the first genuine
/// delivery outranks them. `IF NOT EXISTS` so a v1 database that reached v2 through the migration above,
/// back when it carried this column, still upgrades.
const MIGRATION_V3: &str = r#"ALTER TABLE otel_metrics ADD COLUMN IF NOT EXISTS ingested_at TIMESTAMP DEFAULT TIMESTAMP '1970-01-01 00:00:00';
"#;

const MIGRATION_V4: &str = r#"ALTER TABLE otel_spans ADD COLUMN IF NOT EXISTS content_digest VARCHAR DEFAULT '';
ALTER TABLE otel_metrics ADD COLUMN IF NOT EXISTS content_digest VARCHAR DEFAULT '';
CREATE TABLE IF NOT EXISTS otel_logs (
    project_id VARCHAR NOT NULL,
    log_digest VARCHAR NOT NULL,
    ordinal UINTEGER NOT NULL,
    timestamp TIMESTAMP NOT NULL,
    time TIMESTAMP,
    observed_time TIMESTAMP,
    severity_number INTEGER NOT NULL,
    severity_text VARCHAR,
    body JSON,
    body_text VARCHAR,
    attributes JSON,
    dropped_attributes_count UINTEGER NOT NULL,
    flags UINTEGER NOT NULL,
    trace_id VARCHAR,
    span_id VARCHAR,
    event_name VARCHAR,
    session_id VARCHAR,
    user_id VARCHAR,
    environment VARCHAR,
    service_name VARCHAR,
    service_version VARCHAR,
    service_namespace VARCHAR,
    service_instance_id VARCHAR,
    resource_attributes JSON,
    scope_name VARCHAR,
    scope_version VARCHAR,
    scope_attributes JSON,
    scope_schema_url VARCHAR,
    resource_schema_url VARCHAR,
    raw_log JSON,
    ingested_at TIMESTAMP NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_logs_identity ON otel_logs(project_id, log_digest, ordinal);
CREATE INDEX IF NOT EXISTS idx_logs_project_time ON otel_logs(project_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_logs_trace_span ON otel_logs(project_id, trace_id, span_id);
CREATE INDEX IF NOT EXISTS idx_logs_service ON otel_logs(project_id, service_name);
"#;

const MIGRATION_V5: &str = r#"
ALTER TABLE otel_spans ADD COLUMN IF NOT EXISTS hold_until TIMESTAMP;
ALTER TABLE otel_spans ADD COLUMN IF NOT EXISTS logical_bytes UBIGINT DEFAULT 0;
ALTER TABLE otel_metrics ADD COLUMN IF NOT EXISTS hold_until TIMESTAMP;
ALTER TABLE otel_metrics ADD COLUMN IF NOT EXISTS logical_bytes UBIGINT DEFAULT 0;
ALTER TABLE otel_logs ADD COLUMN IF NOT EXISTS hold_until TIMESTAMP;
ALTER TABLE otel_logs ADD COLUMN IF NOT EXISTS logical_bytes UBIGINT DEFAULT 0;
"#;

const MIGRATION_V6: &str = r#"
CREATE TABLE IF NOT EXISTS span_terms (
    project_id VARCHAR NOT NULL,
    trace_id VARCHAR NOT NULL,
    span_id VARCHAR NOT NULL,
    field VARCHAR NOT NULL,
    term VARCHAR NOT NULL,
    truncated BOOLEAN NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_span_terms_term ON span_terms(term);
CREATE TABLE IF NOT EXISTS log_terms (
    project_id VARCHAR NOT NULL,
    log_digest VARCHAR NOT NULL,
    ordinal UINTEGER NOT NULL,
    field VARCHAR NOT NULL,
    term VARCHAR NOT NULL,
    truncated BOOLEAN NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_log_terms_term ON log_terms(term);
"#;

struct Migration {
    version: i32,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 2,
        name: "v1_to_current",
        sql: MIGRATION_V2,
    },
    Migration {
        version: 3,
        name: "metric_version_column",
        sql: MIGRATION_V3,
    },
    Migration {
        version: 4,
        name: "otel_logs",
        sql: MIGRATION_V4,
    },
    Migration {
        version: 5,
        name: "logical_bytes_and_legal_hold",
        sql: MIGRATION_V5,
    },
    Migration {
        version: 6,
        name: "search_term_tables",
        sql: MIGRATION_V6,
    },
];

fn migration_run(current: Option<i32>) -> Result<MigrationRun, DuckdbError> {
    plan_migrations(
        current,
        SCHEMA_VERSION,
        1,
        MIGRATIONS
            .iter()
            .map(|migration| (migration.version, migration.name)),
    )
    .map_err(|error| DuckdbError::MigrationFailed {
        version: error.version(),
        name: "version_check".to_string(),
        error: error.to_string(),
    })
}

fn apply_migration(conn: &Connection, version: i32, clock: &dyn Clock) -> Result<(), DuckdbError> {
    match MIGRATIONS
        .iter()
        .find(|migration| migration.version == version)
    {
        Some(migration) => apply_versioned_migration(
            conn,
            migration.version,
            migration.name,
            migration.sql,
            clock,
        ),
        None => Err(DuckdbError::MigrationFailed {
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
    clock: &dyn Clock,
) -> Result<(), DuckdbError> {
    let start = std::time::Instant::now();

    in_transaction(conn, |conn| {
        conn.execute_batch(sql)
            .map_err(|e| DuckdbError::MigrationFailed {
                version,
                name: name.to_string(),
                error: e.to_string(),
            })?;

        let now = clock.now().timestamp_nanos_opt().unwrap_or(0);
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
    use crate::TestClock;
    use crate::schema::SCHEMA;

    fn run_migrations(conn: &Connection) -> Result<(), DuckdbError> {
        super::run_migrations(conn, &TestClock)
    }

    fn apply_migration(conn: &Connection, version: i32) -> Result<(), DuckdbError> {
        super::apply_migration(conn, version, &TestClock)
    }

    /// Column name and type, **ordered by position** - the property the positional `Appender` depends on.
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
        let fresh = Connection::open_in_memory().expect("fresh");
        fresh.execute_batch(SCHEMA).expect("fresh schema");

        // A v1 database: the current schema minus what migration 2 adds.
        let upgraded = Connection::open_in_memory().expect("upgraded");
        upgraded.execute_batch(SCHEMA).expect("base schema");
        // The indexes depend on the table, so they go first - DuckDB refuses to alter a table with
        // dependents. Recreated after, since the fresh schema declares them.
        upgraded
            .execute_batch(
                "DROP TABLE otel_logs;
                 DROP INDEX idx_metrics_project_ts;
                 DROP INDEX idx_metrics_project_name;
                 DROP INDEX idx_metrics_project_name_ts;
                 DROP INDEX idx_metrics_exemplar_trace;
                 DROP INDEX idx_metrics_session;
                 ALTER TABLE otel_metrics DROP COLUMN datapoint_id;
                 ALTER TABLE otel_metrics DROP COLUMN scope_attributes;
                 ALTER TABLE otel_metrics DROP COLUMN scope_schema_url;
                 ALTER TABLE otel_metrics DROP COLUMN resource_schema_url;
                 ALTER TABLE otel_metrics DROP COLUMN exemplars;
                 ALTER TABLE otel_metrics DROP COLUMN ingested_at;
                 ALTER TABLE otel_metrics DROP COLUMN content_digest;
                 ALTER TABLE otel_metrics DROP COLUMN hold_until;
                 ALTER TABLE otel_metrics DROP COLUMN logical_bytes;
                 -- Recreated, because a real v1 database has them and DuckDB refuses to alter a table
                 -- with dependents: the migration has to handle that itself.
                 CREATE INDEX idx_metrics_project_ts ON otel_metrics(project_id, timestamp DESC);
                 CREATE INDEX idx_metrics_project_name ON otel_metrics(project_id, metric_name);
                 CREATE INDEX idx_metrics_project_name_ts ON otel_metrics(project_id, metric_name, timestamp DESC);
                 CREATE INDEX idx_metrics_exemplar_trace ON otel_metrics(project_id, exemplar_trace_id);
                 CREATE INDEX idx_metrics_session ON otel_metrics(project_id, session_id);
                 -- The span side of the same reduction: v4 added the instrumentation scope.
                 DROP INDEX idx_spans_project_trace;
                 DROP INDEX idx_spans_project_ts;
                 DROP INDEX idx_spans_project_ingest;
                 DROP INDEX idx_spans_detail;
                 DROP INDEX idx_spans_project_session;
                 DROP INDEX idx_spans_project_span;
                 ALTER TABLE otel_spans DROP COLUMN scope_name;
                 ALTER TABLE otel_spans DROP COLUMN scope_version;
                 ALTER TABLE otel_spans DROP COLUMN content_digest;
                 ALTER TABLE otel_spans DROP COLUMN hold_until;
                 ALTER TABLE otel_spans DROP COLUMN logical_bytes;
                 CREATE INDEX idx_spans_project_trace ON otel_spans(project_id, trace_id);
                 CREATE INDEX idx_spans_project_ts ON otel_spans(project_id, timestamp_start DESC);
                 CREATE INDEX idx_spans_project_ingest ON otel_spans(project_id, ingested_at DESC);
                 CREATE INDEX idx_spans_detail ON otel_spans(project_id, trace_id, span_id);
                 CREATE INDEX idx_spans_project_session ON otel_spans(project_id, session_id);
                 CREATE INDEX idx_spans_project_span ON otel_spans(project_id, span_id);",
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

        // The whole chain, not one step: a future version bump then needs no edit here, and the property
        // being checked is about the destination rather than about any single migration.
        for version in 2..=SCHEMA_VERSION {
            apply_migration(&upgraded, version)
                .unwrap_or_else(|e| panic!("migration {version}: {e}"));
        }

        assert_eq!(
            columns(&upgraded, "otel_spans"),
            columns(&fresh, "otel_spans"),
            "an upgraded database's otel_spans columns differ in name, type or *position* from a fresh \
             one's - and the span writer is a positional Appender, so a position difference silently \
             writes every value into the wrong column"
        );
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

    /// A database **already at v2** upgrades to v3 and gets the metrics version column.
    ///
    /// The v1 test above cannot see this class of defect. It walks the whole chain, so a column added to an
    /// *already-applied* migration still arrives - while a database recorded at version N skips migration N
    /// forever. `ingested_at` went into `MIGRATION_V2` while the schema still said 2, so every database
    /// created after v1.0.13 never received it, and the metrics `Appender` had already started writing it: a
    /// binder error on the first metric ingested. Production is v1-only, so this was a developer-machine
    /// break rather than a deployed one - which is exactly the kind that reaches everyone who works here and
    /// nobody who runs it.
    ///
    /// Each future bump wants its own arm here, for the same reason: the reduction to version N-1 is
    /// migration-specific, which is why this cannot be a loop over every prior version.
    #[tokio::test]
    async fn a_v2_database_upgrades_to_v3_and_gains_the_metric_version_column() {
        let fresh = Connection::open_in_memory().expect("fresh");
        fresh.execute_batch(SCHEMA).expect("fresh schema");

        // A v2 database: the current schema minus what v3 adds. The indexes come off first because DuckDB
        // refuses to alter a table that has dependents - the same constraint a real migration meets.
        let upgraded = Connection::open_in_memory().expect("upgraded");
        upgraded.execute_batch(SCHEMA).expect("base schema");
        upgraded
            .execute_batch(
                "DROP TABLE otel_logs;
                 DROP INDEX idx_metrics_project_ts;
                 DROP INDEX idx_metrics_project_name;
                 DROP INDEX idx_metrics_project_name_ts;
                 DROP INDEX idx_metrics_exemplar_trace;
                 DROP INDEX idx_metrics_session;
                 ALTER TABLE otel_metrics DROP COLUMN ingested_at;
                 ALTER TABLE otel_metrics DROP COLUMN content_digest;
                 ALTER TABLE otel_metrics DROP COLUMN hold_until;
                 ALTER TABLE otel_metrics DROP COLUMN logical_bytes;
                 CREATE INDEX idx_metrics_project_ts ON otel_metrics(project_id, timestamp DESC);
                 CREATE INDEX idx_metrics_project_name ON otel_metrics(project_id, metric_name);
                 CREATE INDEX idx_metrics_project_name_ts ON otel_metrics(project_id, metric_name, timestamp DESC);
                 CREATE INDEX idx_metrics_exemplar_trace ON otel_metrics(project_id, exemplar_trace_id);
                 CREATE INDEX idx_metrics_session ON otel_metrics(project_id, session_id);
                 DROP INDEX idx_spans_project_trace;
                 DROP INDEX idx_spans_project_ts;
                 DROP INDEX idx_spans_project_ingest;
                 DROP INDEX idx_spans_detail;
                 DROP INDEX idx_spans_project_session;
                 DROP INDEX idx_spans_project_span;
                 ALTER TABLE otel_spans DROP COLUMN content_digest;
                 ALTER TABLE otel_spans DROP COLUMN hold_until;
                 ALTER TABLE otel_spans DROP COLUMN logical_bytes;
                 CREATE INDEX idx_spans_project_trace ON otel_spans(project_id, trace_id);
                 CREATE INDEX idx_spans_project_ts ON otel_spans(project_id, timestamp_start DESC);
                 CREATE INDEX idx_spans_project_ingest ON otel_spans(project_id, ingested_at DESC);
                 CREATE INDEX idx_spans_detail ON otel_spans(project_id, trace_id, span_id);
                 CREATE INDEX idx_spans_project_session ON otel_spans(project_id, session_id);
                 CREATE INDEX idx_spans_project_span ON otel_spans(project_id, span_id);",
            )
            .expect("reduce to the v2 shape");
        // A row written before the column existed, so the backfill is exercised rather than assumed.
        upgraded
            .execute_batch(
                "INSERT INTO otel_metrics (project_id, metric_name, metric_type, timestamp, datapoint_id) \
                 VALUES ('p1', 'v2.metric', 'gauge', TIMESTAMP '2026-01-01 00:00:00', 'dp-v2');",
            )
            .expect("v2 row");

        for version in 3..=SCHEMA_VERSION {
            apply_migration(&upgraded, version)
                .unwrap_or_else(|e| panic!("migration {version}: {e}"));
        }

        assert_eq!(
            columns(&upgraded, "otel_metrics"),
            columns(&fresh, "otel_metrics"),
            "a v2 database upgraded to v3 has different otel_metrics columns than a fresh one - and the \
             metrics writer is a positional Appender, so a difference in position writes every value into \
             the wrong column"
        );

        let backfilled: i64 = upgraded
            .query_row(
                "SELECT epoch_us(ingested_at) FROM otel_metrics WHERE datapoint_id = 'dp-v2'",
                [],
                |row| row.get(0),
            )
            .expect("the pre-existing row carries a version");
        assert_eq!(
            backfilled, 0,
            "a row written before the column existed takes the epoch, below any real receipt time, so the \
             first genuine delivery outranks it"
        );
    }

    #[test]
    fn a_populated_v4_database_upgrades_to_hold_and_logical_byte_columns() {
        let fresh = Connection::open_in_memory().expect("fresh");
        fresh.execute_batch(SCHEMA).expect("fresh schema");

        let upgraded = Connection::open_in_memory().expect("upgraded");
        let v4_schema = SCHEMA
            .replace(
                "    hold_until                 TIMESTAMP,\n    logical_bytes              UBIGINT NOT NULL DEFAULT 0,\n",
                "",
            )
            .replace(
                "    hold_until             TIMESTAMP,\n    logical_bytes          UBIGINT NOT NULL DEFAULT 0\n",
                "",
            )
            .replace(
                "    hold_until                TIMESTAMP,\n    logical_bytes             UBIGINT NOT NULL DEFAULT 0\n",
                "",
            );
        upgraded.execute_batch(&v4_schema).expect("v4 schema");
        for table in ["otel_spans", "otel_metrics", "otel_logs"] {
            let names = columns(&upgraded, table)
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>();
            assert!(!names.iter().any(|name| name == "hold_until"));
            assert!(!names.iter().any(|name| name == "logical_bytes"));
        }
        upgraded
            .execute_batch(
                "DELETE FROM schema_version;
                 INSERT INTO schema_version VALUES (1, 4, 0, 'v4');
                 INSERT INTO otel_spans
                     (project_id, trace_id, span_id, span_name, timestamp_start)
                     VALUES ('p', 't', 's', 'legacy', TIMESTAMP '2026-01-01 00:00:00');
                 INSERT INTO otel_metrics
                     (project_id, metric_name, metric_type, timestamp, datapoint_id)
                     VALUES ('p', 'legacy.metric', 'gauge', TIMESTAMP '2026-01-01 00:00:00', 'dp');
                 INSERT INTO otel_logs
                     (project_id, log_digest, ordinal, timestamp, severity_number,
                      dropped_attributes_count, flags, ingested_at)
                     VALUES ('p', 'log', 0, TIMESTAMP '2026-01-01 00:00:00',
                             0, 0, 0, TIMESTAMP '2026-01-01 00:00:00');",
            )
            .expect("v4 populated shape");

        apply_migration(&upgraded, 5).expect("v4 upgrades to v5");
        for table in ["otel_spans", "otel_metrics", "otel_logs"] {
            assert_eq!(
                columns(&upgraded, table),
                columns(&fresh, table),
                "{table} differs after populated v4 upgrade"
            );
            let row: (Option<String>, u64) = upgraded
                .query_row(
                    &format!("SELECT CAST(hold_until AS VARCHAR), logical_bytes FROM {table}"),
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("legacy row survives");
            assert_eq!(row, (None, 0));
        }
    }
}
