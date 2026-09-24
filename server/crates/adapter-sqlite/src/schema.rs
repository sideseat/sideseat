//! SQLite schema definitions.
//!
//! Initial schema with all tables. No migrations needed for first version.

/// Current schema version
pub const SCHEMA_VERSION: i32 = 9;

/// Complete schema SQL
pub const SCHEMA: &str = r#"
-- =============================================================================
-- Infrastructure: Schema version tracking
-- =============================================================================
CREATE TABLE IF NOT EXISTS schema_version (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL,
    applied_at INTEGER NOT NULL,
    description TEXT
);

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at INTEGER NOT NULL,
    checksum TEXT NOT NULL,
    execution_time_ms INTEGER,
    success INTEGER NOT NULL DEFAULT 1
);

-- =============================================================================
-- 1. Organizations (must be before projects due to FK)
-- =============================================================================
CREATE TABLE IF NOT EXISTS organizations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK(length(name) >= 1 AND length(name) <= 100),
    slug TEXT NOT NULL UNIQUE CHECK(
        (length(slug) >= 2 AND length(slug) <= 50 AND slug GLOB '[a-z0-9][a-z0-9-]*[a-z0-9]')
        OR (length(slug) = 1 AND slug GLOB '[a-z0-9]')
    ),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- Last, because the v1 migration appends it - see the `files` table for why physical order is part of
    -- the schema. Set while the organization is being deleted, and a tombstone for the same reason a
    -- project's is: deleting the row cascades its project rows away, and those rows *are* the projects'
    -- tombstones, so removing it early would stop the cleanup that collects a stalled writer's spans.
    deleting_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_organizations_slug ON organizations(slug);

-- =============================================================================
-- 2. Users
-- =============================================================================
CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    email TEXT UNIQUE CHECK(email IS NULL OR length(email) >= 3),
    display_name TEXT CHECK(display_name IS NULL OR length(display_name) <= 100),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- =============================================================================
