//! Versioned SQLite schema migration registry.
//!
//! Fresh databases install [`SCHEMA`] directly. Existing databases apply every immutable migration after their
//! recorded version, in order, and record the checksum and execution time of each step.

use sqlx::SqlitePool;

use super::error::SqliteError;
use super::schema::{SCHEMA, SCHEMA_VERSION};
use sideseat_core::migration::{MigrationRun, plan_migrations};
use sideseat_core::utils::crypto::sha256_hex;
use sideseat_ports::clock::Clock;

/// Initialize a fresh database or apply all pending migrations.
pub async fn run_migrations(pool: &SqlitePool, clock: &dyn Clock) -> Result<(), SqliteError> {
    let table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_version'",
    )
    .fetch_one(pool)
    .await?;

    if !table_exists {
        tracing::debug!(
            "Initializing database with schema version {}",
            SCHEMA_VERSION
        );
        apply_initial_schema(pool, clock).await?;
        return Ok(());
    }

    let current_version: Option<i32> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
            .fetch_optional(pool)
            .await?;

    match migration_run(current_version)? {
        MigrationRun::Initialize { .. } => apply_initial_schema(pool, clock).await?,
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
                apply_migration(pool, step.version, clock).await?;
            }
        }
    }

    Ok(())
}

/// Install the current schema on a fresh database.
async fn apply_initial_schema(pool: &SqlitePool, clock: &dyn Clock) -> Result<(), SqliteError> {
    let start = std::time::Instant::now();

    let mut tx = pool.begin().await?;

    // `raw_sql` executes the schema as one script and preserves SQL parsing across comments and literals.
    sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;

    let now = clock.now().timestamp_nanos_opt().unwrap_or(0);
    sqlx::query(
        "INSERT INTO schema_version (id, version, applied_at, description) VALUES (1, ?, ?, 'Initial schema')",
    )
    .bind(SCHEMA_VERSION)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    let checksum = sha256_hex(SCHEMA);
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

    tx.commit().await?;

    tracing::debug!("Applied initial schema in {}ms", elapsed_ms);
    Ok(())
}

/// The consolidated v1 → v2 baseline migration.
///
/// Version 1 predates every addition in this script, so the transition creates the released v2 shape directly.
/// Later released schema changes remain separate immutable migrations below.
const MIGRATION_V2: &str = r#"
-- files: the content hash algorithm, and the deletion claim that lets cleanup and ingestion agree about
-- a file that is mid-deletion (`claim_file_for_deletion`, `associate_file`).
-- Order matters: these land at the end of the table, so the fresh schema declares them at the end too,
-- and in this same sequence. A positional writer would otherwise read a fresh database differently from
-- an upgraded one.
ALTER TABLE files ADD COLUMN hash_algo TEXT NOT NULL DEFAULT 'sha256';
ALTER TABLE files ADD COLUMN deleting_at INTEGER;

-- projects / organizations: the deletion tombstone, plus the repeated-observation counters that decide
-- when the tombstone may go (`record_project_sweep`). Removal is driven by what has been observed, never
-- by elapsed time - a stalled writer can commit arbitrarily later than any grace period.
ALTER TABLE projects ADD COLUMN deleting_at INTEGER;
ALTER TABLE projects ADD COLUMN clean_sweeps INTEGER NOT NULL DEFAULT 0;
ALTER TABLE projects ADD COLUMN last_sweep_at INTEGER;
ALTER TABLE organizations ADD COLUMN deleting_at INTEGER;

