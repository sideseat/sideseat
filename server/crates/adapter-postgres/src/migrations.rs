//! Versioned PostgreSQL schema migration registry.
//!
//! Fresh databases install the current schema directly. Existing databases hold a session-level advisory lock
//! while applying every immutable migration after their recorded version.

use sqlx::postgres::PgConnection;
use sqlx::{Acquire, PgPool};

use super::error::PostgresError;
use super::schema::{DEFAULT_DATA, SCHEMA, SCHEMA_VERSION, TENANT_RLS_SQL};
use sideseat_core::migration::{MigrationRun, plan_migrations};
use sideseat_ports::clock::Clock;

struct Migration {
    version: i32,
    name: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 2,
        name: "v1_to_current",
    },
    Migration {
        version: 3,
        name: "retention_cleanup_intent",
    },
    Migration {
        version: 4,
        name: "deletion_journal",
    },
    Migration {
        version: 5,
        name: "deletion_journal_span_id_check",
    },
    Migration {
        version: 6,
        name: "staged_payloads",
    },
    Migration {
        version: 7,
        name: "storage_governance",
    },
    Migration {
        version: 8,
        name: "retention_cleanup_logical_bytes",
    },
    Migration {
        version: 9,
        name: "content_bodies",
    },
    Migration {
        version: 10,
        name: "tenant_row_level_security",
    },
];

/// Run all pending migrations.
///
/// Uses `pg_advisory_lock` to prevent concurrent migration execution
/// across multiple application instances. A dedicated connection is held
/// for the entire migration process — advisory locks are session-level
/// and must be acquired and released on the same connection.
pub async fn run_migrations(pool: &PgPool, clock: &dyn Clock) -> Result<(), PostgresError> {
    // Lock ID 0x5364_5365_6174 ("SdSeat" in hex) avoids collision with other apps.
    const MIGRATION_LOCK_ID: i64 = 0x5364_5365;

    let mut conn = pool.acquire().await?;

    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK_ID)
        .execute(&mut *conn)
        .await?;

    let result = run_migrations_inner(&mut conn, clock).await;

    // Advisory locks are session-scoped, so release through the same dedicated connection even after failure.
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_ID)
        .execute(&mut *conn)
        .await;

    result
}

async fn run_migrations_inner(
    conn: &mut PgConnection,
    clock: &dyn Clock,
) -> Result<(), PostgresError> {
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
        tracing::debug!("Applying initial PostgreSQL schema v{}", SCHEMA_VERSION);
        apply_initial_schema(&mut *conn, clock).await?;
        return Ok(());
    }

    let current_version: Option<i32> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
            .fetch_optional(&mut *conn)
            .await?;

    match migration_run(current_version)? {
        MigrationRun::Initialize { .. } => {
            tracing::debug!("Applying initial PostgreSQL schema v{}", SCHEMA_VERSION);
            apply_initial_schema(&mut *conn, clock).await?;
        }
        MigrationRun::Apply(steps) => {
            tracing::debug!(
                "Migrating PostgreSQL schema from v{} to v{}",
                current_version.expect("an incremental run has a current version"),
                SCHEMA_VERSION
            );
            for step in steps {
                apply_versioned_migration(&mut *conn, step.version, clock).await?;
            }
        }
        MigrationRun::UpToDate { version } => {
            tracing::debug!("PostgreSQL schema is up to date (v{})", version);
        }
    }

    Ok(())
}

fn migration_run(current: Option<i32>) -> Result<MigrationRun, PostgresError> {
    plan_migrations(
        current,
        SCHEMA_VERSION,
        1,
        MIGRATIONS
            .iter()
            .map(|migration| (migration.version, migration.name)),
    )
    .map_err(|error| PostgresError::MigrationFailed {
        version: error.version(),
        name: "version_check".to_string(),
        error: error.to_string(),
    })
}

