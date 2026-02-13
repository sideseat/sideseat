//! Repository traits for database backends
//!
//! This module defines traits that provide a unified interface for database operations
//! across multiple backends. Each backend (DuckDB, ClickHouse, SQLite, PostgreSQL)
//! implements these traits with its own specific logic.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::data::cache::CacheService;
use crate::data::error::DataError;
use crate::data::types::{
    ApiKeyRow, ApiKeyScope, ApiKeyValidation, AuthMethodRow, EventRow, FeedMessagesParams,
    FeedSpansParams, FileRow, LastOwnerResult, LinkRow, ListSessionsParams, ListSpansParams,
    ListTracesParams, MemberWithUser, MembershipRow, MessageQueryParams, MessageQueryResult,
    NormalizedMetric, NormalizedSpan, OrgWithRole, OrganizationRow, ProjectRow, SessionRow,
    SpanCounts, SpanRow, TraceRow, UserRow,
};

// ============================================================================
// Filter Option Types
// ============================================================================

/// Result for filter option value with count
#[derive(Debug, Clone)]
pub struct FilterOptionRow {
    pub value: String,
    pub count: u64,
}

// ============================================================================
// Analytics Repository Trait
// ============================================================================

/// Repository trait for analytics operations (traces, spans, sessions, messages, stats)
///
/// Implemented by DuckDB and ClickHouse backends.
#[async_trait]
pub trait AnalyticsRepository: Send + Sync {
    // ==================== Trace Operations ====================

    /// List traces with pagination and filters
    async fn list_traces(
        &self,
        params: &ListTracesParams,
    ) -> Result<(Vec<TraceRow>, u64), DataError>;

    /// Get a single trace by ID
    async fn get_trace(
        &self,
        project_id: &str,
        trace_id: &str,
    ) -> Result<Option<TraceRow>, DataError>;