-- Provider credentials, org-scoped with per-project permissions.
CREATE TABLE IF NOT EXISTS credentials (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    provider_key TEXT NOT NULL,
    display_name TEXT NOT NULL CHECK(length(display_name) >= 1 AND length(display_name) <= 100),
    endpoint_url TEXT,
    extra_config TEXT,
    key_preview TEXT,
    created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_creds_org ON credentials(organization_id);
CREATE INDEX IF NOT EXISTS idx_creds_org_key ON credentials(organization_id, provider_key);
CREATE TABLE IF NOT EXISTS credential_project_permissions (
    id TEXT PRIMARY KEY,
    credential_id TEXT NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
    access TEXT NOT NULL DEFAULT 'allow' CHECK(access IN ('allow', 'deny')),
    created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cred_perms_credential ON credential_project_permissions(credential_id);
CREATE INDEX IF NOT EXISTS idx_cred_perms_project ON credential_project_permissions(project_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_cred_perms_unique_project
    ON credential_project_permissions(credential_id, project_id)
    WHERE project_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_cred_perms_unique_org_default
    ON credential_project_permissions(credential_id)
    WHERE project_id IS NULL;

-- trace_files, keyed with the project first. A trace id comes from the client, so two projects can
-- present the same one; keyed without the project, one project's association satisfied the other's
-- `INSERT OR IGNORE` and left the second with a reference nothing would release. SQLite cannot alter a
-- primary key in place, so the table is rebuilt - and the copy is keyed on the widened key, so rows that
-- had already collided collapse rather than duplicate.
CREATE TABLE trace_files_v2 (
    trace_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    file_hash TEXT NOT NULL,
    -- See the schema comment: two facts, not a boolean. `pending_writers` counts in-flight referencing
    -- batches; `durable` is set once any commits. Legacy rows are copied below with `durable = 1` - they
    -- are committed associations, and a fresh 0 would leave them one release away from deletion.
    pending_writers INTEGER NOT NULL DEFAULT 0,
    durable INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, trace_id, file_hash),
    FOREIGN KEY (project_id, file_hash) REFERENCES files(project_id, file_hash) ON DELETE CASCADE
);
-- durable = 1 for the copied rows: they are committed associations, and a fresh 0 would leave them one
-- release away from deletion. New rows inserted by the app default to 0 (provisional).
INSERT OR IGNORE INTO trace_files_v2 (trace_id, project_id, file_hash, durable)
    SELECT trace_id, project_id, file_hash, 1 FROM trace_files;
DROP TABLE trace_files;
ALTER TABLE trace_files_v2 RENAME TO trace_files;
CREATE INDEX IF NOT EXISTS idx_trace_files_trace ON trace_files(trace_id);
CREATE INDEX IF NOT EXISTS idx_trace_files_project ON trace_files(project_id);
CREATE INDEX IF NOT EXISTS idx_trace_files_project_hash ON trace_files(project_id, file_hash);

-- Deletion records kept permanently, because finite evidence still loses to an arbitrarily delayed
-- writer: it can commit after the project row is gone, and then nothing knows the project existed.
-- Discovery is leased (`claim_token`), backed off (`quiet_checks` materialised into `next_check_at`) and
-- indexed on the due time itself - an index on an input to the eligibility expression bounds rows
-- returned, not rows examined. `next_check_at` is NOT NULL because the eligibility test is
-- `next_check_at <= unixepoch()`, which no null satisfies: one null row would go unclaimed forever.
CREATE TABLE IF NOT EXISTS deleted_projects (
    project_id TEXT PRIMARY KEY,
    deleted_at INTEGER NOT NULL,
    last_checked_at INTEGER,
    quiet_checks INTEGER NOT NULL DEFAULT 0,
    next_check_at INTEGER NOT NULL DEFAULT 0,
    claim_token INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_deleted_projects_due ON deleted_projects(next_check_at);

-- The trace deletion tombstone. The file fence alone cannot stop an ingest whose analytics row commits
-- after `delete_traces` returned from resurrecting a trace with a dangling `#!B64!#` reference; ingest
-- consults this immediately before the analytics write.
CREATE TABLE IF NOT EXISTS deleted_traces (
    project_id  TEXT    NOT NULL,
    trace_id    TEXT    NOT NULL,
    deleted_at  INTEGER NOT NULL,
    -- The same leased, backed-off schedule the deleted-project records use, and for the same reason: the
    -- pre-write check and the analytics write are in different stores, so a crash between them leaves
    -- spans for a deleted trace and only a sweep can collect them. Re-checking every record forever at a
    -- fixed rate would be unbounded lifetime work, so a quiet check pushes the next one further out and
    -- the due time itself is indexed.
    quiet_checks  INTEGER NOT NULL DEFAULT 0,
    next_check_at INTEGER NOT NULL DEFAULT 0,
    claim_token   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, trace_id)
);
CREATE INDEX IF NOT EXISTS idx_deleted_traces_due ON deleted_traces(next_check_at);
-- Sessions whose deletion has to outlive the traces it knew about.
--
-- A session is deleted *by* deleting its traces, so the route resolves session ids to trace ids and
-- tombstones those. That closes nothing for a trace of the same session that arrives *after* the
-- resolution: it was never in the snapshot, so it is never tombstoned, and it recreates the session the
-- caller was told was gone. The session id is the durable fact - the trace ids are a snapshot of one
-- instant - so it is what the write path checks.
CREATE TABLE IF NOT EXISTS deleted_sessions (
    project_id  TEXT    NOT NULL,
    session_id  TEXT    NOT NULL,
    deleted_at  INTEGER NOT NULL,
    -- The same leased, backed-off schedule the other deletion records use, and for the same reason.
    quiet_checks  INTEGER NOT NULL DEFAULT 0,
    next_check_at INTEGER NOT NULL DEFAULT 0,
    claim_token   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, session_id)
);
CREATE INDEX IF NOT EXISTS idx_deleted_sessions_due ON deleted_sessions(next_check_at);

"#;

/// The v2 → v3 step: retention's cleanup intent.
///
/// Released migration scripts are immutable: databases already at v2 only execute this step. The table is
/// idempotent, and its runtime contract is documented in the fresh schema.
const MIGRATION_V3: &str = r#"
CREATE TABLE IF NOT EXISTS retention_cleanup (
    project_id      TEXT    NOT NULL,
    trace_id        TEXT    NOT NULL,
    created_at      INTEGER NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    claim_token     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, trace_id)
);
CREATE INDEX IF NOT EXISTS idx_retention_cleanup_due ON retention_cleanup(next_attempt_at);
"#;

