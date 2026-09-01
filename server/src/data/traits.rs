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
    ApiKeyRow, ApiKeyScope, ApiKeyValidation, AuthMethodRow, CredentialPermissionRow,
    CredentialRow, EventRow, FeedMessagesParams, FeedSpansParams, FileRow, LastOwnerResult,
    LinkRow, ListSessionsParams, ListSpansParams, ListTracesParams, MemberWithUser, MembershipRow,
    MessageQueryParams, MessageQueryResult, NormalizedMetric, NormalizedSpan, OrgWithRole,
    OrganizationRow, ProjectRow, SessionRow, SpanCounts, SpanRow, TraceRow, UserRow,
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

    /// Delete traces by IDs.
    ///
    /// The returned count means different things per backend and no caller reads it: DuckDB
    /// reports rows removed, while ClickHouse deletes through an asynchronous mutation and can
    /// only report how many ids it was asked about. Making them agree would mean waiting for the
    /// mutation to settle just to produce a number the routes discard - they answer 204. What
    /// both backends do guarantee, and what the parity test checks, is which rows are gone.
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

    /// Delete spans by IDs.
    ///
    /// The returned count is backend-specific and unread; see [`AnalyticsRepository::delete_traces`].
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

    /// Delete sessions by IDs.
    ///
    /// The returned count is backend-specific and unread; see [`AnalyticsRepository::delete_traces`].
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

    /// Insert spans in batch (takes ownership to avoid clone for spawn_blocking)
    async fn insert_spans(&self, spans: Vec<NormalizedSpan>) -> Result<(), DataError>;

    /// Insert metrics in batch
    async fn insert_metrics(&self, metrics: &[NormalizedMetric]) -> Result<(), DataError>;

    // ==================== Project Data Operations ====================

    /// Delete all data for a project
    async fn delete_project_data(&self, project_id: &str) -> Result<u64, DataError>;

    /// Count the rows a project still owns, over every table this backend holds for it.
    ///
    /// Deletion verification asks this rather than counting spans, because "the data is gone" has to mean
    /// all of it: metrics live in their own table, ClickHouse applies its deletes as asynchronous
    /// mutations, and a project whose metrics outlived it is as unreachable as one whose spans did.
    async fn count_project_rows(&self, project_id: &str) -> Result<u64, DataError>;

    /// Count spans grouped by project for a set of project IDs.
    /// Used for org/user-level span count aggregation.
    async fn count_spans_by_project(
        &self,
        project_ids: &[String],
    ) -> Result<HashMap<String, u64>, DataError>;
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

    /// Claim a project for deletion, if it exists and nobody else has claimed it.
    ///
    /// Takes the cache because a successful claim must drop every cached answer about the project at
    /// once: from that moment it is not live, and a cached "here it is" would outlive the fact by the
    /// cache's five minutes.
    async fn claim_project_for_deletion(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<bool, DataError>;

    /// Whether this project accepts writes: a row exists and nothing has claimed it.
    ///
    /// A missing project is refused as firmly as a claimed one. Spans written for a project with no row
    /// are unreachable through every read path, so accepting them stores data nothing can ever show.
    async fn project_accepts_writes(&self, id: &str) -> Result<bool, DataError>;

    /// Record that these traces were deleted, so a late ingest cannot resurrect them.
    ///
    /// The trace deletion route removes the analytics rows and then reclaims the file bytes those rows
    /// referenced. An ingest already in flight for one of those traces commits *afterwards* - the file
    /// association and bytes were written before the analytics row, which is deliberate - and the
    /// result is a span row carrying a `#!B64!#` reference to content the deletion has taken, for a
    /// trace the caller was told 204 for. No elapsed time bounds that: a queued batch can be
    /// redelivered minutes later.
    ///
    /// So the deletion leaves a tombstone and the write path consults it. Written in the same request
    /// as the deletion, before the analytics delete, so no window exists where a trace is deleted and
    /// not yet tombstoned.
    async fn record_deleted_traces(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<(), DataError>;

    /// Which of these traces are tombstoned, so their spans must not be written.
    ///
    /// Returns the subset that has been deleted. One query per batch rather than per span: a batch
    /// commonly carries a handful of traces, and this sits directly on the ingestion hot path.
    async fn deleted_traces_among(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError>;

    /// Projects claimed for deletion longer ago than `older_than_secs`, so a cleanup that died part
    /// way through can be resumed.
    async fn get_stale_claimed_projects(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<String>, DataError>;

    /// Record what a cleanup sweep observed for a project, and answer whether its tombstone may go.
    ///
    /// The barrier is repeated observation, not elapsed time: no wall-clock grace period bounds how long
    /// a writer that read the fence before the tombstone can take to commit.
    async fn record_project_sweep(
        &self,
        id: &str,
        was_clean: bool,
        required: i64,
        min_gap_secs: i64,
    ) -> Result<bool, DataError>;

    /// Claim deleted-project ids for a cleanup check, one per window.
    ///
    /// Returns the ids this call claimed - not every one recorded. Cleanup runs on every instance, and the
    /// records are permanent; without a window the cost would grow with instances times lifetime
    /// deletions. Marking the check in the same statement that returns it makes concurrent instances race
    /// for each id and only one wins per window.
    /// Returns `(project id, claim token)`. The token is what a report must present: a worker whose lease
    /// expired part way through its batch would otherwise overwrite the schedule and result of the worker
    /// that has since taken the id.
    async fn claim_deleted_projects_for_check(
        &self,
        lease_secs: i64,
        limit: i64,
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Record that these sessions were deleted, so a trace arriving later cannot recreate them.
    ///
    /// The trace tombstone is not enough on its own: a session is deleted by resolving it to trace ids and
    /// deleting those, and a trace of the same session that arrives *after* that resolution was never in
    /// the snapshot. The session id is the durable fact - the trace ids are one instant's view of it.
    async fn record_deleted_sessions(
        &self,
        project_id: &str,
        session_ids: &[String],
    ) -> Result<(), DataError>;

    /// Which of these sessions are tombstoned, so their spans must not be written.
    async fn deleted_sessions_among(
        &self,
        project_id: &str,
        session_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError>;

    /// Claim a batch of deleted *sessions* whose check is due, leased and exclusive.
    ///
    /// Same protocol as traces, for the same reason: the pre-write session check and the analytics write are
    /// in different stores, so a crash between the write and its compensating re-check leaves spans for a
    /// deleted session that only a sweep can collect.
    async fn claim_deleted_sessions_for_check(
        &self,
        lease_secs: i64,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError>;

    /// Record what a deleted session's check found, matched on the claim token.
    async fn record_deleted_session_check(
        &self,
        project_id: &str,
        session_id: &str,
        claim_token: i64,
        was_quiet: bool,
        base_gap_secs: i64,
        max_gap_secs: i64,
    ) -> Result<(), DataError>;

    /// Claim a batch of deleted *traces* whose check is due, leased and exclusive.
    ///
    /// Same discipline as the project claim, and needed for the same reason: the pre-write tombstone check
    /// and the analytics write are in different stores, so a crash between them leaves spans for a deleted
    /// trace that only a sweep can collect. Re-checking every record forever at a fixed rate would be
    /// unbounded lifetime work, hence the lease, the backoff and the cap.
    async fn claim_deleted_traces_for_check(
        &self,
        lease_secs: i64,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError>;

    /// Record what a deleted trace's check found, matched on the claim token.
    async fn record_deleted_trace_check(
        &self,
        project_id: &str,
        trace_id: &str,
        claim_token: i64,
        was_quiet: bool,
        base_gap_secs: i64,
        max_gap_secs: i64,
    ) -> Result<(), DataError>;

    /// Record what a deleted project's check found. Quiet pushes the next check further out; anything found
    /// brings it back to the base interval.
    async fn record_deleted_project_check(
        &self,
        project_id: &str,
        claim_token: i64,
        was_quiet: bool,
        base_gap_secs: i64,
        max_gap_secs: i64,
    ) -> Result<(), DataError>;

    /// Forget projects deleted longer ago than `retention_secs`. Returns how many were forgotten.
    async fn forget_deleted_projects(&self, retention_secs: i64) -> Result<u64, DataError>;

    /// Claim an organization for deletion, if it exists and nobody else has claimed it.
    async fn claim_organization_for_deletion(&self, id: &str) -> Result<bool, DataError>;

    /// Organizations claimed for deletion longer ago than `older_than_secs`.
    async fn get_stale_claimed_organizations(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<String>, DataError>;

    /// How many project rows an organization still has, tombstoned or not.
    async fn count_projects_of_organization(&self, org_id: &str) -> Result<i64, DataError>;

    /// Delete a project's row. Only correct once its data is gone: the row is what every other path
    /// finds the data by.
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
        hash_algo: &str,
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

    /// Associate a file with a trace, counting the reference only if the association is new.
    ///
    /// One operation because `ref_count` must equal the number of associations - that is what
    /// deletion decrements, and doing the two separately drifted in both directions.
    async fn associate_file(
        &self,
        trace_id: &str,
        project_id: &str,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<bool, DataError>;

    /// How many of these traces reference each file, so deletion can decrement by that many.
    async fn get_file_reference_counts_for_traces(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Associate a trace with a file that already exists, without inventing metadata for it.
    ///
    /// Returns false when there is no such file, or it is claimed for deletion - both mean the caller
    /// must not commit a reference to it.
    async fn associate_existing_file(
        &self,
        trace_id: &str,
        project_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Files claimed for deletion longer ago than `older_than_secs`, so an abandoned claim can be resumed.
    async fn get_stale_claimed_files(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, String)>, DataError>;

    /// Claim a file for deletion, if nothing references it and nobody else has claimed it.
    ///
    /// The fence that closes the delete-then-recreate window: association refuses while a claim is set,
    /// because a claimed file's bytes may already be gone.
    async fn claim_file_for_deletion(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Give up a deletion claim, leaving the file in place.
    async fn release_deletion_claim(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<(), DataError>;

    /// Put back a metadata row whose bytes could not be deleted, as an orphan for a later sweep.
    async fn restore_orphan_metadata(
        &self,
        project_id: &str,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<(), DataError>;

    /// Delete a file's metadata only if nothing references it, and say whether it was deleted.
    ///
    /// The condition belongs inside the statement: reading a count of zero and then deleting races with
    /// a concurrent association, and loses.
    async fn delete_file_if_unreferenced(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Recompute a file's reference count from the associations that exist, and return it.
    ///
    /// The count is a cached `COUNT(*)`; deriving it is the only form immune to two concurrent
    /// cleanups both subtracting the same references.
    async fn sync_ref_count(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError>;

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

    /// Release one association this process created, after the write that would have justified it failed.
    ///
    /// A compensating action, not a deletion: the bytes are written before the analytics row that
    /// references them, so a batch whose analytics write fails has already created associations for spans
    /// that will never land - and those keep `ref_count` above zero, which is exactly what the orphan
    /// sweeper selects on, so nothing would ever reclaim them.
    ///
    /// Scoped to a single `(project, trace, hash)` because only the association this batch *created* may
    /// go. One that already existed belongs to an earlier committed batch, and releasing it would orphan
    /// that batch's file. Returns whether a row was removed.
    /// Confirm a batch's associations now that its analytics rows are committed.
    ///
    /// An association is created *provisional*, because files are written before the rows that name them.
    /// Confirming clears that marker, and the failure path deletes only rows still marked - which is what
    /// makes the release safe under concurrency rather than merely precise: two batches can carry the same
    /// association, and a read-then-release pair cannot tell "mine, unused" from "also the other batch's,
    /// now committed".
    async fn confirm_trace_file_associations(
        &self,
        associations: &[(String, String, String)],
    ) -> Result<u64, DataError>;

    async fn release_trace_file_association(
        &self,
        project_id: &str,
        trace_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Delete trace-file associations for traces, returning the file hashes affected.
    ///
    /// The hashes come from the delete itself rather than from a prior read: an association added between a
    /// read and the delete is removed here but absent from the read, so its file's stored reference count is
    /// never recomputed - and the orphan sweeper selects on that count, so nothing would ever reclaim it.
    async fn delete_trace_files(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError>;

    /// Get total storage used by a project
    async fn get_project_storage_bytes(&self, project_id: &str) -> Result<i64, DataError>;

    /// Get all files with zero ref_count (for cleanup)
    async fn get_orphan_files(&self) -> Result<Vec<(String, String)>, DataError>;

    /// Get total file storage used by all projects in an organization
    async fn get_org_file_storage_bytes(&self, org_id: &str) -> Result<i64, DataError>;

    /// Get total file storage used across all orgs a user belongs to
    async fn get_user_file_storage_bytes(&self, user_id: &str) -> Result<i64, DataError>;

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

    // ==================== Credential Operations ====================

    /// List all credentials for an organization (metadata only, no secrets)
    async fn list_credentials(&self, org_id: &str) -> Result<Vec<CredentialRow>, DataError>;

    /// Get a single credential by id, scoped to org
    async fn get_credential(
        &self,
        id: &str,
        org_id: &str,
    ) -> Result<Option<CredentialRow>, DataError>;

    /// Create a new credential row (secret stored separately)
    #[allow(clippy::too_many_arguments)]
    async fn create_credential(
        &self,
        id: &str,
        org_id: &str,
        provider_key: &str,
        display_name: &str,
        endpoint_url: Option<&str>,
        extra_config: Option<&str>,
        key_preview: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<CredentialRow, DataError>;

    /// Update credential metadata (display_name, endpoint_url, extra_config).
    /// Uses `Option<Option<&str>>` to distinguish absent (don't change) from
    /// `Some(None)` (clear field) and `Some(Some(v))` (set to value).
    async fn update_credential(
        &self,
        id: &str,
        org_id: &str,
        display_name: Option<&str>,
        endpoint_url: Option<Option<&str>>,
        extra_config: Option<Option<&str>>,
    ) -> Result<Option<CredentialRow>, DataError>;

    /// Delete a credential row by id, scoped to org. Returns true if deleted.
    async fn delete_credential(&self, id: &str, org_id: &str) -> Result<bool, DataError>;

    // ==================== Credential Permission Operations ====================

    /// List permissions for a credential
    async fn list_credential_permissions(
        &self,
        credential_id: &str,
    ) -> Result<Vec<CredentialPermissionRow>, DataError>;

    /// Create a credential project permission
    async fn create_credential_permission(
        &self,
        id: &str,
        credential_id: &str,
        org_id: &str,
        project_id: Option<&str>,
        access: &str,
        created_by: Option<&str>,
    ) -> Result<CredentialPermissionRow, DataError>;

    /// Delete a credential permission by id. Returns true if deleted.
    async fn delete_credential_permission(
        &self,
        id: &str,
        credential_id: &str,
    ) -> Result<bool, DataError>;

    /// Get credential IDs accessible by a specific project (for filtering).
    /// Returns credentials that are not denied for this project and either:
    /// - have no allow rules (accessible by default), or
    /// - have an allow rule for this project or the org-level default
    async fn get_credentials_accessible_by_project(
        &self,
        org_id: &str,
        project_id: &str,
    ) -> Result<Vec<String>, DataError>;
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
