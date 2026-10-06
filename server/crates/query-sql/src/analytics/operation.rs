/// A stable name for an operation whose SQL is owned by this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryOperation {
    /// Fetch the winning delivery of one span identity.
    GetSpan,
    /// List winning span deliveries under filters and offset pagination.
    ListSpans,
    /// List trace aggregates with entity-level filtering.
    ListTraces,
    /// Delete every row belonging to selected traces.
    DeleteTraces,
    /// Delete every row belonging to selected span identities.
    DeleteSpans,
    /// Delete every analytical row belonging to one project.
    DeleteProjectData,
    /// Append span revisions through the backend's typed bulk-write API.
    UpsertSpans,
    /// Replace metric datapoint winners and append their rows.
    UpsertMetrics,
    /// Replace retried log-record identities and append their rows.
    UpsertLogs,
    /// List winning metric datapoints under filters and offset pagination.
    ListMetrics,
    /// Read one winning metric datapoint by its deterministic identity.
    GetMetric,
    /// Aggregate filtered winning metric datapoints by metric name and type.
    AggregateMetrics,
    /// Read metric dropdown values and exact winning-datapoint counts.
    GetMetricFilterOptions,
    /// List winning log records under correlation and time filters.
    ListLogs,
    /// Read one log record by semantic digest and export ordinal.
    GetLog,
    /// Read log dropdown values and exact winning-record counts.
    GetLogFilterOptions,
    /// Resolve each selected trace to its canonical session.
    GetTraceSessionPairs,
    /// Resolve selected traces to their distinct canonical sessions.
    GetSessionIdsForTraces,
    /// Resolve selected sessions to the traces canonically owned by them.
    GetTraceIdsForSessions,
    /// Read event/link counts for selected winning span identities.
    GetSpanCountsBulk,
    /// Read which requested traces still have winning rows.
    TracesWithoutSpans,
    /// Read file-reference-bearing fields from selected traces' winning rows.
    FileReferenceFieldsForTraces,
    /// Read content-addressable body fields with their exact span identities.
    SpanBodyFieldsForTraces,
    /// Read one identity-ordered page for resumable content-body backfill.
    SpanBodyBackfillPage,
    /// Count every analytical row still owned by one project.
    CountProjectRows,
    /// Enumerate projects represented by any analytics signal.
    AnalyticsProjectIds,
    /// Count winning span identities for an explicit project set.
    CountSpansByProject,
    /// Read all winning spans for one trace.
    GetSpansForTrace,
    /// Aggregate one trace by id.
    GetTrace,
    /// Aggregate every trace canonically belonging to one session.
    GetTracesForSession,
    /// Aggregate one canonical session.
    GetSession,
    /// List canonical sessions with entity-level filtering and offset pagination.
    ListSessions,
    /// Read one stable cursor page of winning span deliveries.
    GetFeedSpans,
    /// Read trace dropdown values and exact trace counts.
    GetTraceFilterOptions,
    /// Read trace tag dropdown values and exact trace counts.
    GetTraceTagsOptions,
    /// Read span dropdown values and exact winning-span counts.
    GetSpanFilterOptions,
    /// Read session dropdown values and exact canonical-session counts.
    GetSessionFilterOptions,
    /// Read message-bearing span context for one selector.
    GetMessages,
    /// Read one cursor page of project message spans.
    GetProjectMessages,
    /// Read every project-statistics aggregate through one typed plan.
    GetProjectStats,
    /// Read the newest committed ingestion timestamp for one project.
    MaxIngestedAtUs,
    /// Enforce analytical-store retention through backend-specific typed statements.
    EnforceRetention,
    /// Delete every trace canonically owned by selected sessions.
    DeleteSessions,
    /// Chronological text search, completeness probes, arrivals and historical backfill.
    Search,
    /// Enqueue raw records for reconciliation with their rows.
    RawReconciliation,
}

