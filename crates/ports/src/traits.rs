//! Repository traits for database backends
//!
//! This module defines traits that provide a unified interface for database operations
//! across multiple backends. Each backend (DuckDB, ClickHouse, SQLite, PostgreSQL)
//! implements these traits with its own specific logic.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};

use crate::error::DataError;
use crate::types::{
    ApiKeyRow, ApiKeyScope, ApiKeyValidation, AuthMethodRow, ContentBodyBackfillProgress,
    ContentBodyObject, CredentialPermissionRow, CredentialRow, EventRow, FeedMessagesParams,
    FeedSpansParams, FileRow, LastOwnerResult, LinkRow, ListLogsParams, ListMetricsParams,
    ListSessionsParams, ListSpansParams, ListTracesParams, LogRow, MemberWithUser, MembershipRow,
    MessageQueryParams, MessageQueryResult, MetricAggregateRow, MetricRow, NormalizedLog,
    NormalizedMetric, NormalizedSpan, OrgWithRole, OrganizationRow, PressureSpanCandidate,
    ProjectHold, ProjectId, ProjectRow, ProjectStorageUsage, SearchBackfillDocument,
    SearchBackfillSource, SearchCursor, SearchPage, SearchQuery, SearchSignal, SessionRow,
    SpanBodyAssociation, SpanBodyField, SpanBodySource, SpanCounts, SpanRow, StagedPayload,
    TraceRow, UserRow,
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

/// The one thing survivor reconciliation needs from the analytics store.
///
/// A port scoped to its consumer, rather than passing the whole 31-method `AnalyticsRepository` around. Two
/// reasons, and the second is what forced it: the file layer genuinely needs one method, and a test cannot
/// otherwise substitute a stub - implementing thirty-one unrelated methods to drive one interleaving is how a
/// race ends up untested. That is the god-trait problem this codebase already has a plan to split; this is the
/// first cut of it, made where a test demanded it.
///
/// One of the narrow ports the two aggregates bundle, and the first that existed - written when a test needed
/// to substitute a stub and could not implement thirty-one unrelated methods to do it.
#[async_trait]
pub trait SurvivorReferences: Send + Sync {
    /// The text of every field that can hold a `#!B64!#` reference, for the surviving winning spans of these
    /// traces.
    async fn file_reference_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError>;

    /// Inline body fields of the current winning spans for exact body-ownership reconciliation.
    async fn span_body_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<SpanBodySource>, DataError> {
        let _ = (project_id, trace_ids);
        Err(DataError::NotImplemented(
            "span body survivor fields".to_string(),
        ))
    }

    /// One stable identity-ordered page for the resumable body backfill.
    async fn span_body_backfill_page(
        &self,
        project_id: &ProjectId,
        after: Option<(String, String)>,
        limit: usize,
    ) -> Result<Vec<SpanBodySource>, DataError> {
        let _ = (project_id, after, limit);
        Err(DataError::NotImplemented(
            "span body backfill page".to_string(),
        ))
    }
}

// ============================================================================
// Transactional Repository Trait
// ============================================================================

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

/// Writing spans and reading them back as rows.
#[async_trait]
pub trait SpanStore: Send + Sync {
    /// List spans with pagination and filters
    async fn list_spans(&self, params: &ListSpansParams) -> Result<(Vec<SpanRow>, u64), DataError>;

    /// Get spans for a trace
    async fn get_spans_for_trace(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
    ) -> Result<Vec<SpanRow>, DataError>;

    /// Get a single span by ID
    async fn get_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Option<SpanRow>, DataError>;

    /// Get span events
    async fn get_events_for_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<EventRow>, DataError>;

    /// Get span links
    async fn get_links_for_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<LinkRow>, DataError>;

    /// Get span counts (events, links) in bulk
    async fn get_span_counts_bulk(
        &self,
        project_id: &ProjectId,
        span_keys: &[(String, String)],
    ) -> Result<HashMap<(String, String), SpanCounts>, DataError>;

    /// Get feed spans (for real-time feed)
    async fn get_feed_spans(&self, params: &FeedSpansParams) -> Result<Vec<SpanRow>, DataError>;

    /// Get distinct values with counts for span filter options
    async fn get_span_filter_options(
        &self,
        project_id: &ProjectId,
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
        project_id: &ProjectId,
        span_keys: &[(String, String)],
    ) -> Result<u64, DataError>;
    /// Insert spans in batch (takes ownership to avoid clone for spawn_blocking)
    async fn insert_spans(&self, spans: Vec<NormalizedSpan>) -> Result<(), DataError>;

    async fn spans_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String, String)],
    ) -> Result<bool, DataError>;
}

/// Metric writes and the read API over winning datapoint revisions.
#[async_trait]
pub trait MetricStore: Send + Sync {
    async fn insert_metrics(&self, metrics: &[NormalizedMetric]) -> Result<(), DataError>;

    async fn list_metrics(
        &self,
        params: &ListMetricsParams,
    ) -> Result<(Vec<MetricRow>, u64), DataError>;

    async fn get_metric(
        &self,
        project_id: &ProjectId,
        datapoint_id: &str,
    ) -> Result<Option<MetricRow>, DataError>;

    async fn aggregate_metrics(
        &self,
        params: &ListMetricsParams,
    ) -> Result<Vec<MetricAggregateRow>, DataError>;

    async fn get_metric_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError>;

    async fn metrics_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String)],
    ) -> Result<bool, DataError>;
}

/// OTLP log writes and correlation-aware reads.
#[async_trait]
pub trait LogStore: Send + Sync {
    async fn insert_logs(&self, logs: &[NormalizedLog]) -> Result<(), DataError>;

    async fn list_logs(&self, params: &ListLogsParams) -> Result<(Vec<LogRow>, u64), DataError>;

    async fn get_log(
        &self,
        project_id: &ProjectId,
        log_digest: &str,
        ordinal: u32,
    ) -> Result<Option<LogRow>, DataError>;

    async fn get_log_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError>;

    async fn logs_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, u32)],
    ) -> Result<bool, DataError>;
}

/// Chronological, non-ranking search over spans and logs.
#[async_trait]
pub trait SearchIndex: Send + Sync {
    async fn search(&self, query: &SearchQuery) -> Result<SearchPage, DataError>;

    /// Best-effort detector for writes landing in the traversal region already consumed.
    async fn search_arrivals_detected(
        &self,
        query: &SearchQuery,
        through: &SearchCursor,
    ) -> Result<bool, DataError>;

