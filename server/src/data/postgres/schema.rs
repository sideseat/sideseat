//! PostgreSQL schema definitions
//!
//! Initial schema with all tables. Compatible with SQLite schema structure.

/// Current schema version
pub const SCHEMA_VERSION: i32 = 2;

/// Complete schema SQL for PostgreSQL
pub const SCHEMA: &str = r#"
-- =============================================================================
-- Infrastructure: Schema version tracking
-- =============================================================================
CREATE TABLE IF NOT EXISTS schema_version (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL,
    applied_at BIGINT NOT NULL,
    description TEXT
);

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at BIGINT NOT NULL,
    checksum TEXT NOT NULL,
    execution_time_ms INTEGER,
    success BOOLEAN NOT NULL DEFAULT TRUE
);

-- =============================================================================
-- 1. Organizations (must be before projects due to FK)
-- =============================================================================
CREATE TABLE IF NOT EXISTS organizations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK(length(name) >= 1 AND length(name) <= 100),
    slug TEXT NOT NULL UNIQUE CHECK(
        (length(slug) >= 2 AND length(slug) <= 50 AND slug ~ '^[a-z0-9][a-z0-9-]*[a-z0-9]$')
        OR (length(slug) = 1 AND slug ~ '^[a-z0-9]$')
    ),
    -- Set while the organization is being deleted, and a tombstone for the same reason a project's is:
    -- deleting the row cascades its project rows away, and those rows *are* the projects' tombstones, so
    -- removing it early would stop the cleanup that collects a stalled writer's spans.
    deleting_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_organizations_slug ON organizations(slug);

-- =============================================================================
-- 2. Users
-- =============================================================================
CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    email TEXT UNIQUE CHECK(email IS NULL OR length(email) >= 3),
    display_name TEXT CHECK(display_name IS NULL OR length(display_name) <= 100),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