    /// Get distinct values with counts for trace filter options
    async fn get_trace_filter_options(
        &self,
        project_id: &str,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError>;

    /// Get distinct tag values with counts from traces
    async fn get_trace_tags_options(
        &self,
        project_id: &str,
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<Vec<FilterOptionRow>, DataError>;

    /// Delete traces by IDs
    async fn delete_traces(&self, project_id: &str, trace_ids: &[String])
    -> Result<u64, DataError>;

    // ==================== Span Operations ====================

    /// List spans with pagination and filters
    async fn list_spans(&self, params: &ListSpansParams) -> Result<(Vec<SpanRow>, u64), DataError>;

    /// Get spans for a trace
    async fn get_spans_for_trace(
        &self,
        project_id: &str,
        trace_id: &str,
    ) -> Result<Vec<SpanRow>, DataError>;

    /// Get a single span by ID
    async fn get_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Option<SpanRow>, DataError>;

    /// Get span events
    async fn get_events_for_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<EventRow>, DataError>;

    /// Get span links
    async fn get_links_for_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<LinkRow>, DataError>;

    /// Get span counts (events, links) in bulk
    async fn get_span_counts_bulk(
        &self,
        project_id: &str,
        span_keys: &[(String, String)],
    ) -> Result<HashMap<(String, String), SpanCounts>, DataError>;

    /// Get feed spans (for real-time feed)
    async fn get_feed_spans(&self, params: &FeedSpansParams) -> Result<Vec<SpanRow>, DataError>;

    /// Get distinct values with counts for span filter options
    async fn get_span_filter_options(
        &self,
        project_id: &str,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
        observations_only: bool,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError>;

    /// Delete spans by IDs
    async fn delete_spans(
        &self,
        project_id: &str,
        span_keys: &[(String, String)],
    ) -> Result<u64, DataError>;

    // ==================== Session Operations ====================

    /// List sessions with pagination and filters
    async fn list_sessions(
        &self,
        params: &ListSessionsParams,
    ) -> Result<(Vec<SessionRow>, u64), DataError>;

    /// Get a single session by ID
    async fn get_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionRow>, DataError>;

    /// Get traces for a session (all traces, no pagination)
    async fn get_traces_for_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Vec<TraceRow>, DataError>;

    /// Get trace IDs for sessions (for delete)
    async fn get_trace_ids_for_sessions(
        &self,
        project_id: &str,
        session_ids: &[String],
    ) -> Result<Vec<String>, DataError>;

    /// Get distinct values with counts for session filter options
    async fn get_session_filter_options(
        &self,
        project_id: &str,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError>;

    /// Delete sessions by IDs
    async fn delete_sessions(
        &self,
        project_id: &str,
        session_ids: &[String],
    ) -> Result<u64, DataError>;

    // ==================== Message Operations ====================

    /// Get messages for a span, trace, or session (unified query).
    ///
    /// Priority: span_id > session_id > trace_id
    async fn get_messages(
        &self,
        params: &MessageQueryParams,
    ) -> Result<MessageQueryResult, DataError>;

    /// Get messages for a project (feed)
    async fn get_project_messages(
        &self,
        params: &FeedMessagesParams,
    ) -> Result<MessageQueryResult, DataError>;

    // ==================== Stats Operations ====================

    /// Get project statistics
    async fn get_project_stats(
        &self,
        params: &crate::data::types::StatsParams,
    ) -> Result<crate::data::types::ProjectStatsResult, DataError>;

    // ==================== Ingestion Operations ====================

    /// Insert spans in batch
    async fn insert_spans(&self, spans: &[NormalizedSpan]) -> Result<(), DataError>;

    /// Insert metrics in batch
    async fn insert_metrics(&self, metrics: &[NormalizedMetric]) -> Result<(), DataError>;

    // ==================== Project Data Operations ====================

    /// Delete all data for a project
    async fn delete_project_data(&self, project_id: &str) -> Result<u64, DataError>;
}

// ============================================================================
// Transactional Repository Trait
// ============================================================================

/// Repository trait for transactional operations (users, orgs, projects, etc.)
///
/// Implemented by SQLite and PostgreSQL backends.
#[async_trait]
pub trait TransactionalRepository: Send + Sync {
    // ==================== User Operations ====================

    /// Create a new user
    async fn create_user(
        &self,
        cache: Option<&CacheService>,
        email: &str,
        display_name: Option<&str>,
    ) -> Result<UserRow, DataError>;

    /// Get a user by ID
    async fn get_user(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<Option<UserRow>, DataError>;

    /// Get a user by email
    async fn get_user_by_email(
        &self,
        cache: Option<&CacheService>,
        email: &str,
    ) -> Result<Option<UserRow>, DataError>;

    /// Update a user's display name
    async fn update_user(
        &self,
        cache: Option<&CacheService>,
        id: &str,
        display_name: Option<&str>,
    ) -> Result<Option<UserRow>, DataError>;

    // ==================== Organization Operations ====================

    /// Create a new organization with owner membership atomically
    async fn create_organization_with_owner(
        &self,
        cache: Option<&CacheService>,
        name: &str,
        slug: &str,
        owner_user_id: &str,
    ) -> Result<OrganizationRow, DataError>;

    /// Get an organization by ID
    async fn get_organization(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<Option<OrganizationRow>, DataError>;

    /// Update an organization's name
    async fn update_organization(
        &self,
        cache: Option<&CacheService>,
        id: &str,
        name: &str,
    ) -> Result<Option<OrganizationRow>, DataError>;

    /// List organizations for a user with their role
    async fn list_orgs_for_user(
        &self,
        cache: Option<&CacheService>,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<OrgWithRole>, u64), DataError>;

    /// Delete an organization (cascades to projects, memberships, files)
    async fn delete_organization(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<bool, DataError>;

    /// List project IDs for an organization (for cascade cleanup)
    async fn list_project_ids(&self, organization_id: &str) -> Result<Vec<String>, DataError>;

    // ==================== Membership Operations ====================

    /// Get a membership
    async fn get_membership(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        user_id: &str,
    ) -> Result<Option<MembershipRow>, DataError>;

    /// Get a member with user info
    async fn get_member_with_user(
        &self,
        organization_id: &str,
        user_id: &str,
    ) -> Result<Option<MemberWithUser>, DataError>;

    /// Add a member to an organization
    async fn add_member(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<MembershipRow, DataError>;

    /// List members of an organization
    async fn list_members(
        &self,
        organization_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<MemberWithUser>, u64), DataError>;

    /// Update a member's role atomically with last-owner protection
    async fn update_role_atomic(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        user_id: &str,
        new_role: &str,
    ) -> Result<LastOwnerResult<MembershipRow>, DataError>;

    /// Remove a member atomically with last-owner protection
    async fn remove_member_atomic(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        user_id: &str,
    ) -> Result<LastOwnerResult<()>, DataError>;

    // ==================== Project Operations ====================

    /// Create a new project
    async fn create_project(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        name: &str,
    ) -> Result<ProjectRow, DataError>;

    /// Get a project by ID
    async fn get_project(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<Option<ProjectRow>, DataError>;

    /// Update a project's name
    async fn update_project(
        &self,
        cache: Option<&CacheService>,
        id: &str,
        name: &str,
    ) -> Result<Option<ProjectRow>, DataError>;

    /// List projects for an organization
    async fn list_projects_for_org(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError>;

    /// List projects for a user (across all orgs they're a member of)
    async fn list_projects_for_user(
        &self,
        cache: Option<&CacheService>,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError>;

    /// Delete a project (cascades to files)
    async fn delete_project(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<bool, DataError>;

    // ==================== Auth Method Operations ====================

    /// Create a new auth method
    #[allow(clippy::too_many_arguments)]
    async fn create_auth_method(
        &self,
        cache: Option<&CacheService>,
        user_id: &str,
        method_type: &str,
        provider: Option<&str>,
        provider_id: Option<&str>,
        credential_hash: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<AuthMethodRow, DataError>;

    /// Find an auth method by OAuth provider and provider ID
    async fn find_auth_by_oauth(
        &self,
        cache: Option<&CacheService>,
        provider: &str,
        provider_id: &str,
    ) -> Result<Option<AuthMethodRow>, DataError>;

    /// List all auth methods for a user
    async fn list_auth_methods_for_user(
        &self,
        cache: Option<&CacheService>,
        user_id: &str,
    ) -> Result<Vec<AuthMethodRow>, DataError>;

    /// Delete an auth method
    async fn delete_auth_method(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<bool, DataError>;

    /// Get the bootstrap auth method for a user
    async fn get_bootstrap_method(&self, user_id: &str)
    -> Result<Option<AuthMethodRow>, DataError>;

    // ==================== Favorite Operations ====================

    /// Add a favorite
    /// For spans, secondary_id is the span_id (entity_id is trace_id)
    async fn add_favorite(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_id: &str,
        secondary_id: Option<&str>,
        project_id: &str,
    ) -> Result<bool, DataError>;

    /// Remove a favorite
    /// For spans, secondary_id is the span_id (entity_id is trace_id)
    async fn remove_favorite(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_id: &str,
        secondary_id: Option<&str>,
        project_id: &str,
    ) -> Result<bool, DataError>;

    /// Check if entities are favorited
    async fn check_favorites(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &str,
    ) -> Result<Vec<String>, DataError>;

    /// Check if spans are favorited
    async fn check_span_favorites(
        &self,
        user_id: &str,
        span_ids: &[(String, String)],
        project_id: &str,
    ) -> Result<Vec<(String, String)>, DataError>;

    /// Count favorites for a user
    async fn count_favorites(&self, user_id: &str, project_id: &str) -> Result<i64, DataError>;

    /// List all favorite entity IDs for a user
    async fn list_favorite_ids(
        &self,
        user_id: &str,
        entity_type: &str,
        project_id: &str,
    ) -> Result<Vec<String>, DataError>;

    /// Delete favorites by entity (for cascade delete)
    async fn delete_favorites_by_entity(
        &self,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &str,
    ) -> Result<u64, DataError>;

    // ==================== File Operations ====================

    /// Upsert a file record (insert or increment ref_count)
    /// Returns the new ref_count value.
    async fn upsert_file(
        &self,
        project_id: &str,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
    ) -> Result<i64, DataError>;

    /// Get a file by project and hash
    async fn get_file(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<Option<FileRow>, DataError>;

    /// Check if a file exists
    async fn file_exists(&self, project_id: &str, file_hash: &str) -> Result<bool, DataError>;

    /// Decrement ref_count atomically and return the new value
    /// Returns None if file doesn't exist, Some(new_ref_count) otherwise.
    async fn decrement_ref_count(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError>;

    /// Delete a file metadata record
    async fn delete_file(&self, project_id: &str, file_hash: &str) -> Result<bool, DataError>;

    /// Delete all file records for a project
    async fn delete_project_files(&self, project_id: &str) -> Result<u64, DataError>;

    /// Insert a trace-file association
    async fn insert_trace_file(
        &self,
        trace_id: &str,
        project_id: &str,
        file_hash: &str,
    ) -> Result<(), DataError>;

    /// Get file hashes for traces
    async fn get_file_hashes_for_traces(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError>;

    /// Delete trace-file associations for traces
    async fn delete_trace_files(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<u64, DataError>;

    /// Get total storage used by a project
    async fn get_project_storage_bytes(&self, project_id: &str) -> Result<i64, DataError>;

    /// Get all files with zero ref_count (for cleanup)
    async fn get_orphan_files(&self) -> Result<Vec<(String, String)>, DataError>;

    // ==================== API Key Operations ====================

    /// Create API key. Returns Err(Conflict) if limit (100) exceeded.
    #[allow(clippy::too_many_arguments)]
    async fn create_api_key(
        &self,
        cache: Option<&CacheService>,
        org_id: &str,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
        scope: ApiKeyScope,
        created_by: &str,
        expires_at: Option<i64>,
    ) -> Result<ApiKeyRow, DataError>;

    /// Get validation info by hash. Used for OTEL and API auth.
    async fn get_api_key_by_hash(
        &self,
        cache: Option<&CacheService>,
        key_hash: &str,
    ) -> Result<Option<ApiKeyValidation>, DataError>;

    /// List all keys for organization (metadata only, ordered by created_at DESC).
    async fn list_api_keys(
        &self,
        cache: Option<&CacheService>,
        org_id: &str,
    ) -> Result<Vec<ApiKeyRow>, DataError>;

    /// Delete key by ID.
    async fn delete_api_key(
        &self,
        cache: Option<&CacheService>,
        id: &str,
        org_id: &str,
    ) -> Result<bool, DataError>;

    /// Update last_used_at (debounced, only if older than threshold).
    async fn touch_api_key(&self, id: &str, threshold_secs: u64) -> Result<bool, DataError>;

    /// Delete all keys for organization (for org deletion cleanup).
    async fn delete_api_keys_for_org(
        &self,
        cache: Option<&CacheService>,
        org_id: &str,
    ) -> Result<u64, DataError>;

    /// Get key hashes for organization (for cache invalidation on org delete).
    async fn get_api_key_hashes_for_org(&self, org_id: &str) -> Result<Vec<String>, DataError>;
}

// ============================================================================
// Helper function (not part of trait, but shared utility)
// ============================================================================

/// Check if user has minimum role level (pure function, same for all backends)
pub fn has_min_role_level(role: &str, min_role: &str) -> bool {
    // Role hierarchy: owner > admin > member
    let role_level = match role {
        "owner" => 3,
        "admin" => 2,
        "member" => 1,
        _ => 0,
    };
    let min_level = match min_role {
        "owner" => 3,
        "admin" => 2,
        "member" => 1,
        _ => 0,
    };
    role_level >= min_level
}