impl QueryOperation {
    pub const fn name(self) -> &'static str {
        match self {
            Self::GetSpan => "get_span",
            Self::ListSpans => "list_spans",
            Self::ListTraces => "list_traces",
            Self::DeleteTraces => "delete_traces",
            Self::DeleteSpans => "delete_spans",
            Self::DeleteProjectData => "delete_project_data",
            Self::UpsertSpans => "upsert_spans",
            Self::UpsertMetrics => "upsert_metrics",
            Self::UpsertLogs => "upsert_logs",
            Self::ListMetrics => "list_metrics",
            Self::GetMetric => "get_metric",
            Self::AggregateMetrics => "aggregate_metrics",
            Self::GetMetricFilterOptions => "get_metric_filter_options",
            Self::ListLogs => "list_logs",
            Self::GetLog => "get_log",
            Self::GetLogFilterOptions => "get_log_filter_options",
            Self::GetTraceSessionPairs => "get_trace_session_pairs",
            Self::GetSessionIdsForTraces => "get_session_ids_for_traces",
            Self::GetTraceIdsForSessions => "get_trace_ids_for_sessions",
            Self::GetSpanCountsBulk => "get_span_counts_bulk",
            Self::TracesWithoutSpans => "traces_without_spans",
            Self::FileReferenceFieldsForTraces => "file_reference_fields_for_traces",
            Self::SpanBodyFieldsForTraces => "span_body_fields_for_traces",
            Self::SpanBodyBackfillPage => "span_body_backfill_page",
            Self::CountProjectRows => "count_project_rows",
            Self::AnalyticsProjectIds => "analytics_project_ids",
            Self::CountSpansByProject => "count_spans_by_project",
            Self::GetSpansForTrace => "get_spans_for_trace",
            Self::GetTrace => "get_trace",
            Self::GetTracesForSession => "get_traces_for_session",
            Self::GetSession => "get_session",
            Self::ListSessions => "list_sessions",
            Self::GetFeedSpans => "get_feed_spans",
            Self::GetTraceFilterOptions => "get_trace_filter_options",
            Self::GetTraceTagsOptions => "get_trace_tags_options",
            Self::GetSpanFilterOptions => "get_span_filter_options",
            Self::GetSessionFilterOptions => "get_session_filter_options",
            Self::GetMessages => "get_messages",
            Self::GetProjectMessages => "get_project_messages",
            Self::GetProjectStats => "get_project_stats",
            Self::MaxIngestedAtUs => "max_ingested_at_us",
            Self::EnforceRetention => "enforce_retention",
            Self::DeleteSessions => "delete_sessions",
            Self::Search => "search",
            Self::RawReconciliation => "raw_reconciliation",
        }
    }

    /// Adapter function that executes this operation.
    pub const fn adapter_function(self) -> &'static str {
        match self {
            Self::UpsertSpans | Self::UpsertMetrics | Self::UpsertLogs => "insert_batch",
            Self::EnforceRetention => "run_retention",
            Self::RawReconciliation => "enqueue",
            _ => self.name(),
        }
    }

    /// Repository source that executes this operation.
    pub const fn adapter_repository(self) -> &'static str {
        match self {
            Self::UpsertSpans => "span.rs",
            Self::UpsertMetrics
            | Self::ListMetrics
            | Self::GetMetric
            | Self::AggregateMetrics
            | Self::GetMetricFilterOptions => "metric.rs",
            Self::UpsertLogs | Self::ListLogs | Self::GetLog | Self::GetLogFilterOptions => {
                "log.rs"
            }
            Self::GetMessages | Self::GetProjectMessages => "messages.rs",
            Self::GetProjectStats => "stats.rs",
            Self::EnforceRetention => "../retention.rs",
            Self::Search => "search.rs",
            Self::RawReconciliation => "raw.rs",
            _ => "query.rs",
        }
    }

    /// Whether helper functions in the same production module are part of the registered operation.
    pub const fn scans_entire_repository(self) -> bool {
        matches!(
            self,
            Self::UpsertSpans
                | Self::UpsertMetrics
                | Self::UpsertLogs
                | Self::GetProjectStats
                | Self::EnforceRetention
                | Self::Search
        )
    }

    /// Public builder module that must own this operation's statement.
    pub const fn builder_module(self) -> &'static str {
        match self {
            Self::GetSpan
            | Self::ListSpans
            | Self::ListTraces
            | Self::GetTraceSessionPairs
            | Self::GetSessionIdsForTraces
            | Self::GetTraceIdsForSessions
            | Self::GetSpanCountsBulk
            | Self::TracesWithoutSpans
            | Self::FileReferenceFieldsForTraces
            | Self::SpanBodyFieldsForTraces
            | Self::SpanBodyBackfillPage
            | Self::CountProjectRows
            | Self::AnalyticsProjectIds
            | Self::CountSpansByProject
            | Self::GetSpansForTrace
            | Self::GetTrace
            | Self::GetTracesForSession
            | Self::GetSession
            | Self::ListSessions
            | Self::GetFeedSpans
            | Self::GetTraceFilterOptions
            | Self::GetTraceTagsOptions
            | Self::GetSpanFilterOptions
            | Self::GetSessionFilterOptions => "analytics::",
            Self::GetMessages | Self::GetProjectMessages => "messages::",
            Self::GetProjectStats => "stats::",
            Self::ListMetrics
            | Self::GetMetric
            | Self::AggregateMetrics
            | Self::GetMetricFilterOptions => "metric_sql::",
            Self::ListLogs | Self::GetLog | Self::GetLogFilterOptions => "log_sql::",
            Self::MaxIngestedAtUs => "analytics::",
            Self::EnforceRetention => "dml::",
            Self::DeleteSessions | Self::RawReconciliation => "dml::",
            Self::Search => "search_sql::",
            Self::DeleteTraces
            | Self::DeleteSpans
            | Self::DeleteProjectData
            | Self::UpsertSpans
            | Self::UpsertMetrics
            | Self::UpsertLogs => "dml::",
        }
    }
}