    /// A bounded page of current records without a complete index marker.
    ///
    /// The marker is the durable checkpoint: successful rows disappear from the next page, while a
    /// partial failure naturally resumes at the first unfinished row.
    async fn search_backfill_page(
        &self,
        project_id: &ProjectId,
        signal: SearchSignal,
        limit: usize,
    ) -> Result<Vec<SearchBackfillSource>, DataError> {
        let _ = (project_id, signal, limit);
        Err(DataError::NotImplemented(
            "search backfill source page".to_string(),
        ))
    }

    /// Persist domain-produced term documents and their complete markers.
    async fn write_search_backfill(
        &self,
        project_id: &ProjectId,
        signal: SearchSignal,
        documents: &[SearchBackfillDocument],
    ) -> Result<(), DataError> {
        let _ = (project_id, signal, documents);
        Err(DataError::NotImplemented(
            "search backfill document write".to_string(),
        ))
    }
}

/// Traces, sessions and project statistics: the aggregate views a list page shows.
#[async_trait]
pub trait EntityQuery: Send + Sync {
    /// List traces with pagination and filters
    async fn list_traces(
        &self,
        params: &ListTracesParams,
    ) -> Result<(Vec<TraceRow>, u64), DataError>;

    /// Get a single trace by ID
    async fn get_trace(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
    ) -> Result<Option<TraceRow>, DataError>;

    /// Get distinct values with counts for trace filter options
    async fn get_trace_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError>;

