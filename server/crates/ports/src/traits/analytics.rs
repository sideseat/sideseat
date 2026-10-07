use super::*;

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

    /// The latest raw records the surviving winning spans of these traces name.
    ///
    /// A record references media the derived fields may not: a run too short to extract, an attribute no rule
    /// reads. Those objects are owned exactly as extracted files are, so survivor reconciliation must keep
    /// what these records reference, or a record a surviving row names could no longer be decoded.
    async fn survivor_raw_records(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<Vec<u8>>, DataError>;
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

    /// Get at most `limit` spans for a trace.
    async fn get_spans_for_trace(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        limit: usize,
    ) -> Result<Vec<SpanRow>, DataError>;

    /// Get a single span by ID
    async fn get_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Option<SpanRow>, DataError>;

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

    /// The `(trace_id, span_id, content_digest)` records whose winning revision carries exactly that digest,
    /// in one read for the whole batch. The digest is part of the answer because a batch may carry two
    /// revisions of one span, and only the one matching the stored winner is a redelivery.
    async fn spans_with_matching_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String, String)],
    ) -> Result<HashSet<(String, String, String)>, DataError>;
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
    ) -> Result<Vec<SearchBackfillSource>, DataError>;

    /// Persist domain-produced term documents and their complete markers.
    async fn write_search_backfill(
        &self,
        project_id: &ProjectId,
        signal: SearchSignal,
        documents: &[SearchBackfillDocument],
    ) -> Result<(), DataError>;
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
    /// Membership comes from the store because `MESSAGE_CONTENT_FILTER` may remove the root span carrying
    /// the session id. The returned mapping gives replay detection a complete conversation grouping even
    /// when the supplied message rows are filtered.
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
    /// Distinct projects represented by any analytics signal.
    ///
    /// Restore repair cannot seed its traversal only from the transactional store: an analytics backup may
    /// be newer and contain a project whose metadata is outside the transactional recovery point.
    async fn analytics_project_ids(&self, limit: usize) -> Result<Vec<ProjectId>, DataError>;

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
    /// The feed uses this as its traversal watermark so page selection and committed data share the store's
    /// time domain rather than depending on the reader's clock.
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
    ) -> Result<(), DataError>;

    /// Logical bytes currently attributable to a project across analytics signals.
    async fn project_logical_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError>;

    /// Logical bytes protected by an active hold at `now`.
    async fn project_held_logical_bytes(
        &self,
        project_id: &ProjectId,
        now: DateTime<Utc>,
    ) -> Result<u64, DataError>;

    /// Select a bounded oldest-first batch of winning, non-held spans whose bytes cross `target_bytes`.
    async fn oldest_reclaimable_spans(
        &self,
        project_id: &ProjectId,
        target_bytes: u64,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<PressureSpanCandidate>, DataError>;
}

/// Stored raw exports, keyed per project by `raw_id`, each with its versions.
///
/// A record is written before the rows derived from it and lives as long as a row names it. The protocol that
/// keeps the two agreeing - under redelivery, deletion, retention, legal hold and restore - is modelled in
/// `server/specs/RawRecordOwnership.tla`: every deletion or expiry of rows enqueues the records they named
/// ([`RawStore::pending_raw_records`]), and a reconciler rewrites each without its deleted spans, or deletes
/// it when no row names it.
#[async_trait]
pub trait RawStore: Send + Sync {
    /// Store the records whose `raw_id` is not stored yet; a record already present is left as it is.
    async fn insert_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError>;

    /// Store these versions unconditionally: a repair, a rewrite, or a record restored after a delete.
    async fn append_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError>;

    /// The latest version of each requested record that exists.
    async fn get_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<Vec<RawRecordRow>, DataError>;

    /// One page of latest versions in `(received_at, raw_id)` order, after the given position: the order a
    /// re-derivation replays.
    async fn raw_records_page(
        &self,
        project_id: &ProjectId,
        after: Option<(DateTime<Utc>, String)>,
        limit: usize,
    ) -> Result<Vec<RawRecordRow>, DataError>;

    /// Delete every version of these records.
    async fn delete_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<(), DataError>;

    /// The record each of these spans was derived from, for the winning row of each identity.
    async fn span_raw_ids(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<std::collections::HashMap<(String, String), String>, DataError>;

    /// Which of these records a stored row still names.
    async fn raw_records_named(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError>;

    /// Enqueue records for reconciliation.
    async fn enqueue_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<(), DataError>;

    /// Up to `limit` queued entries, oldest first, across projects.
    async fn pending_raw_records(&self, limit: usize) -> Result<Vec<RawPending>, DataError>;

    /// Remove exactly these entries; one enqueued since they were read stays.
    async fn clear_raw_pending(&self, entries: &[RawPending]) -> Result<(), DataError>;
}