/// The deletion journal (schema v4).
///
/// Databases already at v3 execute this immutable step. The table is idempotent; its permanent retention
/// contract is documented in the fresh schema.
const MIGRATION_V4: &str = r#"
-- =============================================================================
-- Deletion journal: the deletions a restore cannot recompute
-- =============================================================================
--
-- Append-only, permanent, and exempt from every sweep. Two consumers need it and neither can be served by the
-- tombstone tables: a restore replays it forward before serving reads, because a snapshot predating a deletion
-- predates its tombstone too - restoring the analytics store further back than the transactional one, which is
-- what different backup cadences produce, resurrects rows the caller was told were gone. And the staged-payload
-- re-drive sweep asks it whether an absence was *intended*, because without that it cannot tell a failed write
-- from a deliberate deletion or a pressure eviction, and recreates exactly what those removed.
--
-- **Age retention writes nothing here.** It is a predicate, so a restored database recomputes the same verdict
-- from the timestamps it holds, and an entry per aged-out record would be an unbounded write for a fact
-- that is already derivable.
-- What is not derivable is a caller's request, and a limit having been reached.
--
-- `sequence` is the append order and is what a replay resumes from. Not the timestamp: two entries can share a
-- microsecond, and a clock is not an order - which is the constraint this whole subsystem is shaped by, since
-- neither analytics store offers a commit-ordered sequence at all.
--
-- No index on `project_id` alone: `deletion_is_journaled` is keyed on the target, and the replay walks
-- `sequence`, which the primary key already orders.
CREATE TABLE IF NOT EXISTS deletion_journal (
    sequence    INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  TEXT    NOT NULL,
    cause       TEXT    NOT NULL CHECK(cause IN ('requested', 'pressure')),
    scope       TEXT    NOT NULL CHECK(scope IN ('trace', 'session', 'project', 'organization', 'span')),
    target_id   TEXT    NOT NULL,
    -- Set only for a span-scoped entry, where `target_id` is the span's trace.
    span_id     TEXT,
    recorded_at INTEGER NOT NULL,
    -- A span-scoped entry carries a span id and nothing else does.
    --
    -- Without this the pair is expressible and inert: `deletion_is_journaled` matches on `span_id = ?`, so a
    -- span row with a null id can be appended successfully and then never found, and the re-drive sweep
    -- recreates the very span the entry was written to explain. The reverse - a span id on a trace-scoped row -
    -- is a claim about a span the entry does not describe.
    CHECK ((scope = 'span') = (span_id IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS idx_deletion_journal_target
    ON deletion_journal(project_id, scope, target_id);
"#;

/// The span-id constraint on `deletion_journal` (schema v5).
///
/// SQLite cannot add a `CHECK` to an existing table, so the table is rebuilt and copied. The copy preserves
/// usable deletion evidence while enforcing the final row shape:
///
/// - `scope = 'span'` with a null `span_id` is genuinely **inert**: `deletion_is_journaled` matches on
///   `span_id`, so such a row has never been findable and has never explained anything. Dropped.
/// - Any other scope carrying a stray `span_id` is **understood correctly today**: `deletions_since` replays its
///   scope and target, and the lookup ignores `span_id` for non-span scopes. So the column is normalised to NULL
///   and the row is kept. Deleting it would lose a real deletion record, and a restore predating that deletion
///   would then bring its trace, session, project or organization back.
const MIGRATION_V5: &str = r#"
CREATE TABLE deletion_journal_v5 (
    sequence    INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  TEXT    NOT NULL,
    cause       TEXT    NOT NULL CHECK(cause IN ('requested', 'pressure')),
    scope       TEXT    NOT NULL CHECK(scope IN ('trace', 'session', 'project', 'organization', 'span')),
    target_id   TEXT    NOT NULL,
    span_id     TEXT,
    recorded_at INTEGER NOT NULL,
    CHECK ((scope = 'span') = (span_id IS NOT NULL))
);
INSERT INTO deletion_journal_v5 (sequence, project_id, cause, scope, target_id, span_id, recorded_at)
    SELECT sequence, project_id, cause, scope, target_id,
           -- Normalised, not filtered: a non-span row's `span_id` is noise the readers already ignore, while the
           -- row itself is a deletion record a restore has to replay.
           CASE WHEN scope = 'span' THEN span_id ELSE NULL END,
           recorded_at
    FROM deletion_journal
    -- The one genuinely inert shape: a span-scoped row with no span id was never findable by
    -- `deletion_is_journaled`, so it has never explained an absence and dropping it loses nothing.
    WHERE NOT (scope = 'span' AND span_id IS NULL);
DROP TABLE deletion_journal;
ALTER TABLE deletion_journal_v5 RENAME TO deletion_journal;
CREATE INDEX IF NOT EXISTS idx_deletion_journal_target
    ON deletion_journal(project_id, scope, target_id);
"#;

const MIGRATION_V6: &str = r#"
CREATE TABLE IF NOT EXISTS staged_payloads (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    signal TEXT NOT NULL CHECK(signal IN ('traces', 'metrics', 'logs')),
    blob_hash TEXT NOT NULL,
    byte_len INTEGER NOT NULL CHECK(byte_len >= 0),
    created_at INTEGER NOT NULL,
    redrive_attempts INTEGER NOT NULL DEFAULT 0 CHECK(redrive_attempts >= 0),
    unconfirmed INTEGER NOT NULL DEFAULT 0 CHECK(unconfirmed IN (0, 1)),
    records_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_staged_payloads_pending
    ON staged_payloads(unconfirmed, created_at, id);
"#;

const MIGRATION_V7: &str = r#"
ALTER TABLE deletion_journal ADD COLUMN logical_bytes INTEGER NOT NULL DEFAULT 0 CHECK(logical_bytes >= 0);
UPDATE deletion_journal
SET logical_bytes =
    64
    + length(CAST(project_id AS BLOB))
    + length(CAST(cause AS BLOB))
    + length(CAST(scope AS BLOB))
    + length(CAST(target_id AS BLOB))
    + length(CAST(COALESCE(span_id, '') AS BLOB));
CREATE TABLE IF NOT EXISTS project_holds (
    project_id TEXT PRIMARY KEY,
    hold_until INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_project_holds_active ON project_holds(hold_until, project_id);
CREATE TABLE IF NOT EXISTS project_maintenance_leases (
    project_id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    lease_until INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_project_maintenance_leases_expiry
    ON project_maintenance_leases(lease_until);
CREATE TABLE IF NOT EXISTS project_storage_usage (
    project_id TEXT PRIMARY KEY,
    logical_bytes INTEGER NOT NULL DEFAULT 0 CHECK(logical_bytes >= 0),
    updated_at INTEGER NOT NULL
);
"#;

const MIGRATION_V8: &str = r#"
ALTER TABLE retention_cleanup ADD COLUMN logical_bytes INTEGER NOT NULL DEFAULT 0 CHECK(logical_bytes >= 0);
UPDATE retention_cleanup
SET logical_bytes =
    64
    + length(CAST(project_id AS BLOB))
    + length(CAST(trace_id AS BLOB));
"#;

const MIGRATION_V9: &str = r#"
CREATE TABLE IF NOT EXISTS content_bodies (
    project_id TEXT NOT NULL,
    body_hash TEXT NOT NULL,
    logical_bytes INTEGER NOT NULL CHECK(logical_bytes >= 0),
    created_at INTEGER NOT NULL,
    last_referenced_at INTEGER NOT NULL,
    deleting_at INTEGER,
    PRIMARY KEY (project_id, body_hash)
);
CREATE TABLE IF NOT EXISTS span_bodies (
    project_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    span_id TEXT NOT NULL,
    field TEXT NOT NULL CHECK(field IN ('messages', 'tool_definitions', 'tool_names', 'raw_span')),
    body_hash TEXT NOT NULL,
    pending_writers INTEGER NOT NULL DEFAULT 0 CHECK(pending_writers >= 0),
    durable INTEGER NOT NULL DEFAULT 0 CHECK(durable IN (0, 1)),
    PRIMARY KEY (project_id, trace_id, span_id, field, body_hash),
    FOREIGN KEY (project_id, body_hash)
        REFERENCES content_bodies(project_id, body_hash) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_span_bodies_object ON span_bodies(project_id, body_hash);
CREATE INDEX IF NOT EXISTS idx_span_bodies_identity
    ON span_bodies(project_id, trace_id, span_id);
CREATE TABLE IF NOT EXISTS content_body_backfill (
    project_id TEXT PRIMARY KEY,
    cursor_trace_id TEXT,
    cursor_span_id TEXT,
    complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN (0, 1)),
    updated_at INTEGER NOT NULL
);
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
        name: "retention_cleanup_intent",
        sql: MIGRATION_V3,
    },
    Migration {
        version: 4,
        name: "deletion_journal",
        sql: MIGRATION_V4,
    },
    Migration {
        version: 5,
        name: "deletion_journal_span_id_check",
        sql: MIGRATION_V5,
    },
    Migration {
        version: 6,
        name: "staged_payloads",
        sql: MIGRATION_V6,
    },
    Migration {
        version: 7,
        name: "storage_governance",
        sql: MIGRATION_V7,
    },
    Migration {
        version: 8,
        name: "retention_cleanup_logical_bytes",
        sql: MIGRATION_V8,
    },
    Migration {
        version: 9,
        name: "content_bodies",
        sql: MIGRATION_V9,
    },
];

fn migration_run(current: Option<i32>) -> Result<MigrationRun, SqliteError> {
    plan_migrations(
        current,
        SCHEMA_VERSION,
        1,
        MIGRATIONS
            .iter()
            .map(|migration| (migration.version, migration.name)),
    )
    .map_err(|error| SqliteError::MigrationFailed {
        version: error.version(),
        name: "version_check".to_string(),
        error: error.to_string(),
    })
}

async fn apply_migration(
    pool: &SqlitePool,
    version: i32,
    clock: &dyn Clock,
) -> Result<(), SqliteError> {
    match MIGRATIONS
        .iter()
        .find(|migration| migration.version == version)
    {
        Some(migration) => {
            apply_versioned_migration(
                pool,
                migration.version,
                migration.name,
                migration.sql,
                clock,
            )
            .await
        }
        None => Err(SqliteError::MigrationFailed {
            version,
            name: "unknown".to_string(),
            error: format!("Unknown migration version: {}", version),
        }),
    }
}

/// Apply one migration atomically and record its checksum and execution time.
async fn apply_versioned_migration(
    pool: &SqlitePool,
    version: i32,
    name: &str,
    sql: &str,
    clock: &dyn Clock,
) -> Result<(), SqliteError> {
    let start = std::time::Instant::now();

    let mut tx = pool.begin().await?;

    // Execute the migration as a script so comments and literals retain normal SQL parsing.
    sqlx::raw_sql(sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| SqliteError::MigrationFailed {
            version,
            name: name.to_string(),
            error: e.to_string(),
        })?;

    let now = clock.now().timestamp_nanos_opt().unwrap_or(0);
    sqlx::query(
        "UPDATE schema_version SET version = ?, applied_at = ?, description = ? WHERE id = 1",
    )
    .bind(version)
    .bind(now)
    .bind(name)
    .execute(&mut *tx)
    .await?;

    let checksum = sha256_hex(sql);
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

    tracing::debug!(
        "Applied migration v{} ({}) in {}ms",
        version,
        name,
        elapsed_ms
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every column the v1 migration adds is declared *last* in the fresh schema, in the same order.
    ///
    /// `ALTER TABLE ADD COLUMN` can only append, so a column declared mid-table gives a fresh database and
    /// an upgraded one different physical column orders for one schema version. Harmless while every writer
    /// names its columns, and silently catastrophic the moment one is positional - DuckDB's metrics appender
    /// is, and the same mistake there wrote every value one column across on exactly the databases the
    /// migration exists to serve.
    ///
    /// Asserted against the schema *text* rather than a live database, because reducing one to its v1 shape
    /// requires `DROP COLUMN`, which makes SQLite re-parse its stored DDL and fail on the comments.
    #[test]
    fn migration_added_columns_are_declared_last() {
        /// The column names a `CREATE TABLE` declares, in order, ignoring comments and constraints.
        fn declared_columns(schema: &str, table: &str) -> Vec<String> {
            let start = schema
                .find(&format!("CREATE TABLE IF NOT EXISTS {table} ("))
                .unwrap_or_else(|| panic!("{table} not found in the schema"));
            let body_start = schema[start..].find('(').expect("open paren") + start + 1;
            let body_end = schema[body_start..].find("\n);").expect("close paren") + body_start;
            schema[body_start..body_end]
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with("--"))
                .filter_map(|line| {
                    let name = line.split_whitespace().next()?;
                    // Table constraints are not columns, and they are written both with and without a
                    // space before the parenthesis - `UNIQUE(a, b)` and `PRIMARY KEY (a)`.
                    let keyword = name.split('(').next().unwrap_or(name).to_ascii_uppercase();
                    if ["PRIMARY", "UNIQUE", "FOREIGN", "CHECK", "CONSTRAINT"]
                        .contains(&keyword.as_str())
                    {
                        return None;
                    }
                    Some(name.to_string())
                })
                .collect()
        }

        // Each table, with the columns MIGRATION_V2 appends to it in the order it appends them.
        for (table, added) in [
            ("files", vec!["hash_algo", "deleting_at"]),
            (
                "projects",
                vec!["deleting_at", "clean_sweeps", "last_sweep_at"],
            ),
            ("organizations", vec!["deleting_at"]),
        ] {
            let declared = declared_columns(SCHEMA, table);
            let tail: Vec<&str> = declared
                .iter()
                .rev()
                .take(added.len())
                .rev()
                .map(String::as_str)
                .collect();
            assert_eq!(
                tail, added,
                "the fresh `{table}` must declare the migration's added columns last and in the order it \
                 adds them, or a fresh and an upgraded database differ in physical column order"
            );
            // And the migration really does add exactly these, in this order.
            let mut position = 0usize;
            for column in &added {
                let needle = format!("ALTER TABLE {table} ADD COLUMN {column} ");
                let found = MIGRATION_V2[position..].find(&needle).unwrap_or_else(|| {
                    panic!("MIGRATION_V2 does not add {table}.{column} in order")
                });
                position += found + needle.len();
            }
        }
    }

    /// A v1 database walked forward has the same schema as a fresh one - every table, not one.
    ///
    /// This is the invariant a migration exists to preserve, and it is easy to half-keep: SQLite cannot add
    /// a NOT NULL constraint with `ALTER TABLE`, so a migration that backfills values and stops leaves the
    /// column nullable while the fresh schema declares it NOT NULL. Two schemas for one version, and the
    /// difference shows only when something inserts the null the fresh schema would have refused.
    ///
    /// Compared over *every* table SQLite reports, by asking SQLite itself about the columns, so a future
    /// migration that alters any table is covered rather than only the one that prompted the test. The v1
    /// database is built as "the current schema minus what migration 2 adds", which is what a v1 database
    /// is by definition - and if that drifts, this test is where it shows.
    #[tokio::test]
    async fn a_v1_database_upgrades_to_exactly_the_fresh_schema() {
        async fn tables(pool: &SqlitePool) -> Vec<String> {
            let mut names: Vec<String> = sqlx::query_scalar(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .fetch_all(pool)
            .await
            .expect("table list");
            names.sort();
            names
        }
        async fn columns(
            pool: &SqlitePool,
            table: &str,
        ) -> Vec<(String, String, i64, Option<String>)> {
            // Sorted, so this compares the column *set* - names, types, nullability and defaults.
            //
            // Physical order is checked separately (`migration_added_columns_are_declared_last`) rather
            // than here, because reducing a database to its v1 shape with `ALTER TABLE DROP COLUMN` makes
            // SQLite re-parse its stored DDL, and it rejects the result once the definition carries
            // comments. Order still matters - a positional writer shifts every value by a column when
            // fresh and upgraded disagree, which is exactly what happened to DuckDB's metrics appender -
            // so it is asserted against the schema text, where no DDL rewriting is involved.
            let mut rows: Vec<(String, String, i64, Option<String>)> = sqlx::query_as(&format!(
                "SELECT name, type, \"notnull\", dflt_value FROM pragma_table_info('{table}')"
            ))
            .fetch_all(pool)
            .await
            .expect("pragma");
            rows.sort();
            rows
        }

        // A fresh database at the current version.
        let fresh = SqlitePool::connect(":memory:").await.expect("fresh pool");
        sqlx::raw_sql(SCHEMA)
            .execute(&fresh)
            .await
            .expect("fresh schema");

        // A v1 database: the current schema, minus everything migration 2 introduces.
        let upgraded = SqlitePool::connect(":memory:")
            .await
            .expect("upgraded pool");
        sqlx::raw_sql(SCHEMA)
            .execute(&upgraded)
            .await
            .expect("base schema");
        // The v1 shape, written out. Rebuilt rather than reduced with `ALTER TABLE DROP COLUMN`: SQLite
        // re-parses its stored `CREATE TABLE` text on a drop and rejects the result once that text carries
        // comments, which the real schema does throughout. Spelling the v1 tables here is also the honest
        // form - this *is* what v1 was, and if it drifts, the comparison below is where it shows.
        sqlx::raw_sql(
            // Tables a v1 database does not have. Each one left in place lets the migration that creates it be
            // deleted with this test still green: the fixture would already carry it, the comparison would find
            // no difference, and a real upgrade would never create it. `deleted_sessions` had exactly that hole
            // - it arrives in migration 2 and the reduction never dropped it.
            "DROP TABLE project_storage_usage;
             DROP TABLE project_maintenance_leases;
             DROP TABLE project_holds;
             DROP TABLE staged_payloads;
             DROP TABLE deletion_journal;
             DROP TABLE retention_cleanup;
             DROP TABLE deleted_sessions;
             DROP TABLE credentials;
             DROP TABLE credential_project_permissions;
             DROP TABLE deleted_projects;
             DROP TABLE deleted_traces;
             DROP TABLE trace_files;
             DROP TABLE files;
             DROP TABLE projects;
             DROP TABLE organizations;
             CREATE TABLE organizations (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 slug TEXT NOT NULL UNIQUE,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE projects (
                 id TEXT PRIMARY KEY,
                 organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE files (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 project_id TEXT NOT NULL,
                 file_hash TEXT NOT NULL,
                 media_type TEXT,
                 size_bytes INTEGER NOT NULL,
                 ref_count INTEGER NOT NULL DEFAULT 1,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 UNIQUE(project_id, file_hash)
             );
             CREATE TABLE trace_files (
                 trace_id TEXT NOT NULL,
                 project_id TEXT NOT NULL,
                 file_hash TEXT NOT NULL,
                 PRIMARY KEY (trace_id, file_hash)
             );
             INSERT INTO schema_version (id, version, applied_at) VALUES (1, 1, 0);",
        )
        .execute(&upgraded)
        .await
        .expect("reduce to the v1 shape");

        // A pre-existing file and association, to prove the trace_files rebuild carries data across.
        // The file row has to exist: the rebuilt table carries a foreign key to it, and a rebuild that
        // silently dropped associations would leave a reference nothing releases.
        sqlx::raw_sql(
            "INSERT INTO files (project_id, file_hash, size_bytes, created_at, updated_at)
                 VALUES ('p1', 'h1', 1, 0, 0);
             INSERT INTO trace_files (trace_id, project_id, file_hash) VALUES ('t1', 'p1', 'h1');",
        )
        .execute(&upgraded)
        .await
        .expect("legacy association");

        for version in 2..=SCHEMA_VERSION {
            apply_migration(&upgraded, version, &crate::TestClock)
                .await
                .unwrap_or_else(|e| panic!("migration {version}: {e}"));
        }

        assert_eq!(
            tables(&upgraded).await,
            tables(&fresh).await,
            "an upgraded database has a different set of tables from a fresh one"
        );
        for table in tables(&fresh).await {
            assert_eq!(
                columns(&upgraded, &table).await,
                columns(&fresh, &table).await,
                "an upgraded database's `{table}` differs from a fresh one's, so an invariant the fresh \
                 schema declares is not enforced after an upgrade"
            );
        }

        // And the rebuild carried the row rather than dropping it.
        let carried: (String, String, String) =
            sqlx::query_as("SELECT trace_id, project_id, file_hash FROM trace_files")
                .fetch_one(&upgraded)
                .await
                .expect("the legacy association survives the table rebuild");
        assert_eq!(
            carried,
            ("t1".to_string(), "p1".to_string(), "h1".to_string()),
            "the trace_files rebuild must carry existing associations; losing one leaves a file \
             reference nothing will ever release"
        );
    }

    #[tokio::test]
    async fn a_populated_v6_database_backfills_journal_bytes_and_adds_governance_tables() {
        let pool = SqlitePool::connect(":memory:").await.expect("pool");
        sqlx::raw_sql(
            "CREATE TABLE schema_version (
                 id INTEGER PRIMARY KEY,
                 version INTEGER NOT NULL,
                 applied_at INTEGER NOT NULL,
                 description TEXT
             );
             CREATE TABLE schema_migrations (
                 version INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 applied_at INTEGER NOT NULL,
                 checksum TEXT NOT NULL,
                 execution_time_ms INTEGER,
                 success INTEGER NOT NULL DEFAULT 1
             );
             CREATE TABLE deletion_journal (
                 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 project_id TEXT NOT NULL,
                 cause TEXT NOT NULL,
                 scope TEXT NOT NULL,
                 target_id TEXT NOT NULL,
                 span_id TEXT,
                 recorded_at INTEGER NOT NULL
             );
             INSERT INTO schema_version VALUES (1, 6, 0, 'v6');
             INSERT INTO deletion_journal
                 (project_id, cause, scope, target_id, span_id, recorded_at)
                 VALUES ('project', 'requested', 'span', 'trace', 'span', 1);",
        )
        .execute(&pool)
        .await
        .expect("populated v6 fixture");

        apply_migration(&pool, 7, &crate::TestClock)
            .await
            .expect("v6 upgrades to v7");

        let logical_bytes: i64 = sqlx::query_scalar("SELECT logical_bytes FROM deletion_journal")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            logical_bytes,
            64 + "project".len() as i64
                + "requested".len() as i64
                + "span".len() as i64
                + "trace".len() as i64
                + "span".len() as i64
        );
        for table in [
            "project_holds",
            "project_maintenance_leases",
            "project_storage_usage",
        ] {
            let exists: bool = sqlx::query_scalar(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = ?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(exists, "{table} was not created");
        }
    }

    #[tokio::test]
    async fn a_populated_v7_database_backfills_retention_cleanup_bytes() {
        let pool = SqlitePool::connect(":memory:").await.expect("pool");
        sqlx::raw_sql(
            "CREATE TABLE schema_version (
                 id INTEGER PRIMARY KEY,
                 version INTEGER NOT NULL,
                 applied_at INTEGER NOT NULL,
                 description TEXT
             );
             CREATE TABLE schema_migrations (
                 version INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 applied_at INTEGER NOT NULL,
                 checksum TEXT NOT NULL,
                 execution_time_ms INTEGER,
                 success INTEGER NOT NULL DEFAULT 1
             );
             CREATE TABLE retention_cleanup (
                 project_id TEXT NOT NULL,
                 trace_id TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 attempts INTEGER NOT NULL DEFAULT 0,
                 next_attempt_at INTEGER NOT NULL DEFAULT 0,
                 claim_token INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (project_id, trace_id)
             );
             INSERT INTO schema_version VALUES (1, 7, 0, 'v7');
             INSERT INTO retention_cleanup
                 (project_id, trace_id, created_at)
                 VALUES ('project', 'trace', 1);",
        )
        .execute(&pool)
        .await
        .expect("populated v7 fixture");

        apply_migration(&pool, 8, &crate::TestClock)
            .await
            .expect("v7 upgrades to v8");

        let logical_bytes: i64 = sqlx::query_scalar("SELECT logical_bytes FROM retention_cleanup")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            logical_bytes,
            64 + "project".len() as i64 + "trace".len() as i64
        );
    }
}
