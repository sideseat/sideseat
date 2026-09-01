//! PostgreSQL migration management
//!
//! Handles schema initialization and versioned migrations.

use sqlx::postgres::PgConnection;
use sqlx::{Acquire, PgPool};

use super::error::PostgresError;
use super::schema::{DEFAULT_DATA, SCHEMA, SCHEMA_VERSION};

/// Run all pending migrations.
///
/// Uses `pg_advisory_lock` to prevent concurrent migration execution
/// across multiple application instances. A dedicated connection is held
/// for the entire migration process — advisory locks are session-level
/// and must be acquired and released on the same connection.
pub async fn run_migrations(pool: &PgPool) -> Result<(), PostgresError> {
    // Acquire advisory lock to prevent concurrent migrations.
    // Lock ID 0x5364_5365_6174 ("SdSeat" in hex) avoids collision with other apps.
    const MIGRATION_LOCK_ID: i64 = 0x5364_5365;

    let mut conn = pool.acquire().await?;

    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK_ID)
        .execute(&mut *conn)
        .await?;

    let result = run_migrations_inner(&mut conn).await;

    // Always release the advisory lock, even on error
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_ID)
        .execute(&mut *conn)
        .await;

    result
}

async fn run_migrations_inner(conn: &mut PgConnection) -> Result<(), PostgresError> {
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
    .fetch_one(&mut *conn)
    .await?;

    if !table_exists {
        // Fresh database - apply initial schema
        tracing::debug!("Applying initial PostgreSQL schema v{}", SCHEMA_VERSION);
        apply_initial_schema(&mut *conn).await?;
        return Ok(());
    }

    // Get current version
    let current_version: Option<i32> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
            .fetch_optional(&mut *conn)
            .await?;

    match current_version {
        None => {
            // Table exists but no version row - apply schema
            tracing::debug!("Applying initial PostgreSQL schema v{}", SCHEMA_VERSION);
            apply_initial_schema(&mut *conn).await?;
        }
        Some(v) if v < SCHEMA_VERSION => {
            // Run incremental migrations
            tracing::debug!(
                "Migrating PostgreSQL schema from v{} to v{}",
                v,
                SCHEMA_VERSION
            );
            for version in (v + 1)..=SCHEMA_VERSION {
                apply_versioned_migration(&mut *conn, version).await?;
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
async fn apply_initial_schema(conn: &mut PgConnection) -> Result<(), PostgresError> {
    let now = chrono::Utc::now().timestamp();

    let mut tx = conn.begin().await?;

    // One script, not statements split on `;`.
    //
    // Splitting was silently fatal: a semicolon inside a `--` comment ended a "statement" mid-table, so
    // PostgreSQL rejected the fragment with "syntax error at end of input" and *no* fresh PostgreSQL
    // database could be created at all. A comment is not a place anyone looks for a syntax hazard, and
    // the same trap is waiting in any string literal containing a semicolon. `raw_sql` sends the script
    // as a simple query, which is what a script is.
    sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;
    sqlx::raw_sql(DEFAULT_DATA).execute(&mut *tx).await?;

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
async fn apply_versioned_migration(
    conn: &mut PgConnection,
    version: i32,
) -> Result<(), PostgresError> {
    let start = std::time::Instant::now();
    let now = chrono::Utc::now().timestamp();

    // One migration, because no schema above v1 was ever released - see the SQLite twin for the full
    // reasoning. This reaches the current schema directly rather than replaying fourteen steps, two of
    // which rebuilt tables a v1 database does not have.
    let (name, sql): (&str, &str) = match version {
        2 => (
            "v1_to_current",
            r#"-- files: the content hash algorithm, and the deletion claim that lets cleanup and
-- ingestion agree about a file that is mid-deletion.
ALTER TABLE files ADD COLUMN IF NOT EXISTS hash_algo TEXT NOT NULL DEFAULT 'sha256';
ALTER TABLE files ADD COLUMN IF NOT EXISTS deleting_at BIGINT;
-- 64-bit counters: an INTEGER ref_count is a decode failure waiting to happen against an i64 in Rust,
-- which SQLite never hit because its INTEGER is already 64-bit. The surrogate key too - a v1 database
-- declared it SERIAL while a fresh v2 declares BIGSERIAL, and two schemas for one version is exactly what
-- the upgrade test exists to refuse. `ALTER TYPE` on the column is enough: BIGSERIAL is BIGINT plus a
-- sequence default, and the sequence itself is already 64-bit in PostgreSQL.
ALTER TABLE files ALTER COLUMN ref_count TYPE BIGINT;
ALTER TABLE files ALTER COLUMN id TYPE BIGINT;
-- And the *sequence*, which `ALTER COLUMN ... TYPE` does not touch. A v1 `SERIAL` owns an `integer`
-- sequence, so widening only the column leaves the id space bounded at 2^31 while a fresh `BIGSERIAL`
-- reaches 2^63 - the two schemas would differ in the one dimension that eventually stops inserts.
ALTER SEQUENCE IF EXISTS files_id_seq AS bigint MAXVALUE 9223372036854775807;

-- projects / organizations: the deletion tombstone plus the repeated-observation counters that decide
-- when it may go. Driven by what has been observed, never by elapsed time.
ALTER TABLE projects ADD COLUMN IF NOT EXISTS deleting_at BIGINT;
ALTER TABLE projects ADD COLUMN IF NOT EXISTS clean_sweeps BIGINT NOT NULL DEFAULT 0;
ALTER TABLE projects ADD COLUMN IF NOT EXISTS last_sweep_at BIGINT;
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS deleting_at BIGINT;

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
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
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
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cred_perms_credential ON credential_project_permissions(credential_id);
CREATE INDEX IF NOT EXISTS idx_cred_perms_project ON credential_project_permissions(project_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_cred_perms_unique_project
    ON credential_project_permissions(credential_id, project_id)
    WHERE project_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_cred_perms_unique_org_default
    ON credential_project_permissions(credential_id)
    WHERE project_id IS NULL;

-- trace_files keyed with the project first. A trace id comes from the client, so two projects can
-- present the same one, and keyed without the project one project's association satisfied the other's
-- conflict clause. Postgres can swap a primary key in place.
-- Two facts, not a boolean: see the schema comment. `pending_writers` counts in-flight referencing
-- batches; `durable` is set once any of them commits. A release deletes only a non-durable row at zero
-- pending, so concurrent batches sharing one association cannot orphan each other's file. Existing rows
-- default to durable=false, pending_writers=0 - a state a release leaves untouched, so a legacy row is
-- kept until its trace is deleted rather than swept out from under a committed span.
ALTER TABLE trace_files ADD COLUMN IF NOT EXISTS pending_writers INTEGER NOT NULL DEFAULT 0;
ALTER TABLE trace_files ADD COLUMN IF NOT EXISTS durable BOOLEAN NOT NULL DEFAULT FALSE;
-- Existing rows are committed associations, so they are durable: a fresh column default of FALSE would put
-- them one release away from deletion, which is data loss. New rows still default to FALSE (provisional).
UPDATE trace_files SET durable = TRUE;
ALTER TABLE trace_files DROP CONSTRAINT IF EXISTS trace_files_pkey;
ALTER TABLE trace_files ADD PRIMARY KEY (project_id, trace_id, file_hash);
CREATE INDEX IF NOT EXISTS idx_trace_files_project_hash ON trace_files(project_id, file_hash);

-- Deletion records kept permanently, leased and backed off, indexed on the due time itself.
-- `next_check_at` is NOT NULL because the eligibility test is `next_check_at <= now`, which no null
-- satisfies - and PostgreSQL sorts nulls *last*, so a nullable column also queued a fresh deletion
-- behind the whole backlog.
CREATE TABLE IF NOT EXISTS deleted_projects (
    project_id TEXT PRIMARY KEY,
    deleted_at BIGINT NOT NULL,
    last_checked_at BIGINT,
    quiet_checks BIGINT NOT NULL DEFAULT 0,
    next_check_at BIGINT NOT NULL DEFAULT 0,
    claim_token BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_deleted_projects_due ON deleted_projects(next_check_at);

-- The trace deletion tombstone; see the SQLite twin.
CREATE TABLE IF NOT EXISTS deleted_traces (
    project_id  TEXT    NOT NULL,
    trace_id    TEXT    NOT NULL,
    deleted_at  BIGINT NOT NULL,
    -- The same leased, backed-off schedule the deleted-project records use, and for the same reason: the
    -- pre-write check and the analytics write are in different stores, so a crash between them leaves
    -- spans for a deleted trace and only a sweep can collect them. Re-checking every record forever at a
    -- fixed rate would be unbounded lifetime work, so a quiet check pushes the next one further out and
    -- the due time itself is indexed.
    quiet_checks  BIGINT NOT NULL DEFAULT 0,
    next_check_at BIGINT NOT NULL DEFAULT 0,
    claim_token   BIGINT NOT NULL DEFAULT 0,
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
    deleted_at  BIGINT NOT NULL,
    -- The same leased, backed-off schedule the other deletion records use, and for the same reason.
    quiet_checks  BIGINT NOT NULL DEFAULT 0,
    next_check_at BIGINT NOT NULL DEFAULT 0,
    claim_token   BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, session_id)
);
CREATE INDEX IF NOT EXISTS idx_deleted_sessions_due ON deleted_sessions(next_check_at);

"#,
        ),
        _ => {
            return Err(PostgresError::MigrationFailed {
                version,
                name: "unknown".to_string(),
                error: format!("No migration defined for version {}", version),
            });
        }
    };

    let mut tx = conn.begin().await?;

    // As one script - see `apply_initial_schema` for why splitting on `;` is a trap.
    sqlx::raw_sql(sql)
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