-- 3. Organization Members (references orgs + users)
-- =============================================================================
CREATE TABLE IF NOT EXISTS organization_members (
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member' CHECK(role IN ('viewer', 'member', 'admin', 'owner')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (organization_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_org_members_user ON organization_members(user_id);
CREATE INDEX IF NOT EXISTS idx_org_members_role ON organization_members(organization_id, role);

-- =============================================================================
-- 4. Auth Methods (references users)
-- =============================================================================
CREATE TABLE IF NOT EXISTS auth_methods (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    method_type TEXT NOT NULL CHECK(method_type IN ('bootstrap', 'password', 'oauth', 'passkey', 'api_key')),
    provider TEXT,
    provider_id TEXT,
    credential_hash TEXT,
    metadata TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_auth_methods_user ON auth_methods(user_id);

-- Unique constraint for OAuth: one provider account per user
CREATE UNIQUE INDEX IF NOT EXISTS idx_auth_methods_oauth
    ON auth_methods(method_type, provider, provider_id)
    WHERE provider IS NOT NULL;

-- Unique constraint: one bootstrap method per user
CREATE UNIQUE INDEX IF NOT EXISTS idx_auth_methods_bootstrap
    ON auth_methods(user_id, method_type)
    WHERE method_type = 'bootstrap';

-- =============================================================================
-- 5. Projects (references organizations)
-- =============================================================================
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- The three below are appended by the v1 migration, in this order - see `files` for why the physical
    -- order is part of the schema rather than a presentation choice.
    --
    -- Set while the project is being deleted, and it is a *tombstone*: it outlives the data, not the
    -- other way round. Deletion spans four stores with no transaction over them, so a writer can read
    -- the fence, have the deletion land underneath it, and commit afterwards - no single observation
    -- can rule that out. What the tombstone gives instead is that no *new* writer passes the fence and
    -- that cleanup keeps running for as long as the row exists, so any such write is collected.
    deleting_at INTEGER,
    -- Consecutive sweeps that found no data for this project. The row is removed on the strength of
    -- what has been observed rather than of how long ago the deletion started: a wall-clock grace
    -- period is not a bound on how long a stalled writer can take.
    clean_sweeps INTEGER NOT NULL DEFAULT 0,
    -- When the sweep above was last *counted*. Without it the count measures sweeps rather than elapsed
    -- observation, so N instances sweeping concurrently would reach the required number in one interval
    -- instead of N - the barrier would get weaker the more instances you run.
    last_sweep_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_projects_org ON projects(organization_id);

-- Projects whose row is gone, kept so their cleanup stays discoverable.
--
-- A tombstone is removed on finite evidence, and finite evidence loses to an arbitrarily delayed writer:
-- one that read the fence before the tombstone can commit after the row is gone, and then nothing knows
-- the project ever existed. This does. The sweep keeps deleting any rows that appear for these ids, so a
-- late write is collected however late it is, and an entry is dropped only after a retention long enough
-- that no request could still be in flight.
CREATE TABLE IF NOT EXISTS deleted_projects (
    project_id TEXT PRIMARY KEY,
    deleted_at INTEGER NOT NULL,
    -- When this id was last checked for stray rows. The records are kept forever, so without a window
    -- every instance would re-check every deletion ever made on every sweep: work proportional to
    -- instances times lifetime deletions. Claiming the check by moving this forward makes it one check
    -- per id per window, whatever the instance count.
    last_checked_at INTEGER,
    -- How many consecutive checks found nothing. Each one pushes the next check further out, so a project
    -- deleted long ago is not re-checked at the same rate as one deleted a minute ago - without this, a
    -- hundred thousand historical deletions meant a hundred thousand storage listings every window,
    -- forever.
    quiet_checks INTEGER NOT NULL DEFAULT 0,
    -- When this id is next due, materialised rather than computed from `last_checked_at` and
    -- `quiet_checks` at query time. An index on an input to the eligibility expression bounds the rows
    -- *returned*, not the rows examined: with many heavily backed-off records the planner still walks a
    -- large part of a table that only ever grows. Indexing the due time itself makes discovery genuinely
    -- bounded.
    -- NOT NULL, and set when the record is created. Left null, PostgreSQL sorts it *last* (SQLite sorts
    -- nulls first), so a freshly deleted project queued behind every overdue record - hours or days on a
    -- backlog, with its late files and rows hidden throughout.
    next_check_at INTEGER NOT NULL DEFAULT 0,
    -- Which claim owns the current check. A report carries the token it was claimed with and updates
    -- nothing if it no longer matches, so a worker whose lease expired mid-batch cannot overwrite the
    -- schedule or the result of the worker that took the id after it.
    claim_token INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_deleted_projects_due ON deleted_projects(next_check_at);

-- =============================================================================
-- 6. Files metadata (references projects)
-- =============================================================================
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    file_hash TEXT NOT NULL,
    media_type TEXT,
    size_bytes INTEGER NOT NULL,
    ref_count INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- Last, and so is `deleting_at`, because both are added by the v1 migration and `ALTER TABLE ADD
    -- COLUMN` can only append. Declared mid-table, a fresh database and an upgraded one differ in physical
    -- column *order* for the same schema version - harmless while every writer names its columns, and
    -- silently catastrophic the moment one is positional. That is not hypothetical: DuckDB's metrics
    -- appender is positional, and the same mistake there wrote every value one column across.
    hash_algo TEXT NOT NULL DEFAULT 'sha256',
    -- Set while cleanup is deleting this file. Association refuses through the fence, because a count
    -- cannot express "deletion in progress" and the bytes may already be gone.
    deleting_at INTEGER,
    UNIQUE(project_id, file_hash)
);

CREATE INDEX IF NOT EXISTS idx_files_project ON files(project_id);
CREATE INDEX IF NOT EXISTS idx_files_ref_zero ON files(project_id) WHERE ref_count = 0;
CREATE INDEX IF NOT EXISTS idx_files_created ON files(project_id, created_at);

-- =============================================================================
-- 7. Trace deletion tombstones
-- =============================================================================
--
-- A trace deletion has to close a race the file fence alone cannot: an ingest of trace X can be in
-- flight while `delete_traces` runs, and its analytics row commits *after* the delete removed the
-- file association and reclaimed the bytes - leaving a dangling `#!B64!#` reference the delete
-- already returned 204 for. Ingest consults the tombstone immediately before its analytics write, and
-- drops the spans of any trace named here, so a queued redelivery collapses to a no-op rather than
-- resurrecting the deleted trace.
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

-- Retention's cleanup **intent**, recorded before the spans are deleted.
--
-- DuckDB commits the span deletion first and the file and favourite cleanup runs afterwards, asynchronously,
-- with failures only logged. A crash or a transactional-store outage in between loses the **only** record of
-- which traces needed cleaning - and their spans are already gone, so a later pass cannot rediscover them.
-- Their file associations, ref-counted bytes and favourites are then orphaned permanently, which is the quota
-- leak the whole file protocol exists to prevent.
--
-- Recorded *before* the delete, so the record survives a crash on either side of it, and removed only once
-- the cleanup has completed.
--
-- **One state, not three.** The plan this comes from specified `pending` / `ready` / `aborted`, because at the
-- time cleanup was trace-wide and acting on a candidate whose deletion had *not* committed would have deleted
-- a live trace's associations. Cleanup is now survivor reconciliation (`reconcile_trace_survivors`), which
-- releases only what no surviving winning span references and skips anything with `pending_writers > 0` - so
-- running it on a trace whose deletion failed is a no-op, and running it twice is idempotent. There is
-- therefore nothing for the extra states to protect, and a state machine with no failure to distinguish is a
-- state machine to get wrong.
--
-- Leased and backed off like the other sweep tables: a claim pushes `next_attempt_at` out before returning, so
-- a slow reconciliation is not re-claimed while it runs, and a candidate that keeps failing is retried more
-- slowly rather than spinning.
CREATE TABLE IF NOT EXISTS retention_cleanup (
    project_id      TEXT    NOT NULL,
    trace_id        TEXT    NOT NULL,
    created_at      INTEGER NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    claim_token     INTEGER NOT NULL DEFAULT 0,
    logical_bytes   INTEGER NOT NULL DEFAULT 0 CHECK(logical_bytes >= 0),
    PRIMARY KEY (project_id, trace_id)
);
-- On the due time itself, not on an input to it: an index on a column the eligibility expression merely reads
-- bounds rows returned rather than rows examined.
CREATE INDEX IF NOT EXISTS idx_retention_cleanup_due ON retention_cleanup(next_attempt_at);
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


-- =============================================================================
-- 8. Trace Files junction table
-- =============================================================================
CREATE TABLE IF NOT EXISTS trace_files (
    trace_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    file_hash TEXT NOT NULL,
    -- An association is created while the batch that references it is still in flight - files are written
    -- before the rows that name them, so a failed batch has created associations for spans that will never
    -- exist, and an association holds `ref_count` above zero (the orphan sweeper selects on zero). A boolean
    -- "provisional" flag could not express the one case that matters under concurrency: *several* batches
    -- carrying the same `(project, trace, hash)` at once. With a flag, whichever failed first deleted the row
    -- another still-in-flight or just-committed batch depended on, orphaning its file - and no number of
    -- reads fixes it, because a read and a release are not atomic.
    --
    -- Two facts instead of one:
    --   `pending_writers` - how many referencing batches have not yet resolved. Incremented per reference
    --     (create or share), decremented by that batch's confirm or release.
    --   `durable` - set the moment *any* referencing batch commits its analytics rows, and never unset. A
    --     durable association is backed by a committed row, so it is kept regardless of `pending_writers`.
    -- A release deletes the row only when it is not durable *and* no writer is still pending, so a failing
    -- batch can never orphan a file another batch committed or is about to.
    pending_writers INTEGER NOT NULL DEFAULT 0,
    durable INTEGER NOT NULL DEFAULT 0,
    -- Project first: a trace id comes from the client, so two projects can present the same one.
    -- Keyed without the project, one project's association satisfied `INSERT OR IGNORE` for the
    -- other, leaving the second with no association and a reference nothing would release.
    PRIMARY KEY (project_id, trace_id, file_hash),
    FOREIGN KEY (project_id, file_hash) REFERENCES files(project_id, file_hash) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_trace_files_trace ON trace_files(trace_id);
CREATE INDEX IF NOT EXISTS idx_trace_files_project ON trace_files(project_id);
-- The derived reference count is a COUNT over (project_id, file_hash). The primary key leads with
-- project_id but separates the two by trace_id, so without this the count scans a project.
CREATE INDEX IF NOT EXISTS idx_trace_files_project_hash ON trace_files(project_id, file_hash);

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

-- =============================================================================
-- 8. Favorites (user-scoped, references users and projects)
-- =============================================================================
CREATE TABLE IF NOT EXISTS favorites (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    entity_type TEXT NOT NULL CHECK(entity_type IN ('trace', 'session', 'span')),
    entity_id TEXT NOT NULL,
    secondary_id TEXT,
    created_at INTEGER NOT NULL
);

-- Partial indexes for uniqueness: SQLite NULL != NULL in UNIQUE constraints
-- Simple entities (trace, session): secondary_id is NULL
CREATE UNIQUE INDEX IF NOT EXISTS idx_favorites_simple
    ON favorites(user_id, project_id, entity_type, entity_id)
    WHERE secondary_id IS NULL;
-- Spans: secondary_id is span_id (not NULL)
CREATE UNIQUE INDEX IF NOT EXISTS idx_favorites_span
    ON favorites(user_id, project_id, entity_type, entity_id, secondary_id)
    WHERE secondary_id IS NOT NULL;

-- Query indexes
CREATE INDEX IF NOT EXISTS idx_favorites_user_project ON favorites(user_id, project_id);
CREATE INDEX IF NOT EXISTS idx_favorites_lookup ON favorites(user_id, project_id, entity_type, entity_id);
-- Cleanup index (for retention/delete operations without user_id)
CREATE INDEX IF NOT EXISTS idx_favorites_cleanup ON favorites(project_id, entity_type, entity_id);

-- =============================================================================
-- 9. API Keys (references organizations and users)
-- =============================================================================
CREATE TABLE IF NOT EXISTS api_keys (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK(length(name) >= 1 AND length(name) <= 100),
    key_hash TEXT NOT NULL UNIQUE,
    key_prefix TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'full' CHECK(scope IN ('read', 'ingest', 'write', 'full')),
    created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    last_used_at INTEGER,
    expires_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_api_keys_org_created ON api_keys(org_id, created_at DESC);

-- =============================================================================
-- Default Data (inserted in dependency order)
-- =============================================================================

-- 1. Default organization
INSERT OR IGNORE INTO organizations (id, name, slug, created_at, updated_at)
VALUES ('default', 'Default Organization', 'default', strftime('%s', 'now'), strftime('%s', 'now'));

-- 2. Default user
INSERT OR IGNORE INTO users (id, display_name, created_at, updated_at)
VALUES ('local', 'Local User', strftime('%s', 'now'), strftime('%s', 'now'));

-- 3. Default membership (user owns default org)
INSERT OR IGNORE INTO organization_members (organization_id, user_id, role, created_at, updated_at)
VALUES ('default', 'local', 'owner', strftime('%s', 'now'), strftime('%s', 'now'));

-- 4. Default auth method (bootstrap for local user)
INSERT OR IGNORE INTO auth_methods (id, user_id, method_type, created_at, updated_at)
VALUES ('bootstrap-local', 'local', 'bootstrap', strftime('%s', 'now'), strftime('%s', 'now'));

-- 5. Default project (in default org)
INSERT OR IGNORE INTO projects (id, organization_id, name, created_at, updated_at)
VALUES ('default', 'default', 'Default Project', strftime('%s', 'now'), strftime('%s', 'now'));

-- =============================================================================
-- 10. Credentials (references organizations and users)
-- =============================================================================
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

-- =============================================================================
-- 11. Credential Project Permissions
-- =============================================================================
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

-- Uniqueness: one rule per (credential, project) when project is specified
CREATE UNIQUE INDEX IF NOT EXISTS idx_cred_perms_unique_project
    ON credential_project_permissions(credential_id, project_id)
    WHERE project_id IS NOT NULL;

-- One org-level default per credential (project_id IS NULL)
CREATE UNIQUE INDEX IF NOT EXISTS idx_cred_perms_unique_org_default
    ON credential_project_permissions(credential_id)
    WHERE project_id IS NULL;

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
    logical_bytes INTEGER NOT NULL DEFAULT 0 CHECK(logical_bytes >= 0),
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

-- Staged OTLP payload registry. Intentionally no TTL or expiry column.
CREATE TABLE IF NOT EXISTS staged_payloads (
    id               TEXT    PRIMARY KEY,
    project_id       TEXT    NOT NULL,
    signal           TEXT    NOT NULL CHECK(signal IN ('traces', 'metrics', 'logs')),
    blob_hash        TEXT    NOT NULL,
    byte_len         INTEGER NOT NULL CHECK(byte_len >= 0),
    created_at       INTEGER NOT NULL,
    redrive_attempts INTEGER NOT NULL DEFAULT 0 CHECK(redrive_attempts >= 0),
    unconfirmed      INTEGER NOT NULL DEFAULT 0 CHECK(unconfirmed IN (0, 1)),
    records_json     TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_staged_payloads_pending
    ON staged_payloads(unconfirmed, created_at, id);

-- Project-wide legal holds, the shared hold/retention lease, and best-effort storage accounting.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_contains_required_tables() {
        let required_tables = [
            "schema_version",
            "schema_migrations",
            "organizations",
            "users",
            "organization_members",
            "auth_methods",
            "projects",
            "files",
            "trace_files",
            "content_bodies",
            "span_bodies",
            "content_body_backfill",
            "favorites",
            "api_keys",
            "credentials",
            "credential_project_permissions",
            "staged_payloads",
        ];

        for table in required_tables {
            assert!(
                SCHEMA.contains(&format!("CREATE TABLE IF NOT EXISTS {}", table)),
                "Schema missing table: {}",
                table
            );
        }
    }

    #[test]
    fn staged_payloads_have_no_ttl_or_expiry_column() {
        let table = SCHEMA
            .split("CREATE TABLE IF NOT EXISTS staged_payloads")
            .nth(1)
            .and_then(|tail| tail.split(");").next())
            .expect("staged payload table");
        let lower = table.to_ascii_lowercase();
        assert!(!lower.contains("ttl"));
        assert!(!lower.contains("expire"));
    }

    #[test]
    fn test_schema_contains_default_data() {
        assert!(
            SCHEMA.contains("INSERT OR IGNORE INTO organizations"),
            "Schema missing default organization"
        );
        assert!(
            SCHEMA.contains("INSERT OR IGNORE INTO users"),
            "Schema missing default user"
        );
        assert!(
            SCHEMA.contains("INSERT OR IGNORE INTO organization_members"),
            "Schema missing default membership"
        );
        assert!(
            SCHEMA.contains("INSERT OR IGNORE INTO auth_methods"),
            "Schema missing default auth method"
        );
        assert!(
            SCHEMA.contains("INSERT OR IGNORE INTO projects"),
            "Schema missing default project"
        );
    }
}