-- =============================================================================
-- 3. Organization Members (references orgs + users)
-- =============================================================================
CREATE TABLE IF NOT EXISTS organization_members (
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member' CHECK(role IN ('viewer', 'member', 'admin', 'owner')),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
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
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
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
    -- Set while the project is being deleted - a tombstone, see the SQLite twin for why it outlives
    -- the data rather than the other way round.
    deleting_at BIGINT,
    -- Consecutive sweeps that found no data for this project. Removal follows what has been observed,
    -- not how long ago the deletion started.
    clean_sweeps BIGINT NOT NULL DEFAULT 0,
    -- When the sweep above was last *counted*. Without it the count measures sweeps rather than elapsed
    -- observation, so N instances sweeping concurrently would reach the required number in one interval
    -- instead of N - the barrier would get weaker the more instances you run.
    last_sweep_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
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
    deleted_at BIGINT NOT NULL,
    -- When this id was last checked for stray rows. The records are kept forever, so without a window
    -- every instance would re-check every deletion ever made on every sweep: work proportional to
    -- instances times lifetime deletions. Claiming the check by moving this forward makes it one check
    -- per id per window, whatever the instance count.
    last_checked_at BIGINT,
    -- How many consecutive checks found nothing. Each one pushes the next check further out, so a project
    -- deleted long ago is not re-checked at the same rate as one deleted a minute ago - without this, a
    -- hundred thousand historical deletions meant a hundred thousand storage listings every window,
    -- forever.
    quiet_checks BIGINT NOT NULL DEFAULT 0,
    -- When this id is next due, materialised rather than computed from `last_checked_at` and
    -- `quiet_checks` at query time. An index on an input to the eligibility expression bounds the rows
    -- *returned*, not the rows examined: with many heavily backed-off records the planner still walks a
    -- large part of a table that only ever grows. Indexing the due time itself makes discovery genuinely
    -- bounded.
    -- NOT NULL, and set when the record is created. Left null, PostgreSQL sorts it *last* (SQLite sorts
    -- nulls first), so a freshly deleted project queued behind every overdue record - hours or days on a
    -- backlog, with its late files and rows hidden throughout.
    next_check_at BIGINT NOT NULL DEFAULT 0,
    -- Which claim owns the current check. A report carries the token it was claimed with and updates
    -- nothing if it no longer matches, so a worker whose lease expired mid-batch cannot overwrite the
    -- schedule or the result of the worker that took the id after it.
    claim_token BIGINT NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_deleted_projects_due ON deleted_projects(next_check_at);

-- =============================================================================
-- 6. Files metadata (references projects)
-- =============================================================================
CREATE TABLE IF NOT EXISTS files (
    -- 64-bit, because `FileRow.id` and `FileRow.ref_count` are `i64` and SQLite's INTEGER is too. An
    -- INT4 column decodes into `i64` only where a query remembers to cast, and one that forgot failed
    -- at runtime rather than at compile time.
    id BIGSERIAL PRIMARY KEY,
    project_id TEXT NOT NULL,
    file_hash TEXT NOT NULL,
    media_type TEXT,
    size_bytes BIGINT NOT NULL,
    hash_algo TEXT NOT NULL DEFAULT 'sha256',
    ref_count BIGINT NOT NULL DEFAULT 1,
    -- Set while cleanup is deleting this file; association refuses through the fence.
    deleting_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    UNIQUE(project_id, file_hash)
);

CREATE INDEX IF NOT EXISTS idx_files_project ON files(project_id);
CREATE INDEX IF NOT EXISTS idx_files_ref_zero ON files(project_id) WHERE ref_count = 0;
CREATE INDEX IF NOT EXISTS idx_files_created ON files(project_id, created_at);

-- =============================================================================
-- 7. Trace deletion tombstones
-- =============================================================================
--
-- Closes the race the file fence alone cannot: an ingest of trace X can be in flight while
-- `delete_traces` runs, and its analytics row commits *after* the delete removed the file association
-- and reclaimed the bytes - leaving a dangling `#!B64!#` reference the delete already answered 204 for.
-- Ingest consults this table immediately before the analytics write and drops tombstoned traces, so a
-- queued redelivery collapses to a no-op instead of resurrecting a deleted trace.
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


-- =============================================================================
-- 8. Trace Files junction table
-- =============================================================================
CREATE TABLE IF NOT EXISTS trace_files (
    trace_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    file_hash TEXT NOT NULL,
    -- Whether this association is still *provisional*: created by a batch whose analytics row has not
    -- committed yet.
    --
    -- Files are written before the rows that name them, so a batch that fails has already created
    -- associations for spans that will never exist - and an association holds `ref_count` above zero, which
    -- the orphan sweeper cannot select. Releasing them on the failure path is not enough on its own: two
    -- batches can carry the same `(project, trace, hash)`, the second sees it already present and commits
    -- its span, and a release by the first then orphans the second's file. No number of reads fixes that -
    -- the read and the release are not atomic.
    --
    -- The flag makes the state durable instead. It is set when the association is created, cleared when the
    -- creating batch's write succeeds, and the failure path deletes *only* rows that are still provisional -
    -- one statement, so a batch that committed in between simply is not matched.
    provisional BOOLEAN NOT NULL DEFAULT FALSE,
    -- Project first, matching SQLite: a trace id comes from the client, so two projects can present
    -- the same one, and keyed without the project one project's association satisfied the other's
    -- conflict clause - leaving the second with no association and a reference nothing would release.
    PRIMARY KEY (project_id, trace_id, file_hash),
    FOREIGN KEY (project_id, file_hash) REFERENCES files(project_id, file_hash) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_trace_files_trace ON trace_files(trace_id);
CREATE INDEX IF NOT EXISTS idx_trace_files_project ON trace_files(project_id);
-- The derived reference count is a COUNT over (project_id, file_hash). The primary key leads with
-- project_id but separates the two by trace_id, so without this the count scans a project.
CREATE INDEX IF NOT EXISTS idx_trace_files_project_hash ON trace_files(project_id, file_hash);

-- =============================================================================
-- 8. Favorites (user-scoped, references users and projects)
-- =============================================================================
CREATE TABLE IF NOT EXISTS favorites (
    id SERIAL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    entity_type TEXT NOT NULL CHECK(entity_type IN ('trace', 'session', 'span')),
    entity_id TEXT NOT NULL,
    secondary_id TEXT,
    created_at BIGINT NOT NULL
);

-- Partial indexes for uniqueness
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
    last_used_at BIGINT,
    expires_at BIGINT,
    created_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_api_keys_org_created ON api_keys(org_id, created_at DESC);

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
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
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
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
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
"#;

/// Default data SQL for PostgreSQL (inserted separately after schema)
pub const DEFAULT_DATA: &str = r#"
-- 1. Default organization
INSERT INTO organizations (id, name, slug, created_at, updated_at)
VALUES ('default', 'Default Organization', 'default', EXTRACT(EPOCH FROM NOW())::BIGINT, EXTRACT(EPOCH FROM NOW())::BIGINT)
ON CONFLICT (id) DO NOTHING;

-- 2. Default user
INSERT INTO users (id, display_name, created_at, updated_at)
VALUES ('local', 'Local User', EXTRACT(EPOCH FROM NOW())::BIGINT, EXTRACT(EPOCH FROM NOW())::BIGINT)
ON CONFLICT (id) DO NOTHING;

-- 3. Default membership (user owns default org)
INSERT INTO organization_members (organization_id, user_id, role, created_at, updated_at)
VALUES ('default', 'local', 'owner', EXTRACT(EPOCH FROM NOW())::BIGINT, EXTRACT(EPOCH FROM NOW())::BIGINT)
ON CONFLICT (organization_id, user_id) DO NOTHING;

-- 4. Default auth method (bootstrap for local user)
INSERT INTO auth_methods (id, user_id, method_type, created_at, updated_at)
VALUES ('bootstrap-local', 'local', 'bootstrap', EXTRACT(EPOCH FROM NOW())::BIGINT, EXTRACT(EPOCH FROM NOW())::BIGINT)
ON CONFLICT (id) DO NOTHING;

-- 5. Default project (in default org)
INSERT INTO projects (id, organization_id, name, created_at, updated_at)
VALUES ('default', 'default', 'Default Project', EXTRACT(EPOCH FROM NOW())::BIGINT, EXTRACT(EPOCH FROM NOW())::BIGINT)
ON CONFLICT (id) DO NOTHING;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_schema_version_is_positive() {
        assert!(SCHEMA_VERSION > 0);
    }

    #[test]
    #[allow(clippy::const_is_empty)]
    fn test_schema_is_not_empty() {
        assert!(!SCHEMA.is_empty());
    }

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
            "favorites",
            "api_keys",
            "credentials",
            "credential_project_permissions",
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
    fn test_default_data_contains_required_inserts() {
        assert!(
            DEFAULT_DATA.contains("INSERT INTO organizations"),
            "Default data missing organization"
        );
        assert!(
            DEFAULT_DATA.contains("INSERT INTO users"),
            "Default data missing user"
        );
        assert!(
            DEFAULT_DATA.contains("INSERT INTO organization_members"),
            "Default data missing membership"
        );
        assert!(
            DEFAULT_DATA.contains("INSERT INTO auth_methods"),
            "Default data missing auth method"
        );
        assert!(
            DEFAULT_DATA.contains("INSERT INTO projects"),
            "Default data missing project"
        );
    }
}