/// Install the current schema and bootstrap data on a fresh database.
async fn apply_initial_schema(
    conn: &mut PgConnection,
    clock: &dyn Clock,
) -> Result<(), PostgresError> {
    let now = clock.now().timestamp();

    let mut tx = conn.begin().await?;

    // Execute each SQL document as a script so comments and literals retain normal SQL parsing.
    sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;
    sqlx::raw_sql(TENANT_RLS_SQL).execute(&mut *tx).await?;
    sqlx::raw_sql(DEFAULT_DATA).execute(&mut *tx).await?;

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

/// Apply one versioned migration and its metadata update atomically.
async fn apply_versioned_migration(
    conn: &mut PgConnection,
    version: i32,
    clock: &dyn Clock,
) -> Result<(), PostgresError> {
    let start = std::time::Instant::now();
    let now = clock.now().timestamp();
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
        // Released migration scripts are immutable: databases already at v2 execute this step next.
        3 => (
            "retention_cleanup_intent",
            r#"
CREATE TABLE IF NOT EXISTS retention_cleanup (
    project_id      TEXT   NOT NULL,
    trace_id        TEXT   NOT NULL,
    created_at      BIGINT NOT NULL,
    attempts        BIGINT NOT NULL DEFAULT 0,
    next_attempt_at BIGINT NOT NULL DEFAULT 0,
    claim_token     BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, trace_id)
);
CREATE INDEX IF NOT EXISTS idx_retention_cleanup_due ON retention_cleanup(next_attempt_at);
"#,
        ),
        // Databases already at v3 execute this immutable step next.
        4 => (
            "deletion_journal",
            r#"
-- =============================================================================
-- Deletion journal: the deletions a restore cannot recompute
-- =============================================================================
--
-- Append-only, permanent, exempt from every sweep. See the SQLite twin and
-- `sideseat_ports::traits::DeletionJournal` for the full reasoning. The short version is that a snapshot
-- predating a deletion predates its tombstone too, so a restore needs a record it can replay forward, and the
-- staged-payload re-drive sweep needs to tell a failed write from a deliberate deletion.
--
-- Age retention writes nothing here: it is a predicate, so a restored database recomputes the same verdict.
--
-- `BIGSERIAL`, not `SERIAL`: the journal is permanent and never truncated, so a 2^31 id space is a bound on how
-- many deletions a deployment may ever record. The same mistake the `files` surrogate key was migrated out of.
CREATE TABLE IF NOT EXISTS deletion_journal (
    sequence    BIGSERIAL PRIMARY KEY,
    project_id  TEXT   NOT NULL,
    cause       TEXT   NOT NULL CHECK(cause IN ('requested', 'pressure')),
    scope       TEXT   NOT NULL CHECK(scope IN ('trace', 'session', 'project', 'organization', 'span')),
    target_id   TEXT   NOT NULL,
    -- Set only for a span-scoped entry, where `target_id` is the span's trace.
    span_id     TEXT,
    recorded_at BIGINT NOT NULL,
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
"#,
        ),
        // Existing malformed shapes are handled differently because a journal row is evidence. A span-scoped row
        // with no span id is inert - `deletion_is_journaled` matches on `span_id`, so it was never findable - and
        // is deleted. Any other scope carrying a stray `span_id` is read *correctly* today, so its column is
        // normalised and the row kept: deleting it would lose a real deletion record, and a restore predating
        // that deletion would bring its target back.
        //
        // `NOT VALID` deliberately omits `VALIDATE`. `ADD CONSTRAINT`
        // takes `ACCESS EXCLUSIVE`, and PostgreSQL holds it until the surrounding transaction commits - so
        // validating here would run its full scan under that lock and block every journal read and write, on the
        // one table designed to grow without bound. It does not need validating: the statements above make every
        // existing row conform, so the only rows the constraint could reject are future ones, which `NOT VALID`
        // checks exactly as a validated constraint would. `VALIDATE` would only re-confirm what this migration
        // just established, at the cost of the lock.
        5 => (
            "deletion_journal_span_id_check",
            r#"
DELETE FROM deletion_journal WHERE scope = 'span' AND span_id IS NULL;
UPDATE deletion_journal SET span_id = NULL WHERE scope <> 'span' AND span_id IS NOT NULL;
ALTER TABLE deletion_journal DROP CONSTRAINT IF EXISTS deletion_journal_span_scope_check;
ALTER TABLE deletion_journal
    ADD CONSTRAINT deletion_journal_span_scope_check
    CHECK ((scope = 'span') = (span_id IS NOT NULL)) NOT VALID;
"#,
        ),
        6 => (
            "staged_payloads",
            r#"
CREATE TABLE IF NOT EXISTS staged_payloads (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    signal TEXT NOT NULL CHECK(signal IN ('traces', 'metrics', 'logs')),
    blob_hash TEXT NOT NULL,
    byte_len BIGINT NOT NULL CHECK(byte_len >= 0),
    created_at BIGINT NOT NULL,
    redrive_attempts BIGINT NOT NULL DEFAULT 0 CHECK(redrive_attempts >= 0),
    unconfirmed BOOLEAN NOT NULL DEFAULT FALSE,
    records_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_staged_payloads_pending
    ON staged_payloads(unconfirmed, created_at, id);
"#,
        ),
        7 => (
            "storage_governance",
            r#"
ALTER TABLE deletion_journal
    ADD COLUMN logical_bytes BIGINT NOT NULL DEFAULT 0 CHECK(logical_bytes >= 0);
UPDATE deletion_journal
SET logical_bytes =
    64
    + octet_length(project_id)
    + octet_length(cause)
    + octet_length(scope)
    + octet_length(target_id)
    + octet_length(COALESCE(span_id, ''));
CREATE TABLE IF NOT EXISTS project_holds (
    project_id TEXT PRIMARY KEY,
    hold_until BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_project_holds_active ON project_holds(hold_until, project_id);
CREATE TABLE IF NOT EXISTS project_maintenance_leases (
    project_id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    lease_until BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_project_maintenance_leases_expiry
    ON project_maintenance_leases(lease_until);
CREATE TABLE IF NOT EXISTS project_storage_usage (
    project_id TEXT PRIMARY KEY,
    logical_bytes BIGINT NOT NULL DEFAULT 0 CHECK(logical_bytes >= 0),
    updated_at BIGINT NOT NULL
);
"#,
        ),
        8 => (
            "retention_cleanup_logical_bytes",
            r#"
ALTER TABLE retention_cleanup
    ADD COLUMN logical_bytes BIGINT NOT NULL DEFAULT 0 CHECK(logical_bytes >= 0);
UPDATE retention_cleanup
SET logical_bytes =
    64
    + octet_length(project_id)
    + octet_length(trace_id);
"#,
        ),
        9 => (
            "content_bodies",
            r#"
CREATE TABLE IF NOT EXISTS content_bodies (
    project_id TEXT NOT NULL,
    body_hash TEXT NOT NULL,
    logical_bytes BIGINT NOT NULL CHECK(logical_bytes >= 0),
    created_at BIGINT NOT NULL,
    last_referenced_at BIGINT NOT NULL,
    deleting_at BIGINT,
    PRIMARY KEY (project_id, body_hash)
);
CREATE TABLE IF NOT EXISTS span_bodies (
    project_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    span_id TEXT NOT NULL,
    field TEXT NOT NULL CHECK(field IN ('messages', 'tool_definitions', 'tool_names', 'raw_span')),
    body_hash TEXT NOT NULL,
    pending_writers INTEGER NOT NULL DEFAULT 0 CHECK(pending_writers >= 0),
    durable BOOLEAN NOT NULL DEFAULT FALSE,
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
    complete BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at BIGINT NOT NULL
);
"#,
        ),
        10 => ("tenant_row_level_security", TENANT_RLS_SQL),
        _ => {
            return Err(PostgresError::MigrationFailed {
                version,
                name: "unknown".to_string(),
                error: format!("No migration defined for version {}", version),
            });
        }
    };

    let mut tx = conn.begin().await?;

    // Execute the migration as a script so comments and literals retain normal SQL parsing.
    sqlx::raw_sql(sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| PostgresError::MigrationFailed {
            version,
            name: name.to_string(),
            error: e.to_string(),
        })?;

    let elapsed = start.elapsed().as_millis() as i64;

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
    sideseat_core::utils::crypto::sha256_hex(sql)
}
