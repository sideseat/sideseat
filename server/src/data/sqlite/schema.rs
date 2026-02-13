//! SQLite schema definitions
//!
//! Initial schema with all tables. No migrations needed for first version.

/// Current schema version
pub const SCHEMA_VERSION: i32 = 1;

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
    updated_at INTEGER NOT NULL
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
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_projects_org ON projects(organization_id);

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
    UNIQUE(project_id, file_hash)
);

CREATE INDEX IF NOT EXISTS idx_files_project ON files(project_id);
CREATE INDEX IF NOT EXISTS idx_files_ref_zero ON files(project_id) WHERE ref_count = 0;
CREATE INDEX IF NOT EXISTS idx_files_created ON files(project_id, created_at);

-- =============================================================================
-- 7. Trace Files junction table
-- =============================================================================
CREATE TABLE IF NOT EXISTS trace_files (
    trace_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    file_hash TEXT NOT NULL,
    PRIMARY KEY (trace_id, file_hash),
    FOREIGN KEY (project_id, file_hash) REFERENCES files(project_id, file_hash) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_trace_files_trace ON trace_files(trace_id);
CREATE INDEX IF NOT EXISTS idx_trace_files_project ON trace_files(project_id);

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