    /// Get distinct tag values with counts from traces
    async fn get_trace_tags_options(
        &self,
        project_id: &ProjectId,
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
    async fn delete_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<u64, DataError>;

    /// Which of these traces have **no winning spans left**.
    ///
    /// Retention expires span identities, not traces, so a trace it touched is usually still there. Anything
    /// keyed on the trace as a whole - its favourite, for one - may only be removed for a trace that is
    /// actually gone, and "was in the retention batch" is not that.
    async fn traces_without_spans(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError>;

    /// List sessions with pagination and filters
    async fn list_sessions(
        &self,
        params: &ListSessionsParams,
    ) -> Result<(Vec<SessionRow>, u64), DataError>;

    /// Get a single session by ID
    async fn get_session(
        &self,
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Option<SessionRow>, DataError>;

    /// Get traces for a session (all traces, no pagination)
    async fn get_traces_for_session(
        &self,
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Vec<TraceRow>, DataError>;

    /// Get trace IDs for sessions (for delete)
    async fn get_trace_ids_for_sessions(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
        as_of_us: Option<i64>,
    ) -> Result<Vec<String>, DataError>;

    /// The sessions the given traces belong to.
    ///
    /// `as_of_us` bounds the answer to a traversal's watermark, so membership is resolved at the same
    /// instant the rows are. **Honoured by DuckDB only**: ClickHouse's `FINAL` has no "as of" form and a
    /// merge may already have discarded the earlier version, exactly as documented for the page and context
    /// queries. `None` means "current", which is what every non-paging caller wants.
    ///
    /// The mirror of [`Self::get_trace_ids_for_sessions`], and needed for the same reason the feed needs to
    /// widen its context: a framework records the session id on the span that knows it, usually the root
    /// alone, so a *set of spans* is not evidence of which sessions they belong to. Asking by trace is,
    /// because every span of a trace shares the trace's session.
    async fn get_session_ids_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        as_of_us: Option<i64>,
    ) -> Result<Vec<String>, DataError>;

    /// Which session each of the given traces belongs to.
    ///
    /// [`Self::get_session_ids_for_traces`] answers *which sessions are involved*; this answers *which
    /// trace is in which*, which is what the feed needs to group traces into conversations so a replay
    /// crossing traces can be recognised.
    ///
    /// It has to come from the store rather than from the rows the feed is handed: those rows have been
    /// through `MESSAGE_CONTENT_FILTER`, and a framework records the session on the span that knows it -
    /// usually a root that often carries no content and is therefore removed. Deriving the grouping from
    /// the filtered rows made each trace its own conversation, so the cross-trace stripping never ran and
    /// re-sent history came back as duplicates while the response still claimed `session_scoped`.
    ///
    /// Traces with no session are simply absent from the result.
    async fn get_trace_session_pairs(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        as_of_us: Option<i64>,
    ) -> Result<Vec<(String, String)>, DataError>;

    /// Get distinct values with counts for session filter options
    async fn get_session_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError>;

    /// Delete sessions by IDs, returning **the trace ids it removed**.
    ///
    /// Not a row count: a session is deleted by resolving it to traces and deleting those, and the caller
    /// has to tombstone and reclaim files for exactly what went. It cannot resolve that set itself - the
    /// resolution it made before the call is one instant's view, and a trace that joined the session since
    /// is deleted here too. Tombstoning only the earlier snapshot left such a trace with its rows gone, its
    /// file associations held forever (the orphan sweeper selects on zero references), and no tombstone -
    /// so the trace sweep could not find it and the session sweep could not either, because it resolves
    /// sessions through analytics rows that no longer exist.
    async fn delete_sessions(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
    ) -> Result<Vec<String>, DataError>;
    /// Get project statistics
    async fn get_project_stats(
        &self,
        params: &crate::types::StatsParams,
    ) -> Result<crate::types::ProjectStatsResult, DataError>;
}

/// The rows a message or feed view reconstructs from.
#[async_trait]
pub trait MessageStore: Send + Sync {
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
}

/// Deletes, counts and the watermark - what a sweep needs and a read path does not.
#[async_trait]
pub trait AnalyticsMaintenance: Send + Sync {
    /// Delete all data for a project
    async fn delete_project_data(&self, project_id: &ProjectId) -> Result<u64, DataError>;

    /// Count the rows a project still owns, over every table this backend holds for it.
    ///
    /// Deletion verification asks this rather than counting spans, because "the data is gone" has to mean
    /// all of it: metrics live in their own table, ClickHouse applies its deletes as asynchronous
    /// mutations, and a project whose metrics outlived it is as unreachable as one whose spans did.
    async fn count_project_rows(&self, project_id: &ProjectId) -> Result<u64, DataError>;

    /// The newest ingestion time the store has actually committed for a project, in microseconds.
    ///
    /// The feed's traversal watermark. It used to be `Utc::now()`, which is a statement about *this
    /// process's clock* rather than about the store: a clock ahead of the store's excluded rows that were
    /// already committed, and one behind it admitted rows the next page would read again. Asking the store
    /// removes that entirely - the value is one it has, by definition.
    ///
    /// The residual, stated because it is not zero: a write whose `ingested_at` was stamped before this read
    /// but which commits after it is below the watermark and appears on a later page. That window is the
    /// duration of one write rather than an arbitrary clock difference, and closing it fully needs a
    /// commit-ordered sequence that neither analytics backend provides.
    ///
    /// `None` when the project has no rows, in which case a traversal has nothing to bound.
    async fn max_ingested_at_us(&self, project_id: &ProjectId) -> Result<Option<i64>, DataError>;

    /// Count spans grouped by project for a set of project IDs.
    /// Used for org/user-level span count aggregation.
    async fn count_spans_by_project(
        &self,
        project_ids: &[ProjectId],
    ) -> Result<HashMap<String, u64>, DataError>;

    /// Stamp every existing signal row for a project with the durable hold deadline.
    async fn patch_project_hold(
        &self,
        project_id: &ProjectId,
        hold_until: DateTime<Utc>,
    ) -> Result<(), DataError> {
        let _ = (project_id, hold_until);
        Err(DataError::NotImplemented(
            "analytics legal-hold patch".to_string(),
        ))
    }

    /// Logical bytes currently attributable to a project across analytics signals.
    async fn project_logical_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        let _ = project_id;
        Err(DataError::NotImplemented(
            "analytics logical-byte accounting".to_string(),
        ))
    }

    /// Logical bytes protected by an active hold at `now`.
    async fn project_held_logical_bytes(
        &self,
        project_id: &ProjectId,
        now: DateTime<Utc>,
    ) -> Result<u64, DataError> {
        let _ = (project_id, now);
        Err(DataError::NotImplemented(
            "analytics held-byte accounting".to_string(),
        ))
    }

    /// Select a bounded oldest-first batch of winning, non-held spans whose bytes cross `target_bytes`.
    async fn oldest_reclaimable_spans(
        &self,
        project_id: &ProjectId,
        target_bytes: u64,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<PressureSpanCandidate>, DataError> {
        let _ = (project_id, target_bytes, now, limit);
        Err(DataError::NotImplemented(
            "pressure-reclamation candidate selection".to_string(),
        ))
    }
}

/// Users, organizations, memberships and auth methods: who is asking.
#[async_trait]
pub trait IdentityStore: Send + Sync {
    /// Create a new user
    async fn create_user(
        &self,
        email: &str,
        display_name: Option<&str>,
    ) -> Result<UserRow, DataError>;

    /// Get a user by ID
    async fn get_user(&self, id: &str) -> Result<Option<UserRow>, DataError>;

    /// Get a user by email
    async fn get_user_by_email(&self, email: &str) -> Result<Option<UserRow>, DataError>;

    /// Update a user's display name
    async fn update_user(
        &self,
        id: &str,
        display_name: Option<&str>,
    ) -> Result<Option<UserRow>, DataError>;
    /// Create a new organization with owner membership atomically
    async fn create_organization_with_owner(
        &self,
        name: &str,
        slug: &str,
        owner_user_id: &str,
    ) -> Result<OrganizationRow, DataError>;

    /// Get an organization by ID
    async fn get_organization(&self, id: &str) -> Result<Option<OrganizationRow>, DataError>;

    /// Update an organization's name
    async fn update_organization(
        &self,
        id: &str,
        name: &str,
    ) -> Result<Option<OrganizationRow>, DataError>;

    /// List organizations for a user with their role
    async fn list_orgs_for_user(
        &self,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<OrgWithRole>, u64), DataError>;

    /// Delete an organization (cascades to projects, memberships, files)
    async fn delete_organization(&self, id: &str) -> Result<bool, DataError>;

    /// List project IDs for an organization (for cascade cleanup)
    async fn list_project_ids(&self, organization_id: &str) -> Result<Vec<String>, DataError>;
    /// Get a membership
    async fn get_membership(
        &self,
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
        organization_id: &str,
        user_id: &str,
        new_role: &str,
    ) -> Result<LastOwnerResult<MembershipRow>, DataError>;

    /// Remove a member atomically with last-owner protection
    async fn remove_member_atomic(
        &self,
        organization_id: &str,
        user_id: &str,
    ) -> Result<LastOwnerResult<()>, DataError>;
    /// Create a new auth method
    #[allow(clippy::too_many_arguments)]
    async fn create_auth_method(
        &self,
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
        provider: &str,
        provider_id: &str,
    ) -> Result<Option<AuthMethodRow>, DataError>;

    /// List all auth methods for a user
    async fn list_auth_methods_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<AuthMethodRow>, DataError>;

    /// Delete an auth method
    async fn delete_auth_method(&self, id: &str) -> Result<bool, DataError>;

    /// Get the bootstrap auth method for a user
    async fn get_bootstrap_method(&self, user_id: &str)
    -> Result<Option<AuthMethodRow>, DataError>;
}

/// Projects and their deletion fence.
#[async_trait]
pub trait ProjectStore: Send + Sync {
    /// Create a new project
    async fn create_project(
        &self,
        organization_id: &str,
        name: &str,
    ) -> Result<ProjectRow, DataError>;

    /// Get a project by ID
    async fn get_project(&self, id: &str) -> Result<Option<ProjectRow>, DataError>;

    /// Update a project's name
    async fn update_project(&self, id: &str, name: &str) -> Result<Option<ProjectRow>, DataError>;

    /// List projects for an organization
    async fn list_projects_for_org(
        &self,
        organization_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError>;

    /// List projects for a user (across all orgs they're a member of)
    async fn list_projects_for_user(
        &self,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError>;

    /// List all live projects for bounded background maintenance.
    async fn list_projects(
        &self,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError>;

    /// Claim a project for deletion, if it exists and nobody else has claimed it.
    ///
    /// Takes the cache because a successful claim must drop every cached answer about the project at
    /// once: from that moment it is not live, and a cached "here it is" would outlive the fact by the
    /// cache's five minutes.
    async fn claim_project_for_deletion(&self, id: &str) -> Result<bool, DataError>;

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
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<(), DataError>;

    /// Which of these traces are tombstoned, so their spans must not be written.
    ///
    /// Returns the subset that has been deleted. One query per batch rather than per span: a batch
    /// commonly carries a handful of traces, and this sits directly on the ingestion hot path.
    async fn deleted_traces_among(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError>;

    /// Projects claimed for deletion longer ago than `older_than_secs`, so a cleanup that died part
    /// way through can be resumed.
    /// Abandoned project cleanups, with the tombstone value each was observed at - see
    /// [`TransactionalRepository::reclaim_stale_project`]. Capped, so a backlog cannot starve the leased
    /// trace and session sweeps that run after it in the same pass.
    async fn get_stale_claimed_projects(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Re-lease an abandoned project cleanup, if the tombstone is still the one observed.
    ///
    /// The lease *is* the tombstone's timestamp, pushed forward: `deleting_at` is both the fence and the age
    /// that makes a claim look abandoned, so refreshing it hands this project to one resumer without lifting
    /// the fence - which has to stay set until the cleanup finishes. Unleased, every replica resumed every
    /// stale project and duplicated all of its four-store cleanup.
    async fn reclaim_stale_project(
        &self,
        project_id: &ProjectId,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError>;

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
        project_id: &ProjectId,
        session_ids: &[String],
    ) -> Result<(), DataError>;

    /// Which of these sessions are tombstoned, so their spans must not be written.
    async fn deleted_sessions_among(
        &self,
        project_id: &ProjectId,
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
        project_id: &ProjectId,
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
        project_id: &ProjectId,
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
        project_id: &ProjectId,
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
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Re-lease an abandoned organization cleanup - see [`TransactionalRepository::reclaim_stale_project`].
    async fn reclaim_stale_organization(
        &self,
        org_id: &str,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError>;

    /// How many project rows an organization still has, tombstoned or not.
    async fn count_projects_of_organization(&self, org_id: &str) -> Result<i64, DataError>;

    /// Delete a project's row. Only correct once its data is gone: the row is what every other path
    /// finds the data by.
    async fn delete_project(&self, id: &str) -> Result<bool, DataError>;
}

/// The rows that name stored bytes, and the reference counting that protects them.
#[async_trait]
pub trait FileMetaStore: Send + Sync {
    /// A stable page of trace identities still named by transactional byte ownership.
    ///
    /// Restore repair must inspect both directions of a mismatched snapshot: analytics rows can have lost
    /// their ownership rows, and ownership rows can outlive analytics rows. The latter cannot be discovered
    /// by scanning analytics alone, so this pages the union of `trace_files` and `span_bodies` by trace id.
    async fn restore_association_trace_ids(
        &self,
        project_id: &ProjectId,
        after_trace_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DataError>;

    /// Upsert a file record (insert or increment ref_count)
    /// Returns the new ref_count value.
    async fn upsert_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<i64, DataError>;

    /// Get a file by project and hash
    async fn get_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<FileRow>, DataError>;

    /// Check if a file exists
    async fn file_exists(&self, project_id: &ProjectId, file_hash: &str)
    -> Result<bool, DataError>;

    /// Decrement ref_count atomically and return the new value
    /// Returns None if file doesn't exist, Some(new_ref_count) otherwise.
    async fn decrement_ref_count(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError>;

    /// Delete a file metadata record
    async fn delete_file(&self, project_id: &ProjectId, file_hash: &str)
    -> Result<bool, DataError>;

    /// Delete all file records for a project
    async fn delete_project_files(&self, project_id: &ProjectId) -> Result<u64, DataError>;

    /// Associate a file with a trace, counting the reference only if the association is new.
    ///
    /// One operation because `ref_count` must equal the number of associations - that is what
    /// deletion decrements, and doing the two separately drifted in both directions.
    async fn associate_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<bool, DataError>;

    /// How many of these traces reference each file, so deletion can decrement by that many.
    async fn get_file_reference_counts_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Associate a trace with a file that already exists, without inventing metadata for it.
    ///
    /// Returns false when there is no such file, or it is claimed for deletion - both mean the caller
    /// must not commit a reference to it.
    async fn associate_existing_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Files claimed for deletion longer ago than `older_than_secs`, so an abandoned claim can be resumed.
    async fn get_stale_claimed_files(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError>;

    /// Re-take an abandoned claim, but only if it is still exactly the one observed.
    ///
    /// The recovery path reads a snapshot and then deletes bytes; in between, the row can be released or
    /// deleted and recreated by an ingestion that associates the same content hash. Gating the byte deletion
    /// on this compare-and-set is what stops it removing content a committed span references.
    async fn reclaim_stale_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError>;

    /// Claim a file for deletion, if nothing references it and nobody else has claimed it.
    ///
    /// The fence that closes the delete-then-recreate window: association refuses while a claim is set,
    /// because a claimed file's bytes may already be gone.
    async fn claim_file_for_deletion(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Give up a deletion claim, leaving the file in place.
    async fn release_deletion_claim(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<(), DataError>;

    /// Put back a metadata row whose bytes could not be deleted, as an orphan for a later sweep.
    async fn restore_orphan_metadata(
        &self,
        project_id: &ProjectId,
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
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError>;

    /// Recompute a file's reference count from the associations that exist, and return it.
    ///
    /// The count is a cached `COUNT(*)`; deriving it is the only form immune to two concurrent
    /// cleanups both subtracting the same references.
    async fn sync_ref_count(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError>;

    /// Insert a trace-file association
    async fn insert_trace_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<(), DataError>;

    /// Get file hashes for traces
    async fn get_file_hashes_for_traces(
        &self,
        project_id: &ProjectId,
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
        project_id: &ProjectId,
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
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError>;

    /// Record that these traces need file and favourite cleanup, **before** the spans are deleted.
    ///
    /// DuckDB commits the span deletion and the cleanup runs afterwards, asynchronously, with failures only
    /// logged. A crash or a transactional-store outage in between loses the only record of which traces needed
    /// cleaning - and their spans are already gone, so no later pass can rediscover them. Their associations,
    /// ref-counted bytes and favourites are orphaned permanently.
    ///
    /// Idempotent for the *row*, and it **bumps the token**, so a claim held by an earlier worker no longer
    /// matches: re-recording means there is new work behind the same identity, and a stale completion must not
    /// discard it.
    /// Returns each trace with the **token it now carries**, so the recording pass can complete on exactly the
    /// rows it wrote. Guessing a token would either fail to complete (leaving a record the sweep re-drives) or,
    /// worse, match a newer one.
    async fn record_retention_cleanup(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Claim due cleanup candidates, leasing them so a concurrent instance takes different ones.
    ///
    /// The claim pushes `next_attempt_at` out **before** returning, which is what stops a slow reconciliation
    /// being re-claimed while it runs - the same discipline the deleted-project and deleted-trace sweeps use,
    /// and for the same reason: without it every replica drains the same rows.
    async fn claim_retention_cleanup(
        &self,
        limit: i64,
        lease_secs: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError>;

    /// Drop candidates whose cleanup has completed, **only if still on the claimed token**.
    ///
    /// Deleting by identity alone let a stale worker remove a *newer* intent. A worker claims trace T, pauses
    /// past its lease; retention runs again for T, re-uses the row and deletes more spans; the paused worker
    /// then finishes its old work and deletes the row - so if the newer worker fails, the cleanup it recorded is
    /// gone and nothing can rediscover it. Comparing the token makes the completion refer to the work that was
    /// actually claimed.
    async fn complete_retention_cleanup(
        &self,
        project_id: &ProjectId,
        completed: &[(String, i64)],
    ) -> Result<(), DataError>;

    /// Restore an association a survivor scan released, as **durable**.
    ///
    /// The compensation path: a span committed between the scan and the release. `durable` rather than
    /// provisional, because the reference belongs to a *committed* span - restoring it as provisional lets a
    /// later failing batch's release delete it, since that release deletes a non-durable row with no writer left.
    async fn restore_durable_trace_file(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        file_hash: &str,
    ) -> Result<(), DataError>;

    /// Release a trace's associations **except** the ones its surviving spans still reference.
    ///
    /// The counterpart to [`Self::delete_trace_files`], and the distinction is the whole point: that one is
    /// for a trace that is *gone*, this one for a trace that has merely had some of its spans expired.
    /// Retention selects individual span identities, so handing it the trace-wide delete removed **every**
    /// association for a trace whose other spans were still live - leaving those spans pointing at bytes that
    /// had been reclaimed, which is precisely the dangling reference the write-files-before-rows ordering
    /// exists to prevent, produced by retention instead.
    ///
    /// Two conditions, and neither is optional:
    ///
    /// - `file_hash NOT IN keep`, so a file a survivor references is untouched.
    /// - `pending_writers = 0`, which is what protects a batch in flight. Referencing a file increments that
    ///   counter *before* the span row exists, so a concurrent ingestion is invisible to the survivor scan -
    ///   there is no span to find yet. Without this the reconciliation is a read-then-act race with exactly
    ///   the window it is meant to close.
    ///
    /// Returns the hashes actually removed, so the caller reconciles the set the statement produced rather
    /// than one it read beforehand - the same reason `delete_trace_files` uses `RETURNING`.
    async fn release_trace_files_except(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        keep: &[String],
    ) -> Result<Vec<String>, DataError>;

    /// Get total storage used by a project
    async fn get_project_storage_bytes(&self, project_id: &ProjectId) -> Result<i64, DataError>;

    /// Get all files with zero ref_count (for cleanup)
    async fn get_orphan_files(&self) -> Result<Vec<(String, String)>, DataError>;

    /// Get total file storage used by all projects in an organization
    async fn get_org_file_storage_bytes(&self, org_id: &str) -> Result<i64, DataError>;

    /// Get total file storage used across all orgs a user belongs to
    async fn get_user_file_storage_bytes(&self, user_id: &str) -> Result<i64, DataError>;
}

/// Transactional ownership of content-addressed span bodies.
#[async_trait]
pub trait ContentBodyStore: Send + Sync {
    /// Register content-addressed objects before their bytes are written.
    ///
    /// Returns the logical bytes of objects that were new to this project. Registration alone is not a
    /// readable reference; only a durable `span_bodies` row makes an object live.
    async fn register_content_bodies(
        &self,
        objects: &[ContentBodyObject],
    ) -> Result<Vec<ContentBodyObject>, DataError>;

    /// Return associations that do not already have this exact durable body.
    ///
    /// Re-delivery of an unchanged span is the steady-state ingest path. It must not rewrite the same
    /// content-addressed object or churn its ownership row on every delivery.
    async fn unresolved_span_bodies(
        &self,
        associations: &[SpanBodyAssociation],
    ) -> Result<Vec<SpanBodyAssociation>, DataError>;

    async fn stage_span_bodies(
        &self,
        associations: &[SpanBodyAssociation],
    ) -> Result<u64, DataError>;

    async fn confirm_span_bodies(
        &self,
        associations: &[SpanBodyAssociation],
    ) -> Result<u64, DataError>;

    async fn release_span_body(&self, association: &SpanBodyAssociation)
    -> Result<bool, DataError>;

    async fn get_span_body_hash(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
        field: SpanBodyField,
    ) -> Result<Option<String>, DataError>;

    async fn get_orphan_content_bodies(
        &self,
        limit: usize,
    ) -> Result<Vec<(ProjectId, String)>, DataError>;

    /// List deletion claims old enough that their worker may have crashed.
    async fn get_stale_claimed_content_bodies(
        &self,
        older_than: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<(ProjectId, String)>, DataError>;

    async fn claim_content_body_for_deletion(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<bool, DataError>;

    async fn release_content_body_deletion_claim(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<(), DataError>;

    async fn delete_claimed_content_body(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<bool, DataError>;

    async fn delete_span_bodies(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<Vec<String>, DataError>;

    async fn delete_trace_bodies(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError>;

    async fn delete_project_bodies(&self, project_id: &ProjectId)
    -> Result<Vec<String>, DataError>;

    /// Replace durable body ownership for selected traces with the exact winning-span field set.
    ///
    /// Provisional rows (`pending_writers > 0`) are never removed; they belong to an ingest that has not
    /// reached its analytics write yet.
    async fn reconcile_span_bodies(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        keep: &[SpanBodyAssociation],
    ) -> Result<Vec<String>, DataError>;

    async fn content_body_backfill_progress(
        &self,
        project_id: &ProjectId,
    ) -> Result<Option<ContentBodyBackfillProgress>, DataError>;

    async fn save_content_body_backfill_progress(
        &self,
        progress: &ContentBodyBackfillProgress,
    ) -> Result<(), DataError>;

    async fn reset_content_body_backfill(&self, project_id: &ProjectId) -> Result<(), DataError>;
}

/// API keys, stored as a hash.
#[async_trait]
pub trait ApiKeyStore: Send + Sync {
    /// Create API key. Returns Err(Conflict) if limit (100) exceeded.
    #[allow(clippy::too_many_arguments)]
    async fn create_api_key(
        &self,
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
        key_hash: &str,
    ) -> Result<Option<ApiKeyValidation>, DataError>;

    /// List all keys for organization (metadata only, ordered by created_at DESC).
    async fn list_api_keys(&self, org_id: &str) -> Result<Vec<ApiKeyRow>, DataError>;

    /// Delete key by ID.
    async fn delete_api_key(&self, id: &str, org_id: &str) -> Result<bool, DataError>;

    /// Update last_used_at (debounced, only if older than threshold).
    async fn touch_api_key(&self, id: &str, threshold_secs: u64) -> Result<bool, DataError>;

    /// Delete all keys for organization (for org deletion cleanup).
    async fn delete_api_keys_for_org(&self, org_id: &str) -> Result<u64, DataError>;

    /// Get key hashes for organization (for cache invalidation on org delete).
    async fn get_api_key_hashes_for_org(&self, org_id: &str) -> Result<Vec<String>, DataError>;
}

/// Provider credentials and their per-project permissions.
#[async_trait]
pub trait CredentialStore: Send + Sync {
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
        project_id: Option<&ProjectId>,
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
        project_id: &ProjectId,
    ) -> Result<Vec<String>, DataError>;
}

/// Favourites, keyed on the entity they mark.
#[async_trait]
pub trait FavoriteStore: Send + Sync {
    /// Add a favorite
    /// For spans, secondary_id is the span_id (entity_id is trace_id)
    async fn add_favorite(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_id: &str,
        secondary_id: Option<&str>,
        project_id: &ProjectId,
    ) -> Result<bool, DataError>;

    /// Remove a favorite
    /// For spans, secondary_id is the span_id (entity_id is trace_id)
    async fn remove_favorite(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_id: &str,
        secondary_id: Option<&str>,
        project_id: &ProjectId,
    ) -> Result<bool, DataError>;

    /// Check if entities are favorited
    async fn check_favorites(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &ProjectId,
    ) -> Result<Vec<String>, DataError>;

    /// Check if spans are favorited
    async fn check_span_favorites(
        &self,
        user_id: &str,
        span_ids: &[(String, String)],
        project_id: &ProjectId,
    ) -> Result<Vec<(String, String)>, DataError>;

    /// Count favorites for a user
    async fn count_favorites(
        &self,
        user_id: &str,
        project_id: &ProjectId,
    ) -> Result<i64, DataError>;

    /// List all favorite entity IDs for a user
    async fn list_favorite_ids(
        &self,
        user_id: &str,
        entity_type: &str,
        project_id: &ProjectId,
    ) -> Result<Vec<String>, DataError>;

    /// Delete favorites by entity (for cascade delete)
    async fn delete_favorites_by_entity(
        &self,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &ProjectId,
    ) -> Result<u64, DataError>;
}

/// Everything an analytics adapter provides, as one bound.
///
/// **A bundle of the narrow ports, not a trait with its own methods.** It was a single 31-method trait, which
/// meant a caller needing spans depended on session statistics and on every delete - and no consumer could state
/// what it actually used. The narrow ports are the seams; this exists so the composition root can hand out one
/// object, and so `dyn AnalyticsRepository` upcasts to whichever port a caller wants.
///
/// The blanket impl is what keeps it a bundle: implementing the parts *is* implementing this, so no adapter
/// writes an empty impl and nothing can drift between the two.
#[async_trait]
pub trait AnalyticsRepository:
    SpanStore
    + MetricStore
    + LogStore
    + SearchIndex
    + EntityQuery
    + MessageStore
    + AnalyticsMaintenance
    + SurvivorReferences
{
}

impl<T> AnalyticsRepository for T where
    T: SpanStore
        + MetricStore
        + LogStore
        + SearchIndex
        + EntityQuery
        + MessageStore
        + AnalyticsMaintenance
        + SurvivorReferences
{
}

/// What was removed, for a deletion whose reason cannot be recomputed from the data that remains.
///
/// The distinction the journal turns on. **Age retention writes nothing here**, deliberately: it is a
/// predicate, so a restored database recomputes exactly the same verdict from the timestamps it holds, and an
/// entry per aged-out record would be an unbounded write for a fact that is already derivable. What is *not*
/// derivable is a caller's request and a limit having been reached, and those are what the journal holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionCause {
    /// A caller asked for it - a trace, session, project or organization deletion.
    Requested,
    /// A limit was reached: count retention (`max_spans`) or quota reclamation. Not reproducible, because it
    /// happened *because* a limit was hit, and a restored database is not at that limit.
    ///
    /// **Nothing writes this yet, and the reason is a granularity question the plan does not settle.** Count
    /// retention selects individual span identities, up to `RETENTION_BATCH_SIZE` (100 000) per batch, and the
    /// journal is permanent and counted against the project's quota - so an entry per evicted span is roughly
    /// 8 MB of permanent rows per full batch, forever. One entry per affected *trace* is bounded and cheap and
    /// answers the wrong question: it would tell the re-drive sweep that a span was deliberately evicted when
    /// only a sibling was, so the sweep would decline to re-drive a payload whose write actually failed - loss,
    /// in the direction this whole subsystem refuses. Omitting the entry is the other error: the sweep re-drives
    /// a payload whose records were evicted, the eviction happens again, and it repeats to the re-drive cap -
    /// bounded, reported, and recoverable.
    ///
    /// So the cautious error is taken until the mechanism that reads it exists. The variant is declared because
    /// the *stored* vocabulary has to be settled before rows are written under it, and the `CHECK` constraint in
    /// both schemas already admits it.
    Pressure,
}

impl DeletionCause {
    /// The stored spelling. Explicit rather than derived from the variant name, so renaming a variant cannot
    /// silently orphan every row already written under the old name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Pressure => "pressure",
        }
    }

    /// Parse a stored spelling. Named `from_stored` rather than `from_str` because it is not `FromStr`: an
    /// unknown spelling is `None` rather than an error, since only a newer writer can produce one.
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "requested" => Some(Self::Requested),
            "pressure" => Some(Self::Pressure),
            _ => None,
        }
    }
}

/// The kind of thing a journal entry names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionScope {
    Trace,
    Session,
    Project,
    Organization,
    /// One span, which only pressure eviction produces: nothing asks to delete a single span.
    Span,
}

impl DeletionScope {
    /// The stored spelling - see [`DeletionCause::as_str`] for why it is written out.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Session => "session",
            Self::Project => "project",
            Self::Organization => "organization",
            Self::Span => "span",
        }
    }

    /// Parse a stored spelling - see [`DeletionCause::from_stored`] for the name.
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "trace" => Some(Self::Trace),
            "session" => Some(Self::Session),
            "project" => Some(Self::Project),
            "organization" => Some(Self::Organization),
            "span" => Some(Self::Span),
            _ => None,
        }
    }
}

/// One deletion, as ids, kind and instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionRecord {
    /// The project - or, for an [`DeletionScope::Organization`] entry, the organization.
    ///
    /// One column rather than two, because what it means for both is "the tenant this entry is charged to":
    /// the journal is inside the per-project storage quota, and an organization's own deletion has no project
    /// to charge. A second nullable column would have to be joined or coalesced at every read to answer the
    /// same question.
    pub project_id: ProjectId,
    pub cause: DeletionCause,
    pub scope: DeletionScope,
    /// The trace, session, project or organization id.
    pub target_id: String,
    /// Set only for [`DeletionScope::Span`], where the target is the span's trace.
    pub span_id: Option<String>,
    pub recorded_at: DateTime<Utc>,
}

impl DeletionRecord {
    pub fn logical_bytes_for<P: AsRef<str> + ?Sized>(
        project_id: &P,
        cause: DeletionCause,
        scope: DeletionScope,
        target_id: &str,
        span_id: Option<&str>,
    ) -> u64 {
        64u64
            .saturating_add(project_id.as_ref().len() as u64)
            .saturating_add(cause.as_str().len() as u64)
            .saturating_add(scope.as_str().len() as u64)
            .saturating_add(target_id.len() as u64)
            .saturating_add(span_id.map(str::len).unwrap_or_default() as u64)
    }

    /// Stable logical size charged to the project's storage budget.
    ///
    /// The fixed portion accounts for the sequence, instant, nullable tag and row framing; strings are charged
    /// by UTF-8 bytes so SQLite and PostgreSQL report the same value for identical records.
    pub fn logical_bytes(&self) -> u64 {
        Self::logical_bytes_for(
            &self.project_id,
            self.cause,
            self.scope,
            &self.target_id,
            self.span_id.as_deref(),
        )
    }
}

/// The append-only record of deletions a restore cannot recompute.
///
/// Two consumers, and each needs something the other does not:
///
/// - **A restore replays it forward before serving reads.** A snapshot predating a deletion predates its
///   tombstone too, so restoring the analytics store further back than the transactional one - which is what
///   different backup cadences produce - resurrects rows the caller was told were gone. Replaying the journal
///   removes them again.
/// - **The staged-payload re-drive sweep asks whether an absence was intended.** That sweep looks for staged
///   payloads whose records are absent and re-drives them; without the journal it cannot tell a failed write
///   from a deliberate deletion or a pressure eviction, so it recreates exactly what those removed, and the
///   eviction happens again, until the re-drive cap is exhausted.
///
/// **Never truncated by any sweep, and permanent.** Any retention here would be a bound on how late a stalled
/// writer may commit, which is the thing this whole family of records exists because nothing bounds. The cost
/// is stated rather than hidden: the journal is inside the storage quota, so a project that deletes enough can
/// be refused even after every telemetry byte is reclaimed. That is the correct direction - discarding the
/// record of a deletion to admit new writes trades a durable guarantee for throughput - and it is survivable
/// because the entries are ids and instants rather than payloads.
///
/// **Appended before the deletion it records, never after.** A crash between them must leave a record with no
/// deletion (harmless: the replay removes something already gone, and every step is idempotent) rather than a
/// deletion with no record (the resurrection this exists to prevent). Same ordering as the tombstone, for the
/// same reason.
#[async_trait]
pub trait DeletionJournal: Send + Sync {
    /// Append these deletions.
    ///
    /// A batch, because a trace deletion route removes many traces in one request and one round trip per trace
    /// would make the journal the cost of deleting. Atomic: a partial append is a deletion with no record for
    /// some of its targets, which is the state the ordering above exists to make impossible.
    ///
    /// **Prefer the combined methods below where one exists.** Appending on its own leaves a window in whichever
    /// direction the caller picked: journal first and a failed tombstone leaves a record for a deletion that did
    /// not happen, which a restore replays as a deletion the caller was told had failed; tombstone first and a
    /// failed append leaves a deletion the sweeps will perform anyway with no record, which a restore undoes.
    /// Both writes land in *this* store, so the window is avoidable rather than a trade.
    async fn append_deletions(&self, records: &[DeletionRecord]) -> Result<(), DataError>;

    /// Tombstone these traces and journal their deletion, in one transaction.
    ///
    /// The pair has to be atomic, and an ordering cannot substitute for that. A tombstone is not inert - the
    /// deletion sweeps act on it and remove the rows later - so with the tombstone first a failed append leaves a
    /// deletion that happens anyway and no record of it; with the append first a failed tombstone leaves a
    /// permanent record for a deletion the caller was told had failed, and a restore replays it. Both rows live
    /// in the transactional store, so there is no reason for either.
    async fn record_deleted_traces_journalled(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<(), DataError>;

    /// Tombstone these sessions **and** the traces they resolved to, and journal all of it, in one transaction.
    ///
    /// Four writes rather than two, for the reason `record_deleted_sessions` exists: a session is deleted by
    /// deleting its traces, so both tombstones are needed, and the journal needs both scopes - the session entry
    /// is the durable fact a replay re-resolves, the trace entries are what remains when the session is no longer
    /// resolvable from restored data at all.
    async fn record_deleted_sessions_journalled(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
        trace_ids: &[String],
    ) -> Result<(), DataError>;

    /// Journal requested exact-span deletions and record their trace cleanup atomically.
    async fn record_deleted_spans_journalled(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Persist pressure-eviction journal rows and cleanup candidates in one transaction.
    async fn record_pressure_eviction(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<Vec<(String, i64)>, DataError>;

    /// Claim a project for deletion and journal it, in one transaction.
    ///
    /// Returns whether this caller won the claim. The journal entry is written **only when it did**, which the
    /// separate calls could not achieve: journalling before the claim wrote an entry for every losing caller -
    /// and since an organization cleanup re-runs while its projects' tombstones remain, that was an unbounded
    /// number of permanent, quota-counted records for one deletion.
    async fn claim_project_for_deletion_journalled(&self, id: &str) -> Result<bool, DataError>;

    /// Claim an organization for deletion and journal it, in one transaction. See the project twin.
    async fn claim_organization_for_deletion_journalled(&self, id: &str)
    -> Result<bool, DataError>;

    /// Entries after `after_sequence`, oldest first, at most `limit`.
    ///
    /// The sequence is the journal's own append order and is returned with each entry, so a replay that is
    /// interrupted resumes from what it has applied rather than from a timestamp - two entries can share a
    /// microsecond, and a clock is not an order (the analytics stores have no commit-ordered sequence at all,
    /// which is why this one is a column the transactional store assigns).
    ///
    /// Returns `(entries, highest_sequence_examined)`, and the second value is not a convenience. A row whose
    /// cause or scope this build does not understand is skipped rather than guessed at, so a page can yield
    /// *fewer* entries than it examined - and a page consisting entirely of such rows yields none at all, which
    /// against a cursor advanced only by returned entries is indistinguishable from the end of the journal. The
    /// replay would stop there and never reach the known deletions behind them. Advancing on what was
    /// **examined** makes progress a property of the page rather than of what happened to be interpretable -
    /// the same reasoning as the search cursor's last-*examined* position in §4.3 of the plan.
    ///
    /// `highest_sequence_examined` equals `after_sequence` when the page was empty, so a caller can loop until
    /// it stops advancing.
    async fn deletions_since(
        &self,
        after_sequence: i64,
        limit: usize,
    ) -> Result<(Vec<(i64, DeletionRecord)>, i64), DataError>;

    /// Whether the journal explains this record's absence.
    ///
    /// What the re-drive sweep asks before recreating a staged payload's records. Scoped by project and
    /// target, and a `Span` target is answered by an entry for the span **or** for its trace, because
    /// deleting the trace removes the span too.
    async fn deletion_is_journaled(
        &self,
        project_id: &ProjectId,
        scope: DeletionScope,
        target_id: &str,
        span_id: Option<&str>,
    ) -> Result<bool, DataError>;

    /// Span identities already covered by a span- or trace-scoped journal entry.
    async fn journaled_spans_among(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<HashSet<(String, String)>, DataError>;

    /// Exact span deletions recorded for these traces.
    ///
    /// A cleanup candidate stores one row per trace, while pressure/requested deletion records are exact span
    /// identities. Re-reading those identities lets cleanup re-apply the analytical deletion after a crash or a
    /// writer that committed in the cross-store window, before it reconciles the trace's surviving file
    /// references.
    async fn journaled_span_deletions_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, String)>, DataError>;
}

/// Stable logical size of one retention-cleanup candidate.
pub fn retention_cleanup_logical_bytes<P: AsRef<str> + ?Sized>(
    project_id: &P,
    trace_id: &str,
) -> u64 {
    64u64
        .saturating_add(project_id.as_ref().len() as u64)
        .saturating_add(trace_id.len() as u64)
}

/// Durable metadata for payload bytes staged before OTLP acknowledgement.
///
/// The blob itself lives behind `FileStorage`; this registry is what makes queue loss discoverable and
/// records the bounded redrive state. Rows have no TTL and leave only through confirmation or a proven
/// deletion.
#[async_trait]
pub trait StagedPayloadStore: Send + Sync {
    async fn create_staged_payload(&self, payload: &StagedPayload) -> Result<(), DataError>;

    async fn get_staged_payload(&self, id: &str) -> Result<Option<StagedPayload>, DataError>;

    /// Oldest payloads that have not exhausted the redrive cap.
    async fn pending_staged_payloads(&self, limit: usize) -> Result<Vec<StagedPayload>, DataError>;

    /// Increment and return the durable attempt count.
    async fn increment_staged_redrive_attempts(&self, id: &str) -> Result<u32, DataError>;

    async fn mark_staged_unconfirmed(&self, id: &str) -> Result<(), DataError>;

    async fn delete_staged_payload(&self, id: &str) -> Result<(), DataError>;
}

/// Durable legal-hold state, maintenance fencing and quota admission.
///
/// Hold recording and retention batches share the same leased per-project mutex. The usage counter is
/// deliberately best-effort: admission increments it atomically, while reconciliation replaces it from
/// independently measured storage.
#[async_trait]
pub trait StorageGovernance: Send + Sync {
    async fn active_project_hold(
        &self,
        project_id: &ProjectId,
        now: DateTime<Utc>,
    ) -> Result<Option<ProjectHold>, DataError>;

    async fn list_active_project_holds(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<ProjectHold>, DataError>;

    async fn set_project_hold(
        &self,
        project_id: &ProjectId,
        hold_until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<ProjectHold, DataError>;

    async fn clear_project_hold(&self, project_id: &ProjectId) -> Result<bool, DataError>;

    async fn acquire_project_maintenance(
        &self,
        project_id: &ProjectId,
        owner: &str,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> Result<bool, DataError>;

    async fn release_project_maintenance(
        &self,
        project_id: &ProjectId,
        owner: &str,
    ) -> Result<(), DataError>;

    /// Reserve ordinary-write capacity. Returns the resulting measured usage, or `None` on refusal.
    async fn reserve_project_storage(
        &self,
        project_id: &ProjectId,
        additional_bytes: u64,
        ordinary_limit_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<Option<ProjectStorageUsage>, DataError>;

    async fn project_storage_usage(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectStorageUsage, DataError>;

    async fn replace_project_storage_usage(
        &self,
        project_id: &ProjectId,
        logical_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<ProjectStorageUsage, DataError>;

    /// Held transactional bytes: staged payloads plus the permanent deletion journal.
    async fn held_transactional_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError>;

    /// Projects participating in accounting, including projects whose measured usage is still zero.
    async fn storage_project_ids(&self, limit: usize) -> Result<Vec<ProjectId>, DataError>;
}

/// Everything a transactional adapter provides, as one bound. See [`AnalyticsRepository`] for why this is a
/// bundle rather than a trait of its own: it was 95 methods, and the seven cohesive ports above are what a
/// caller can actually name.
pub trait TransactionalRepository:
    IdentityStore
    + ProjectStore
    + FileMetaStore
    + ContentBodyStore
    + ApiKeyStore
    + CredentialStore
    + FavoriteStore
    + DeletionJournal
    + StagedPayloadStore
{
}

impl<T> TransactionalRepository for T where
    T: IdentityStore
        + ProjectStore
        + FileMetaStore
        + ContentBodyStore
        + ApiKeyStore
        + CredentialStore
        + FavoriteStore
        + DeletionJournal
        + StagedPayloadStore
{
}
