//! Database migration system
//!
//! Handles schema versioning and incremental migrations.
//! Version 1 is the initial schema - future migrations will be added here.

use sqlx::SqlitePool;

use super::error::SqliteError;
use super::schema::{SCHEMA, SCHEMA_VERSION};
use crate::utils::crypto::sha256_hex;

/// Run all pending migrations
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), SqliteError> {
    // Check if this is a fresh database
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
        tracing::debug!(
            "Database schema is up to date (version {})",
            current_version
        );
        return Ok(());
    }

    // Apply incremental migrations
    for version in (current_version + 1)..=SCHEMA_VERSION {
        tracing::debug!("Applying migration to version {}", version);
        apply_migration(pool, version).await?;
    }

    Ok(())
}

/// Apply the initial schema (version 1)
async fn apply_initial_schema(pool: &SqlitePool) -> Result<(), SqliteError> {
    let start = std::time::Instant::now();

    let mut tx = pool.begin().await?;

    // As one script, not statements split on `;`.
    //
    // `sqlx::query` prepares a single statement, so a multi-statement schema reaches SQLite only as far
    // as its first `;` - and worse, a semicolon inside a `--` comment ends a "statement" mid-table and
    // the fragment after it is a syntax error. That has now cost a debugging session twice, once here and
    // once in the PostgreSQL twin, and no comment is a place anyone looks for a syntax hazard. `raw_sql`
    // sends the script as a simple query, which is what a script is.
    sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;

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

/// The one migration: a v1 database to the current schema.
///
/// # Why there is only one
///
/// No schema above v1 was ever released, so no database exists at v2..v14 and the fourteen historical
/// migrations that used to live here described upgrades nobody could need. Replaying them in sequence
/// also meant replaying two table *rebuilds* that a v1 database does not need at all - it lacks the
/// tables being rebuilt - so the sequence did strictly more work than the destination requires.
///
/// This reaches the destination directly: the columns a v1 database is missing, and the tables it never
/// had, created in their final shape. What each piece is *for* is documented where it is used; the
/// pointers below say which subsystem to look in.
const MIGRATION_V2: &str = r#"
-- files: the content hash algorithm, and the deletion claim that lets cleanup and ingestion agree about
-- a file that is mid-deletion (`claim_file_for_deletion`, `associate_file`).
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
    PRIMARY KEY (project_id, trace_id, file_hash),
    FOREIGN KEY (project_id, file_hash) REFERENCES files(project_id, file_hash) ON DELETE CASCADE
);
INSERT OR IGNORE INTO trace_files_v2 (trace_id, project_id, file_hash)
    SELECT trace_id, project_id, file_hash FROM trace_files;
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
"#;

async fn apply_migration(pool: &SqlitePool, version: i32) -> Result<(), SqliteError> {
    match version {
        // Handled by the initial schema.
        1 => Ok(()),
        2 => apply_versioned_migration(pool, 2, "v1_to_current", MIGRATION_V2).await,
        _ => Err(SqliteError::MigrationFailed {
            version,
            name: "unknown".to_string(),
            error: format!("Unknown migration version: {}", version),
        }),
    }
}

/// Apply a versioned migration with tracking
async fn apply_versioned_migration(
    pool: &SqlitePool,
    version: i32,
    name: &str,
    sql: &str,
) -> Result<(), SqliteError> {
    let start = std::time::Instant::now();

    let mut tx = pool.begin().await?;

    // One script, not statements split on `;`. A semicolon inside a `--` comment or a string literal
    // ends a "statement" mid-construct, and the fragment is then rejected as a syntax error - which is
    // exactly what happened to the PostgreSQL twin's initial schema, where it meant no fresh database
    // could be created at all.
    sqlx::raw_sql(sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| SqliteError::MigrationFailed {
            version,
            name: name.to_string(),
            error: e.to_string(),
        })?;

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
        sqlx::raw_sql(
            "DROP TABLE credentials;
             DROP TABLE credential_project_permissions;
             DROP TABLE deleted_projects;
             DROP TABLE deleted_traces;
             DROP TABLE trace_files;
             CREATE TABLE trace_files (
                 trace_id TEXT NOT NULL,
                 project_id TEXT NOT NULL,
                 file_hash TEXT NOT NULL,
                 PRIMARY KEY (trace_id, file_hash)
             );
             ALTER TABLE files DROP COLUMN hash_algo;
             ALTER TABLE files DROP COLUMN deleting_at;
             ALTER TABLE projects DROP COLUMN deleting_at;
             ALTER TABLE projects DROP COLUMN clean_sweeps;
             ALTER TABLE projects DROP COLUMN last_sweep_at;
             ALTER TABLE organizations DROP COLUMN deleting_at;
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
            apply_migration(&upgraded, version)
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
}