/// Registry used by the architectural gate.
///
/// Adding an operation here makes the gate inspect the same-named adapter functions and reject
/// SQL literals in them. The registry therefore tightens with the builder rather than relying on
/// a separately maintained grandfathering list.
pub const MIGRATED_OPERATIONS: &[QueryOperation] = &[
    QueryOperation::GetSpan,
    QueryOperation::ListSpans,
    QueryOperation::ListTraces,
    QueryOperation::DeleteTraces,
    QueryOperation::DeleteSpans,
    QueryOperation::DeleteProjectData,
    QueryOperation::UpsertSpans,
    QueryOperation::UpsertMetrics,
    QueryOperation::UpsertLogs,
    QueryOperation::ListMetrics,
    QueryOperation::GetMetric,
    QueryOperation::AggregateMetrics,
    QueryOperation::GetMetricFilterOptions,
    QueryOperation::ListLogs,
    QueryOperation::GetLog,
    QueryOperation::GetLogFilterOptions,
    QueryOperation::GetTraceSessionPairs,
    QueryOperation::GetSessionIdsForTraces,
    QueryOperation::GetTraceIdsForSessions,
    QueryOperation::GetSpanCountsBulk,
    QueryOperation::TracesWithoutSpans,
    QueryOperation::FileReferenceFieldsForTraces,
    QueryOperation::SpanBodyFieldsForTraces,
    QueryOperation::SpanBodyBackfillPage,
    QueryOperation::CountProjectRows,
    QueryOperation::AnalyticsProjectIds,
    QueryOperation::CountSpansByProject,
    QueryOperation::GetSpansForTrace,
    QueryOperation::GetTrace,
    QueryOperation::GetTracesForSession,
    QueryOperation::GetSession,
    QueryOperation::ListSessions,
    QueryOperation::GetFeedSpans,
    QueryOperation::GetTraceFilterOptions,
    QueryOperation::GetTraceTagsOptions,
    QueryOperation::GetSpanFilterOptions,
    QueryOperation::GetSessionFilterOptions,
    QueryOperation::GetMessages,
    QueryOperation::GetProjectMessages,
    QueryOperation::GetProjectStats,
    QueryOperation::MaxIngestedAtUs,
    QueryOperation::EnforceRetention,
    QueryOperation::DeleteSessions,
    QueryOperation::Search,
    QueryOperation::RawReconciliation,
];
