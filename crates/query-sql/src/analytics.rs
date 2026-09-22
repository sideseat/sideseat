//! Typed statements shared by the analytical adapters.
//!
//! The public constructors describe operations in storage-neutral terms. Rendering is the only
//! place where DuckDB's window deduplication and ClickHouse's `FINAL` relation are selected.

use crate::Backend;
use sideseat_core::core::constants::{QUERY_MAX_FILTER_SUGGESTIONS, QUERY_MAX_SPANS_PER_TRACE};
use sideseat_core::utils::sql::{escape_like_pattern, is_plain_identifier};
use sideseat_ports::filters::{
    BooleanOp, DatetimeOp, Filter, NullOp, NumberOp, OptionsOp, StringOp, columns,
};
use sideseat_ports::types::{
    FeedSpansParams, ListSessionsParams, ListSpansParams, ListTracesParams, OrderDirection,
    SESSION_FILTER_OPTION_COLUMNS, SPAN_FILTER_OPTION_COLUMNS, TRACE_FILTER_OPTION_COLUMNS,
};

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
    /// Count winning span identities for an explicit project set.
    CountSpansByProject,
    /// Read all winning spans for one trace.
    GetSpansForTrace,
    /// Expand events from one winning span row.
    GetEventsForSpan,
    /// Expand links from one winning span row.
    GetLinksForSpan,
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
            Self::CountSpansByProject => "count_spans_by_project",
            Self::GetSpansForTrace => "get_spans_for_trace",
            Self::GetEventsForSpan => "get_events_for_span",
            Self::GetLinksForSpan => "get_links_for_span",
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
        }
    }

    /// Adapter function that executes this operation.
    pub const fn adapter_function(self) -> &'static str {
        match self {
            Self::UpsertSpans | Self::UpsertMetrics | Self::UpsertLogs => "insert_batch",
            Self::EnforceRetention => "run_retention",
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
            | Self::CountSpansByProject
            | Self::GetSpansForTrace
            | Self::GetEventsForSpan
            | Self::GetLinksForSpan
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
            Self::DeleteSessions => "dml::",
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
    QueryOperation::CountSpansByProject,
    QueryOperation::GetSpansForTrace,
    QueryOperation::GetEventsForSpan,
    QueryOperation::GetLinksForSpan,
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
];

/// Driver-neutral parameter values. Adapters only translate these values into their driver's
/// binding API; they do not decide SQL shape or parameter order.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryValue {
    String(String),
    Int64(i64),
    Float64(f64),
}

/// The semantic value occupying a placeholder, in statement order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    ProjectId,
    TraceId,
    SpanId,
}

/// SQL lowered from a typed statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedQuery {
    sql: String,
    bindings: Vec<Binding>,
}

/// One executable statement with its ordered values.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterizedQuery {
    sql: String,
    params: Vec<QueryValue>,
}

impl ParameterizedQuery {
    pub(crate) fn new(sql: String, params: Vec<QueryValue>) -> Self {
        Self { sql, params }
    }

    pub fn sql(&self) -> &str {
        &self.sql
    }

    pub fn params(&self) -> &[QueryValue] {
        &self.params
    }
}

/// Count and row statements for an offset-paginated read.
#[derive(Debug, Clone, PartialEq)]
pub struct PageQuery {
    pub count: ParameterizedQuery,
    pub rows: ParameterizedQuery,
}

/// One validated filter-option column and its executable statement.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterOptionQuery {
    pub column: String,
    pub query: ParameterizedQuery,
}

/// Statements needed to verify that one project's analytical data is gone.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRowCountPlan {
    pub spans: ParameterizedQuery,
    pub metrics_table_exists: Option<ParameterizedQuery>,
    pub metrics: ParameterizedQuery,
    pub logs: ParameterizedQuery,
}

/// Per-signal logical-byte totals for one project.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectLogicalBytesPlan {
    pub spans: ParameterizedQuery,
    pub metrics: ParameterizedQuery,
    pub logs: ParameterizedQuery,
}

impl RenderedQuery {
    pub fn sql(&self) -> &str {
        &self.sql
    }

    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Projection {
    SpanDetail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relation {
    Spans,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Project,
    Trace,
    Span,
}

impl Column {
    const fn name(self) -> &'static str {
        match self {
            Self::Project => "project_id",
            Self::Trace => "trace_id",
            Self::Span => "span_id",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Equality {
    column: Column,
    binding: Binding,
}

/// A typed `SELECT` statement. Its fields are deliberately closed: callers choose an operation,
/// not fragments of SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectStatement {
    operation: QueryOperation,
    projection: Projection,
    relation: Relation,
    predicates: Vec<Equality>,
    limit: Option<u32>,
}

/// Fetch the winning delivery of a span identity.
pub fn span_by_id() -> SelectStatement {
    SelectStatement {
        operation: QueryOperation::GetSpan,
        projection: Projection::SpanDetail,
        relation: Relation::Spans,
        predicates: vec![
            Equality {
                column: Column::Project,
                binding: Binding::ProjectId,
            },
            Equality {
                column: Column::Trace,
                binding: Binding::TraceId,
            },
            Equality {
                column: Column::Span,
                binding: Binding::SpanId,
            },
        ],
        limit: Some(1),
    }
}

/// Build the filtered span page as one semantic operation.
pub fn list_spans(params: &ListSpansParams, backend: Backend) -> PageQuery {
    let dialect = analytics_dialect(backend);
    let mut conditions = vec!["project_id = ?".to_string()];
    let mut values = vec![QueryValue::String(params.project_id.to_string())];

    push_optional_eq(
        &mut conditions,
        &mut values,
        "trace_id",
        params.trace_id.as_deref(),
    );

    if let Some(session_id) = params.session_id.as_deref() {
        conditions.push(format!(
            "trace_id IN ({})",
            dialect.traces_of_session_relation()
        ));
        values.extend(dialect.traces_of_session_values(params.project_id.as_str(), session_id));
    }

    push_optional_eq(
        &mut conditions,
        &mut values,
        "user_id",
        params.user_id.as_deref(),
    );
    if let Some(environments) = &params.environment
        && !environments.is_empty()
    {
        conditions.push(format!(
            "environment IN ({})",
            placeholders(environments.len())
        ));
        values.extend(environments.iter().cloned().map(QueryValue::String));
    }
    push_optional_eq(
        &mut conditions,
        &mut values,
        "span_category",
        params.span_category.as_deref(),
    );
    push_optional_eq(
        &mut conditions,
        &mut values,
        "observation_type",
        params.observation_type.as_deref(),
    );
    push_optional_eq(
        &mut conditions,
        &mut values,
        "framework",
        params.framework.as_deref(),
    );
    push_optional_eq(
        &mut conditions,
        &mut values,
        "gen_ai_request_model",
        params.gen_ai_request_model.as_deref(),
    );
    push_optional_eq(
        &mut conditions,
        &mut values,
        "status_code",
        params.status_code.as_deref(),
    );

    if let Some(from) = &params.from_timestamp {
        conditions.push(dialect.timestamp_comparison("timestamp_start", ">="));
        values.push(dialect.timestamp_value(from));
    }
    if let Some(to) = &params.to_timestamp {
        conditions.push(dialect.timestamp_comparison("timestamp_start", "<="));
        values.push(dialect.timestamp_value(to));
    }
    if params.is_observation == Some(true) {
        conditions.push(format!("({})", crate::display::genai_span_predicate("")));
    }

    for filter in &params.filters {
        if filter.is_vacuous() {
            continue;
        }
        if filter.column() == "session_id" {
            let twin = filter.positive_twin();
            let rendered = twin.as_ref().unwrap_or(filter);
            let quantifier = if twin.is_some() { "NOT IN" } else { "IN" };
            let mut inner_values = Vec::new();
            let condition = dialect.render_filter_against(
                rendered,
                dialect.canonical_session_column(),
                &mut inner_values,
            );
            conditions.push(format!(
                "trace_id {quantifier} (SELECT cts.trace_id FROM ({}) cts \
                 WHERE cts.project_id = ? AND {condition})",
                dialect.canonical_trace_sessions_relation()
            ));
            values.push(QueryValue::String(params.project_id.to_string()));
            values.extend(inner_values);
            continue;
        }
        conditions.push(dialect.render_filter_column(
            filter,
            columns::map_span_column(filter.column()),
            "",
            &mut values,
        ));
    }

    let where_clause = conditions.join(" AND ");
    let source = dialect.span_page_relation();
    let order = span_order(params);
    let offset = params.page.saturating_sub(1) * params.limit;

    PageQuery {
        count: ParameterizedQuery {
            sql: dialect.span_count_sql(source, &where_clause),
            params: values.clone(),
        },
        rows: ParameterizedQuery {
            sql: format!(
                "SELECT\n{}\nFROM {source}\nWHERE {where_clause}\n\
                 ORDER BY {order}\nLIMIT {} OFFSET {offset}",
                dialect.span_detail_projection(),
                params.limit,
            ),
            params: values,
        },
    }
}

/// Build a stable cursor page over winning span deliveries.
///
/// The optional traversal watermark is part of the winner relation itself. Applying it after
/// deduplication would discard a post-watermark re-delivery without promoting the older delivery
/// that the traversal is supposed to see.
pub fn feed_spans(params: &FeedSpansParams, backend: Backend) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    let (source, mut values) = match (backend, params.ingested_before_us) {
        (Backend::Duckdb, Some(watermark)) => (
            "(SELECT * FROM otel_spans WHERE EPOCH_US(ingested_at) < ?::BIGINT \
             QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                        ORDER BY ingested_at DESC, rowid DESC) = 1)"
                .to_string(),
            vec![QueryValue::Int64(watermark)],
        ),
        (Backend::Duckdb, None) => (
            DuckdbAnalyticsDialect.span_page_relation().to_string(),
            Vec::new(),
        ),
        (Backend::Clickhouse, Some(watermark)) => (
            "(SELECT * FROM otel_spans \
              WHERE toInt64(toUnixTimestamp64Micro(ingested_at)) < ? \
              ORDER BY ingested_at DESC LIMIT 1 BY project_id, trace_id, span_id)"
                .to_string(),
            vec![QueryValue::Int64(watermark)],
        ),
        (Backend::Clickhouse, None) => (
            ClickhouseAnalyticsDialect.span_page_relation().to_string(),
            Vec::new(),
        ),
        (Backend::Sqlite | Backend::Postgres, _) => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    };
    let mut conditions = vec!["project_id = ?".to_string()];
    values.push(QueryValue::String(params.project_id.to_string()));

    if let Some((cursor_time_us, cursor_span_id, cursor_trace_id)) = &params.cursor {
        let ingested = match backend {
            Backend::Duckdb => "EPOCH_US(ingested_at)",
            Backend::Clickhouse => "toInt64(toUnixTimestamp64Micro(ingested_at))",
            Backend::Sqlite | Backend::Postgres => unreachable!(),
        };
        conditions.push(format!("({ingested}, span_id, trace_id) < (?, ?, ?)"));
        values.push(QueryValue::Int64(*cursor_time_us));
        values.push(QueryValue::String(cursor_span_id.clone()));
        values.push(QueryValue::String(cursor_trace_id.clone()));
    }
    if let Some(start) = &params.start_time {
        conditions.push(dialect.timestamp_comparison("timestamp_start", ">="));
        values.push(dialect.timestamp_value(start));
    }
    if let Some(end) = &params.end_time {
        conditions.push(dialect.timestamp_comparison("timestamp_start", "<"));
        values.push(dialect.timestamp_value(end));
    }
    if params.is_observation == Some(true) {
        conditions.push(format!("({})", crate::display::genai_span_predicate("")));
    }

    ParameterizedQuery {
        sql: format!(
            "SELECT\n{}\nFROM {source}\nWHERE {}\n\
             ORDER BY ingested_at DESC, span_id DESC, trace_id DESC\nLIMIT {}",
            dialect.span_detail_projection(),
            conditions.join(" AND "),
            params.limit,
        ),
        params: values,
    }
}

fn filter_option_scope(
    project_id: &str,
    from_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    alias: &str,
    backend: Backend,
) -> (String, Vec<QueryValue>) {
    let dialect = analytics_dialect(backend);
    let column = |name: &str| {
        if alias.is_empty() {
            name.to_string()
        } else {
            format!("{alias}.{name}")
        }
    };
    let mut conditions = vec![format!("{} = ?", column("project_id"))];
    let mut values = vec![QueryValue::String(project_id.to_string())];
    if let Some(from) = from_timestamp {
        conditions.push(dialect.timestamp_comparison(&column("timestamp_start"), ">="));
        values.push(dialect.timestamp_value(from));
    }
    if let Some(to) = to_timestamp {
        conditions.push(dialect.timestamp_comparison(&column("timestamp_start"), "<="));
        values.push(dialect.timestamp_value(to));
    }
    (conditions.join(" AND "), values)
}

fn filter_option_value(expression: &str, backend: Backend) -> String {
    match backend {
        Backend::Duckdb => expression.to_string(),
        Backend::Clickhouse => format!("toNullable({expression})"),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

/// Build validated trace filter-option statements.
pub fn trace_filter_options(
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    backend: Backend,
) -> Vec<FilterOptionQuery> {
    let dialect = analytics_dialect(backend);
    let source = dialect.span_page_relation();
    let (scope, values) =
        filter_option_scope(project_id, from_timestamp, to_timestamp, "s", backend);
    let canonical_name = dialect
        .canonical_session_column()
        .rsplit('.')
        .next()
        .expect("canonical session column");
    columns
        .iter()
        .filter_map(|column| {
            let span_column = TRACE_FILTER_OPTION_COLUMNS
                .iter()
                .find(|(view_column, _)| *view_column == column.as_str())
                .map(|(_, span_column)| *span_column)?;
            let sql = if column == "session_id" {
                let value = filter_option_value(&format!("cts.{canonical_name}"), backend);
                format!(
                    "SELECT {value} AS value, \
                            COUNT(DISTINCT cts.trace_id) AS count \
                     FROM ({}) cts \
                     WHERE (cts.project_id, cts.trace_id) IN (\
                       SELECT s.project_id, s.trace_id FROM {source} s WHERE {scope}\
                     ) \
                     GROUP BY cts.{canonical_name} \
                     ORDER BY count DESC LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}",
                    dialect.canonical_trace_sessions_relation(),
                )
            } else if column == "trace_name" {
                let flavor = match backend {
                    Backend::Duckdb => crate::display::DisplayNameDialect::DuckDb,
                    Backend::Clickhouse => crate::display::DisplayNameDialect::ClickHouse,
                    Backend::Sqlite | Backend::Postgres => unreachable!(),
                };
                let display_name = crate::display::trace_display_name("s", flavor);
                let value = filter_option_value(&display_name, backend);
                format!(
                    "SELECT value, COUNT(DISTINCT trace_id) AS count \
                     FROM (\
                       SELECT s.trace_id, {value} AS value \
                       FROM {source} s WHERE {scope} GROUP BY s.trace_id\
                     ) names \
                     WHERE value IS NOT NULL \
                     GROUP BY value ORDER BY count DESC \
                     LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}"
                )
            } else {
                let value = filter_option_value(&format!("s.{span_column}"), backend);
                format!(
                    "SELECT {value} AS value, \
                            COUNT(DISTINCT s.trace_id) AS count \
                     FROM {source} s \
                     WHERE {scope} AND s.{span_column} IS NOT NULL \
                     GROUP BY s.{span_column} \
                     ORDER BY count DESC LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}"
                )
            };
            Some(FilterOptionQuery {
                column: column.clone(),
                query: ParameterizedQuery {
                    sql,
                    params: values.clone(),
                },
            })
        })
        .collect()
}

/// Build the trace-tag option statement.
pub fn trace_tag_options(
    project_id: &str,
    from_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    backend: Backend,
) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    let source = dialect.span_page_relation();
    let (scope, values) =
        filter_option_scope(project_id, from_timestamp, to_timestamp, "s", backend);
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT tag AS value, COUNT(DISTINCT trace_id) AS count \
             FROM (\
               SELECT s.trace_id, \
                      UNNEST(from_json(s.tags, '[\"VARCHAR\"]')) AS tag \
               FROM {source} s \
               WHERE {scope} AND s.tags IS NOT NULL AND s.tags != '[]'\
             ) tags \
             WHERE tag IS NOT NULL AND tag != '' \
             GROUP BY tag ORDER BY count DESC \
             LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}"
        ),
        Backend::Clickhouse => format!(
            "SELECT toNullable(arrayJoin(JSONExtractArrayRaw(ifNull(s.tags, '[]')))) AS value, \
                    count(DISTINCT s.trace_id) AS count \
             FROM {source} s \
             WHERE {scope} AND s.tags IS NOT NULL AND s.tags != '[]' \
             GROUP BY value ORDER BY count DESC \
             LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}"
        ),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    ParameterizedQuery {
        sql,
        params: values,
    }
}

/// Build validated span filter-option statements over winning span identities.
pub fn span_filter_options(
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    observations_only: bool,
    backend: Backend,
) -> Vec<FilterOptionQuery> {
    let dialect = analytics_dialect(backend);
    let source = dialect.span_page_relation();
    let (mut scope, values) =
        filter_option_scope(project_id, from_timestamp, to_timestamp, "s", backend);
    if observations_only {
        scope.push_str(&format!(
            " AND ({})",
            crate::display::genai_span_predicate("s")
        ));
    }
    columns
        .iter()
        .filter(|column| SPAN_FILTER_OPTION_COLUMNS.contains(&column.as_str()))
        .map(|column| FilterOptionQuery {
            column: column.clone(),
            query: ParameterizedQuery {
                sql: {
                    let value = filter_option_value(&format!("s.{column}"), backend);
                    format!(
                        "SELECT {value} AS value, COUNT(*) AS count \
                         FROM {source} s \
                         WHERE {scope} AND s.{column} IS NOT NULL \
                         GROUP BY s.{column} \
                         ORDER BY count DESC LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}"
                    )
                },
                params: values.clone(),
            },
        })
        .collect()
}

/// Build validated session filter-option statements.
///
/// A value belongs to a session when any winning span of any canonically-owned trace carries it;
/// the naming span itself need not carry the value or even a non-null `session_id`.
pub fn session_filter_options(
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    backend: Backend,
) -> Vec<FilterOptionQuery> {
    let dialect = analytics_dialect(backend);
    let source = dialect.span_page_relation();
    let (scope, values) =
        filter_option_scope(project_id, from_timestamp, to_timestamp, "s", backend);
    let canonical_column = dialect.canonical_session_column();
    columns
        .iter()
        .filter(|column| SESSION_FILTER_OPTION_COLUMNS.contains(&column.as_str()))
        .map(|column| FilterOptionQuery {
            column: column.clone(),
            query: ParameterizedQuery {
                sql: {
                    let value = filter_option_value(&format!("s.{column}"), backend);
                    format!(
                        "SELECT {value} AS value, \
                            COUNT(DISTINCT {canonical_column}) AS count \
                     FROM {source} s \
                     JOIN ({}) cts \
                       ON cts.project_id = s.project_id AND cts.trace_id = s.trace_id \
                     WHERE {scope} AND s.{column} IS NOT NULL \
                     GROUP BY s.{column} \
                     ORDER BY count DESC LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}",
                        dialect.canonical_trace_sessions_relation(),
                    )
                },
                params: values.clone(),
            },
        })
        .collect()
}

/// Build the two-stage trace aggregate page.
pub fn list_traces(params: &ListTracesParams, backend: Backend) -> PageQuery {
    match backend {
        Backend::Duckdb => duckdb_trace_page(params),
        Backend::Clickhouse => clickhouse_trace_page(params),
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    }
}

/// Resolve each requested trace to the session on its earliest winning span.
pub fn trace_session_pairs(
    project_id: &str,
    trace_ids: &[String],
    as_of_us: Option<i64>,
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if trace_ids.is_empty() {
        return None;
    }
    let session = match backend {
        Backend::Duckdb => "arg_min(session_id, (timestamp_start, span_id))",
        Backend::Clickhouse => "argMin(assumeNotNull(session_id), (timestamp_start, span_id))",
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    if backend == Backend::Duckdb {
        let watermark = as_of_us
            .map(|_| "AND EPOCH_US(ingested_at) < ?::BIGINT ")
            .unwrap_or_default();
        let mut params = Vec::with_capacity(1 + usize::from(as_of_us.is_some()) + trace_ids.len());
        params.push(QueryValue::String(project_id.to_string()));
        params.extend(as_of_us.map(|us| QueryValue::String(us.to_string())));
        params.extend(trace_ids.iter().cloned().map(QueryValue::String));
        return Some(ParameterizedQuery {
            sql: format!(
                "SELECT trace_id, {session} AS session \
                 FROM (SELECT * FROM otel_spans \
                       WHERE project_id = ? {watermark}AND trace_id IN ({}) \
                       QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                                  ORDER BY ingested_at DESC, rowid DESC) = 1) \
                 WHERE session_id IS NOT NULL AND session_id != '' GROUP BY trace_id",
                placeholders(trace_ids.len())
            ),
            params,
        });
    }

    let (source, mut params) = membership_source(as_of_us, backend);
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(trace_ids.iter().cloned().map(QueryValue::String));
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT trace_id, {session} AS session \
             FROM {source} \
             WHERE project_id = ? AND trace_id IN ({}) \
             AND session_id IS NOT NULL AND session_id != '' GROUP BY trace_id",
            placeholders(trace_ids.len())
        ),
        params,
    })
}

/// Resolve requested traces to the distinct sessions on their earliest winning spans.
pub fn session_ids_for_traces(
    project_id: &str,
    trace_ids: &[String],
    as_of_us: Option<i64>,
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if trace_ids.is_empty() {
        return None;
    }
    let (source, mut params) = membership_source(as_of_us, backend);
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(trace_ids.iter().cloned().map(QueryValue::String));
    let session = match backend {
        Backend::Duckdb => "arg_min(session_id, (timestamp_start, span_id))",
        Backend::Clickhouse => "argMin(assumeNotNull(session_id), (timestamp_start, span_id))",
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT DISTINCT canonical_session FROM ( \
               SELECT {session} AS canonical_session \
               FROM {source} \
               WHERE project_id = ? AND trace_id IN ({}) \
               AND session_id IS NOT NULL AND session_id != '' GROUP BY trace_id \
             )",
            placeholders(trace_ids.len())
        ),
        params,
    })
}

/// Resolve requested sessions to the traces whose earliest winning span names one of them.
pub fn trace_ids_for_sessions(
    project_id: &str,
    session_ids: &[String],
    as_of_us: Option<i64>,
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if session_ids.is_empty() {
        return None;
    }
    let (source, mut params) = membership_source(as_of_us, backend);
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(session_ids.iter().cloned().map(QueryValue::String));
    let session = match backend {
        Backend::Duckdb => "arg_min(session_id, (timestamp_start, span_id))",
        Backend::Clickhouse => "argMin(assumeNotNull(session_id), (timestamp_start, span_id))",
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT trace_id FROM ( \
               SELECT trace_id, {session} AS canonical_session \
               FROM {source} \
               WHERE project_id = ? AND session_id IS NOT NULL AND session_id != '' \
               GROUP BY trace_id \
             ) WHERE canonical_session IN ({})",
            placeholders(session_ids.len())
        ),
        params,
    })
}

fn membership_source(as_of_us: Option<i64>, backend: Backend) -> (String, Vec<QueryValue>) {
    match (backend, as_of_us) {
        (Backend::Duckdb, Some(us)) => (
            "(SELECT * FROM otel_spans WHERE EPOCH_US(ingested_at) < ?::BIGINT \
             QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                        ORDER BY ingested_at DESC, rowid DESC) = 1)"
                .to_string(),
            vec![QueryValue::String(us.to_string())],
        ),
        (Backend::Duckdb, None) => (
            DuckdbAnalyticsDialect.span_page_relation().to_string(),
            Vec::new(),
        ),
        (Backend::Clickhouse, Some(us)) => (
            "(SELECT * FROM otel_spans \
              WHERE toInt64(toUnixTimestamp64Micro(ingested_at)) < ? \
              ORDER BY ingested_at DESC LIMIT 1 BY project_id, trace_id, span_id)"
                .to_string(),
            vec![QueryValue::Int64(us)],
        ),
        (Backend::Clickhouse, None) => ("otel_spans FINAL".to_string(), Vec::new()),
        (Backend::Sqlite | Backend::Postgres, _) => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    }
}

/// Read event and link counts from the winning row of each requested span identity.
pub fn span_counts_bulk(
    project_id: &str,
    spans: &[(String, String)],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if spans.is_empty() {
        return None;
    }
    let source = analytics_dialect(backend).span_page_relation();
    let (event_count, link_count) = match backend {
        Backend::Duckdb => (
            "COALESCE(json_array_length(raw_span->'events'), 0)",
            "COALESCE(json_array_length(raw_span->'links'), 0)",
        ),
        Backend::Clickhouse => (
            "coalesce(JSONLength(raw_span, 'events'), 0)",
            "coalesce(JSONLength(raw_span, 'links'), 0)",
        ),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    let identities = std::iter::repeat_n("(?, ?)", spans.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut params = Vec::with_capacity(1 + spans.len() * 2);
    params.push(QueryValue::String(project_id.to_string()));
    for (trace_id, span_id) in spans {
        params.push(QueryValue::String(trace_id.clone()));
        params.push(QueryValue::String(span_id.clone()));
    }
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT trace_id, span_id, {event_count} AS event_count, \
             {link_count} AS link_count \
             FROM {source} \
             WHERE project_id = ? AND (trace_id, span_id) IN ({identities})"
        ),
        params,
    })
}

/// Read the subset of requested trace ids that still have at least one winning span.
pub fn surviving_trace_ids(
    project_id: &str,
    trace_ids: &[String],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    trace_identity_read(
        project_id,
        trace_ids,
        backend,
        "DISTINCT trace_id",
        QueryOperation::TracesWithoutSpans,
    )
}

/// Read every field that can carry an external file reference for selected winning spans.
pub fn file_reference_fields(
    project_id: &str,
    trace_ids: &[String],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    let projection = match backend {
        Backend::Duckdb => "messages, tool_definitions, raw_span, metadata",
        Backend::Clickhouse => {
            "coalesce(messages, ''), coalesce(tool_definitions, ''), \
             coalesce(raw_span, ''), coalesce(metadata, '')"
        }
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    trace_identity_read(
        project_id,
        trace_ids,
        backend,
        projection,
        QueryOperation::FileReferenceFieldsForTraces,
    )
}

/// Read every body field with the span identity that owns it.
pub fn span_body_fields(
    project_id: &str,
    trace_ids: &[String],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    let projection = match backend {
        Backend::Duckdb => "trace_id, span_id, messages, tool_definitions, tool_names, raw_span",
        Backend::Clickhouse => {
            "trace_id, span_id, coalesce(messages, ''), coalesce(tool_definitions, ''), \
             coalesce(tool_names, ''), coalesce(raw_span, '')"
        }
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    trace_identity_read(
        project_id,
        trace_ids,
        backend,
        projection,
        QueryOperation::SpanBodyFieldsForTraces,
    )
}

pub fn span_body_backfill_page(
    project_id: &str,
    after: Option<(&str, &str)>,
    limit: usize,
    backend: Backend,
) -> ParameterizedQuery {
    let source = analytics_dialect(backend).span_page_relation();
    let projection = match backend {
        Backend::Duckdb => "trace_id, span_id, messages, tool_definitions, tool_names, raw_span",
        Backend::Clickhouse => {
            "trace_id, span_id, coalesce(messages, ''), coalesce(tool_definitions, ''), \
             coalesce(tool_names, ''), coalesce(raw_span, '')"
        }
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    let mut params = vec![QueryValue::String(project_id.to_string())];
    let cursor = if let Some((trace_id, span_id)) = after {
        params.extend([
            QueryValue::String(trace_id.to_string()),
            QueryValue::String(trace_id.to_string()),
            QueryValue::String(span_id.to_string()),
        ]);
        " AND (trace_id > ? OR (trace_id = ? AND span_id > ?))"
    } else {
        ""
    };
    params.push(QueryValue::Int64(i64::try_from(limit).unwrap_or(i64::MAX)));
    ParameterizedQuery {
        sql: format!(
            "SELECT {projection} FROM {source} WHERE project_id = ?{cursor} \
             ORDER BY trace_id, span_id LIMIT ?"
        ),
        params,
    }
}

fn trace_identity_read(
    project_id: &str,
    trace_ids: &[String],
    backend: Backend,
    projection: &str,
    _operation: QueryOperation,
) -> Option<ParameterizedQuery> {
    if trace_ids.is_empty() {
        return None;
    }
    let source = analytics_dialect(backend).span_page_relation();
    let mut params = Vec::with_capacity(1 + trace_ids.len());
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(trace_ids.iter().cloned().map(QueryValue::String));
    Some(ParameterizedQuery {
        sql: format!(
            "SELECT {projection} FROM {source} \
             WHERE project_id = ? AND trace_id IN ({})",
            placeholders(trace_ids.len())
        ),
        params,
    })
}

/// Build exact span/metric row counts for one project.
pub fn project_row_count(
    project_id: &str,
    backend: Backend,
    metrics_table: Option<&str>,
) -> ProjectRowCountPlan {
    let project_params = || vec![QueryValue::String(project_id.to_string())];
    match backend {
        Backend::Duckdb => ProjectRowCountPlan {
            spans: ParameterizedQuery {
                sql: "SELECT COUNT(*) FROM otel_spans WHERE project_id = ?".to_string(),
                params: project_params(),
            },
            metrics_table_exists: None,
            metrics: ParameterizedQuery {
                sql: "SELECT COUNT(*) FROM otel_metrics WHERE project_id = ?".to_string(),
                params: project_params(),
            },
            logs: ParameterizedQuery {
                sql: "SELECT COUNT(*) FROM otel_logs WHERE project_id = ?".to_string(),
                params: project_params(),
            },
        },
        Backend::Clickhouse => {
            let metrics_table = metrics_table
                .expect("ClickHouse project row count needs its configured metric table");
            assert!(
                is_plain_identifier(metrics_table),
                "invalid metrics table: {metrics_table:?}"
            );
            ProjectRowCountPlan {
                spans: ParameterizedQuery {
                    sql: "SELECT count() FROM otel_spans FINAL WHERE project_id = ?".to_string(),
                    params: project_params(),
                },
                metrics_table_exists: Some(ParameterizedQuery {
                    sql: "SELECT count() FROM system.tables \
                          WHERE database = currentDatabase() AND name = ?"
                        .to_string(),
                    params: vec![QueryValue::String(metrics_table.to_string())],
                }),
                metrics: ParameterizedQuery {
                    sql: "SELECT count() FROM otel_metrics FINAL WHERE project_id = ?".to_string(),
                    params: project_params(),
                },
                logs: ParameterizedQuery {
                    sql: "SELECT count() FROM otel_logs FINAL WHERE project_id = ?".to_string(),
                    params: project_params(),
                },
            }
        }
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    }
}

/// Build attributable logical-byte sums, optionally restricted to rows under an active hold.
pub fn project_logical_bytes(
    project_id: &str,
    backend: Backend,
    held_at: Option<chrono::DateTime<chrono::Utc>>,
) -> ProjectLogicalBytesPlan {
    let predicate = if held_at.is_some() {
        "project_id = ? AND hold_until IS NOT NULL AND hold_until >= ?"
    } else {
        "project_id = ?"
    };
    let params = || {
        let mut values = vec![QueryValue::String(project_id.to_string())];
        if let Some(held_at) = held_at {
            values.push(match backend {
                Backend::Duckdb => QueryValue::String(held_at.to_rfc3339()),
                Backend::Clickhouse => QueryValue::Int64(held_at.timestamp_micros()),
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            });
        }
        values
    };
    let sum = |relation: &str| ParameterizedQuery {
        sql: match backend {
            Backend::Duckdb => format!(
                "SELECT COALESCE(SUM(logical_bytes), 0)::UBIGINT FROM {relation} WHERE {predicate}"
            ),
            Backend::Clickhouse => format!(
                "SELECT toUInt64(sum(logical_bytes)) FROM {relation} FINAL WHERE {predicate}"
            ),
            Backend::Sqlite | Backend::Postgres => unreachable!(),
        },
        params: params(),
    };
    let spans = match backend {
        Backend::Duckdb => sum("(SELECT * FROM otel_spans QUALIFY ROW_NUMBER() OVER (\
             PARTITION BY project_id, trace_id, span_id \
             ORDER BY ingested_at DESC, rowid DESC) = 1)"),
        Backend::Clickhouse => sum("otel_spans"),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    ProjectLogicalBytesPlan {
        spans,
        metrics: sum("otel_metrics"),
        logs: sum("otel_logs"),
    }
}

/// Select a bounded oldest-first pressure batch from the winning, non-held span relation.
pub fn oldest_reclaimable_spans(
    project_id: &str,
    backend: Backend,
    target_bytes: u64,
    now: chrono::DateTime<chrono::Utc>,
    limit: usize,
) -> ParameterizedQuery {
    let relation = match backend {
        Backend::Duckdb => {
            "(SELECT * FROM otel_spans QUALIFY ROW_NUMBER() OVER (\
             PARTITION BY project_id, trace_id, span_id \
             ORDER BY ingested_at DESC, rowid DESC) = 1)"
        }
        Backend::Clickhouse => "otel_spans FINAL",
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    };
    let now = match backend {
        Backend::Duckdb => QueryValue::String(now.to_rfc3339()),
        Backend::Clickhouse => QueryValue::Int64(now.timestamp_micros()),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    ParameterizedQuery::new(
        format!(
            "SELECT trace_id, span_id, logical_bytes FROM (\
                 SELECT trace_id, span_id, logical_bytes, \
                        ROW_NUMBER() OVER (\
                            ORDER BY timestamp_start ASC, trace_id ASC, span_id ASC\
                        ) AS pressure_rank, \
                        COALESCE(SUM(logical_bytes) OVER (\
                            ORDER BY timestamp_start ASC, trace_id ASC, span_id ASC \
                            ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING\
                        ), 0) AS bytes_before \
                 FROM {relation} \
                 WHERE project_id = ? AND (hold_until IS NULL OR hold_until < ?)\
             ) WHERE pressure_rank <= ? AND bytes_before < ? \
             ORDER BY pressure_rank"
        ),
        vec![
            QueryValue::String(project_id.to_string()),
            now,
            QueryValue::Int64(i64::try_from(limit).unwrap_or(i64::MAX)),
            QueryValue::Int64(i64::try_from(target_bytes).unwrap_or(i64::MAX)),
        ],
    )
}

/// The newest committed ingestion timestamp for one project.
///
/// This intentionally reads the raw relation: a later re-delivery is itself a commit that must advance the
/// traversal watermark, whether or not it replaces an earlier logical span.
pub fn max_ingested_at_us(project_id: &str, backend: Backend) -> ParameterizedQuery {
    let expression = match backend {
        Backend::Duckdb => "MAX(EPOCH_US(ingested_at))",
        Backend::Clickhouse => "max(toInt64(toUnixTimestamp64Micro(ingested_at)))",
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    };
    ParameterizedQuery::new(
        format!(
            "SELECT {expression} AS max_ingested_at_us \
             FROM otel_spans WHERE project_id = ?"
        ),
        vec![QueryValue::String(project_id.to_string())],
    )
}

/// Count winning span identities for an explicit maintenance project set.
pub fn span_counts_by_project(
    project_ids: &[String],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if project_ids.is_empty() {
        return None;
    }
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT project_id, COUNT(*) AS cnt FROM (\
             SELECT DISTINCT project_id, trace_id, span_id FROM otel_spans \
             WHERE project_id IN ({})) GROUP BY project_id",
            placeholders(project_ids.len())
        ),
        Backend::Clickhouse => format!(
            "SELECT project_id, count() AS cnt FROM otel_spans FINAL \
             WHERE project_id IN ({}) GROUP BY project_id",
            placeholders(project_ids.len())
        ),
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    };
    Some(ParameterizedQuery {
        sql,
        params: project_ids
            .iter()
            .cloned()
            .map(QueryValue::String)
            .collect(),
    })
}

/// Read all winning spans of one trace in event-time order.
pub fn spans_for_trace(project_id: &str, trace_id: &str, backend: Backend) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    ParameterizedQuery {
        sql: format!(
            "SELECT\n{}\nFROM {}\nWHERE project_id = ? AND trace_id = ? \
             ORDER BY timestamp_start LIMIT {QUERY_MAX_SPANS_PER_TRACE}",
            dialect.span_detail_projection(),
            dialect.span_page_relation(),
        ),
        params: vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(trace_id.to_string()),
        ],
    }
}

/// Expand the ordered events from one winning span row.
pub fn events_for_span(
    project_id: &str,
    trace_id: &str,
    span_id: &str,
    backend: Backend,
) -> ParameterizedQuery {
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT _s.span_id, (ordinality - 1)::INTEGER AS event_index, \
                    event->>'timestamp' AS event_timestamp, \
                    event->>'name' AS event_name, \
                    (event->'attributes')::VARCHAR AS attributes \
             FROM (SELECT span_id, raw_span FROM otel_spans \
                   WHERE project_id = ? AND trace_id = ? AND span_id = ? \
                   ORDER BY ingested_at DESC, rowid DESC LIMIT 1) _s, \
                  UNNEST(CAST(_s.raw_span->'events' AS JSON[])) \
                  WITH ORDINALITY AS t(event, ordinality) \
             ORDER BY ordinality LIMIT {QUERY_MAX_SPANS_PER_TRACE}"
        ),
        Backend::Clickhouse => format!(
            "SELECT span_id, \
                    toInt32(arrayJoin(range(JSONLength(raw_span, 'events')))) AS event_index, \
                    ifNull(JSONExtractString(JSONExtractRaw(raw_span, 'events', \
                      arrayJoin(range(JSONLength(raw_span, 'events'))) + 1), 'timestamp'), '') \
                      AS event_timestamp, \
                    JSONExtractString(JSONExtractRaw(raw_span, 'events', \
                      arrayJoin(range(JSONLength(raw_span, 'events'))) + 1), 'name') AS event_name, \
                    JSONExtractRaw(JSONExtractRaw(raw_span, 'events', \
                      arrayJoin(range(JSONLength(raw_span, 'events'))) + 1), 'attributes') AS attributes \
             FROM otel_spans FINAL \
             WHERE project_id = ? AND trace_id = ? AND span_id = ? \
               AND JSONLength(raw_span, 'events') > 0 \
             ORDER BY event_index LIMIT {QUERY_MAX_SPANS_PER_TRACE}"
        ),
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    };
    point_span_json_query(sql, project_id, trace_id, span_id)
}

/// Expand the ordered links from one winning span row.
pub fn links_for_span(
    project_id: &str,
    trace_id: &str,
    span_id: &str,
    backend: Backend,
) -> ParameterizedQuery {
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT _s.span_id, link->>'trace_id' AS linked_trace_id, \
                    link->>'span_id' AS linked_span_id, \
                    (link->'attributes')::VARCHAR AS attributes \
             FROM (SELECT span_id, raw_span FROM otel_spans \
                   WHERE project_id = ? AND trace_id = ? AND span_id = ? \
                   ORDER BY ingested_at DESC, rowid DESC LIMIT 1) _s, \
                  UNNEST(CAST(_s.raw_span->'links' AS JSON[])) \
                  WITH ORDINALITY AS t(link, ordinality) \
             ORDER BY ordinality LIMIT {QUERY_MAX_SPANS_PER_TRACE}"
        ),
        Backend::Clickhouse => format!(
            "SELECT span_id, \
                    ifNull(JSONExtractString(JSONExtractRaw(raw_span, 'links', \
                      arrayJoin(range(JSONLength(raw_span, 'links'))) + 1), 'trace_id'), '') \
                      AS linked_trace_id, \
                    ifNull(JSONExtractString(JSONExtractRaw(raw_span, 'links', \
                      arrayJoin(range(JSONLength(raw_span, 'links'))) + 1), 'span_id'), '') \
                      AS linked_span_id, \
                    JSONExtractRaw(JSONExtractRaw(raw_span, 'links', \
                      arrayJoin(range(JSONLength(raw_span, 'links'))) + 1), 'attributes') AS attributes \
             FROM otel_spans FINAL \
             WHERE project_id = ? AND trace_id = ? AND span_id = ? \
               AND JSONLength(raw_span, 'links') > 0 \
             LIMIT {QUERY_MAX_SPANS_PER_TRACE}"
        ),
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    };
    point_span_json_query(sql, project_id, trace_id, span_id)
}

fn point_span_json_query(
    sql: String,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> ParameterizedQuery {
    ParameterizedQuery {
        sql,
        params: vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(trace_id.to_string()),
            QueryValue::String(span_id.to_string()),
        ],
    }
}

/// Aggregate one trace using the same projection and token-dedup rules as the trace list.
pub fn trace_by_id(project_id: &str, trace_id: &str, backend: Backend) -> ParameterizedQuery {
    let source = analytics_dialect(backend).span_page_relation();
    let sql = match backend {
        Backend::Duckdb => format!(
            "WITH gen_totals AS ({}), \
             target AS (\
               SELECT project_id, trace_id FROM {source} \
               WHERE project_id = ? AND trace_id = ? \
               GROUP BY project_id, trace_id\
             ) \
             SELECT {} \
             FROM target t \
             JOIN {source} s ON t.project_id = s.project_id AND t.trace_id = s.trace_id \
             LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id \
             GROUP BY t.trace_id",
            duckdb_gen_totals_sql("g.project_id = ? AND g.trace_id = ?"),
            duckdb_trace_projection(),
        ),
        Backend::Clickhouse => format!(
            "WITH {}, {}, \
             target AS (\
               SELECT project_id, trace_id FROM otel_spans FINAL \
               WHERE project_id = ? AND trace_id = ? \
               GROUP BY project_id, trace_id\
             ) \
             SELECT {} \
             FROM target t \
             JOIN otel_spans s FINAL \
               ON t.project_id = s.project_id AND t.trace_id = s.trace_id \
             LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id \
             GROUP BY t.trace_id",
            clickhouse_dedup_lookup("trace_id = ?"),
            clickhouse_gen_totals_cte(Some("g.trace_id"), "g.project_id = ? AND g.trace_id = ?"),
            clickhouse_trace_projection(),
        ),
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    };
    let pairs = match backend {
        Backend::Duckdb => 2,
        Backend::Clickhouse => 3,
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    let mut params = Vec::with_capacity(pairs * 2);
    for _ in 0..pairs {
        params.push(QueryValue::String(project_id.to_string()));
        params.push(QueryValue::String(trace_id.to_string()));
    }
    ParameterizedQuery { sql, params }
}

/// Aggregate all traces whose canonical earliest-span session matches the requested session.
pub fn traces_for_session(
    project_id: &str,
    session_id: &str,
    backend: Backend,
) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    let session_relation = dialect.traces_of_session_relation();
    let source = dialect.span_page_relation();
    let sql = match backend {
        Backend::Duckdb => format!(
            "WITH session_traces AS ({session_relation}), \
             gen_totals AS ({}), \
             target AS (\
               SELECT s.project_id, s.trace_id FROM {source} s \
               WHERE s.project_id = ? \
                 AND s.trace_id IN (SELECT trace_id FROM session_traces) \
               GROUP BY s.project_id, s.trace_id\
             ) \
             SELECT {} \
             FROM target t \
             JOIN {source} s ON t.project_id = s.project_id AND t.trace_id = s.trace_id \
             LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id \
             GROUP BY t.trace_id \
             ORDER BY MIN(s.timestamp_start) DESC",
            duckdb_gen_totals_sql(
                "g.project_id = ? AND g.trace_id IN (SELECT trace_id FROM session_traces)"
            ),
            duckdb_trace_projection(),
        ),
        Backend::Clickhouse => format!(
            "WITH session_traces AS ({session_relation}), \
             {}, {}, \
             target AS (\
               SELECT s.project_id, s.trace_id FROM otel_spans s FINAL \
               WHERE s.project_id = ? \
                 AND s.trace_id IN (SELECT trace_id FROM session_traces) \
               GROUP BY s.project_id, s.trace_id\
             ) \
             SELECT {} \
             FROM target t \
             JOIN otel_spans s FINAL \
               ON t.project_id = s.project_id AND t.trace_id = s.trace_id \
             LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id \
             GROUP BY t.trace_id \
             ORDER BY min(s.timestamp_start) DESC",
            clickhouse_dedup_lookup("trace_id IN (SELECT trace_id FROM session_traces)"),
            clickhouse_gen_totals_cte(
                Some("g.trace_id"),
                "g.project_id = ? AND g.trace_id IN (SELECT trace_id FROM session_traces)"
            ),
            clickhouse_trace_projection(),
        ),
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    };
    let mut params = dialect.traces_of_session_values(project_id, session_id);
    if backend == Backend::Clickhouse {
        params.push(QueryValue::String(project_id.to_string()));
    }
    params.push(QueryValue::String(project_id.to_string()));
    params.push(QueryValue::String(project_id.to_string()));
    ParameterizedQuery { sql, params }
}

/// Aggregate one session using canonical trace membership and shared token-dedup rules.
pub fn session_by_id(project_id: &str, session_id: &str, backend: Backend) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    let session_relation = dialect.traces_of_session_relation();
    let source = dialect.span_page_relation();
    let totals_columns = [
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "reasoning_tokens",
        "input_cost",
        "output_cost",
        "cache_read_cost",
        "cache_write_cost",
        "reasoning_cost",
        "total_cost",
    ];
    let session_totals = totals_columns
        .iter()
        .map(|column| format!("COALESCE(SUM({column}), 0) AS {column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let projected_totals = totals_columns
        .iter()
        .map(|column| {
            let cast = if backend == Backend::Duckdb && column.ends_with("_cost") {
                "::DOUBLE"
            } else {
                ""
            };
            format!("COALESCE(MAX(gt.{column}), 0){cast} AS {column}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let (session_expr, user_expr, environment_expr, start_expr, end_expr, observation_count) =
        match backend {
            Backend::Duckdb => (
                "?",
                "FIRST(s.user_id ORDER BY s.timestamp_start, s.span_id) \
                 FILTER (WHERE s.user_id IS NOT NULL)",
                "FIRST(s.environment ORDER BY s.timestamp_start, s.span_id) \
                 FILTER (WHERE s.environment IS NOT NULL)",
                "EPOCH_US(MIN(s.timestamp_start))",
                "EPOCH_US(MAX(COALESCE(s.timestamp_end, s.timestamp_start)))",
                "COUNT(*) FILTER (WHERE s.observation_type != 'span')",
            ),
            Backend::Clickhouse => (
                "toNullable(?)",
                "argMinIf(s.user_id, (s.timestamp_start, s.span_id), s.user_id IS NOT NULL)",
                "argMinIf(s.environment, (s.timestamp_start, s.span_id), \
                 s.environment IS NOT NULL)",
                "toInt64(toUnixTimestamp64Micro(min(s.timestamp_start)))",
                "toInt64(toUnixTimestamp64Micro(\
                 max(coalesce(s.timestamp_end, s.timestamp_start))))",
                "countIf(s.observation_type != 'span')",
            ),
            Backend::Sqlite | Backend::Postgres => unreachable!(),
        };
    let prefix = match backend {
        Backend::Duckdb => format!(
            "WITH session_traces AS ({session_relation}), \
             gen_totals_by_trace AS ({}),",
            duckdb_gen_totals_sql(
                "g.project_id = ? AND g.trace_id IN (SELECT trace_id FROM session_traces)"
            )
        ),
        Backend::Clickhouse => {
            let totals = clickhouse_gen_totals_cte(
                Some("g.trace_id"),
                "g.project_id = ? AND g.trace_id IN (SELECT trace_id FROM session_traces)",
            )
            .replacen("gen_totals AS", "gen_totals_by_trace AS", 1);
            format!(
                "WITH session_traces AS ({session_relation}), {}, {totals},",
                clickhouse_dedup_lookup("trace_id IN (SELECT trace_id FROM session_traces)"),
            )
        }
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    let sql = format!(
        "{prefix} \
         session_totals AS (SELECT {session_totals} FROM gen_totals_by_trace) \
         SELECT {session_expr} AS session_id, \
                {user_expr} AS user_id, \
                {environment_expr} AS environment, \
                {start_expr} AS start_time, \
                {end_expr} AS end_time, \
                COUNT(DISTINCT s.trace_id) AS trace_count, \
                COUNT(*) AS span_count, \
                {observation_count} AS observation_count, \
                {projected_totals} \
         FROM {source} s CROSS JOIN session_totals gt \
         WHERE s.project_id = ? \
           AND s.trace_id IN (SELECT trace_id FROM session_traces)"
    );
    let mut params = dialect.traces_of_session_values(project_id, session_id);
    if backend == Backend::Clickhouse {
        params.push(QueryValue::String(project_id.to_string()));
    }
    params.push(QueryValue::String(project_id.to_string()));
    params.push(QueryValue::String(session_id.to_string()));
    params.push(QueryValue::String(project_id.to_string()));
    ParameterizedQuery { sql, params }
}

/// List canonical sessions while keeping selection separate from membership.
///
/// A filter selects a session when any span of any trace canonically assigned to that session
/// satisfies it. Once selected, the row aggregates every trace in the session, not only the traces
/// or spans that matched the filter.
pub fn list_sessions(params: &ListSessionsParams, backend: Backend) -> PageQuery {
    match backend {
        Backend::Duckdb => duckdb_session_page(params),
        Backend::Clickhouse => clickhouse_session_page(params),
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    }
}

fn session_scope_condition(
    session_column: &str,
    project_id: &str,
    inner: &str,
    inner_values: Vec<QueryValue>,
    negated: bool,
    backend: Backend,
) -> (String, Vec<QueryValue>) {
    let dialect = analytics_dialect(backend);
    let quantifier = if negated { "NOT IN" } else { "IN" };
    let mut values = vec![
        QueryValue::String(project_id.to_string()),
        QueryValue::String(project_id.to_string()),
    ];
    values.extend(inner_values);
    (
        format!(
            "{session_column} {quantifier} (\
             SELECT {} FROM ({}) cts \
             WHERE cts.project_id = ? \
               AND cts.trace_id IN (\
                 SELECT n.trace_id FROM {} n \
                 WHERE n.project_id = ? AND {inner}\
               )\
             )",
            dialect.canonical_session_column(),
            dialect.canonical_trace_sessions_relation(),
            dialect.span_page_relation(),
        ),
        values,
    )
}

fn session_conditions(
    params: &ListSessionsParams,
    alias: &str,
    backend: Backend,
) -> (String, Vec<QueryValue>) {
    let dialect = analytics_dialect(backend);
    let column = |name: &str| {
        if alias.is_empty() {
            name.to_string()
        } else {
            format!("{alias}.{name}")
        }
    };
    let mut conditions = vec![
        format!("{} = ?", column("project_id")),
        format!("{} IS NOT NULL", column("session_id")),
    ];
    let mut values = vec![QueryValue::String(params.project_id.to_string())];

    if let Some(user_id) = params.user_id.as_deref() {
        let (condition, scoped_values) = session_scope_condition(
            &column("session_id"),
            params.project_id.as_str(),
            "n.user_id = ?",
            vec![QueryValue::String(user_id.to_string())],
            false,
            backend,
        );
        conditions.push(condition);
        values.extend(scoped_values);
    }
    if let Some(environments) = &params.environment
        && !environments.is_empty()
    {
        let (condition, scoped_values) = session_scope_condition(
            &column("session_id"),
            params.project_id.as_str(),
            &format!("n.environment IN ({})", placeholders(environments.len())),
            environments
                .iter()
                .cloned()
                .map(QueryValue::String)
                .collect(),
            false,
            backend,
        );
        conditions.push(condition);
        values.extend(scoped_values);
    }
    if let Some(from) = &params.from_timestamp {
        conditions.push(dialect.timestamp_comparison(&column("timestamp_start"), ">="));
        values.push(dialect.timestamp_value(from));
    }
    if let Some(to) = &params.to_timestamp {
        conditions.push(dialect.timestamp_comparison(&column("timestamp_start"), "<="));
        values.push(dialect.timestamp_value(to));
    }

    for filter in &params.filters {
        if filter.is_vacuous() {
            continue;
        }
        if filter.column() == "session_id" {
            let twin = filter.positive_twin();
            let rendered = twin.as_ref().unwrap_or(filter);
            let quantifier = if twin.is_some() { "NOT IN" } else { "IN" };
            let mut filter_values = Vec::new();
            let condition = dialect.render_filter_against(
                rendered,
                dialect.canonical_session_column(),
                &mut filter_values,
            );
            conditions.push(format!(
                "{} {quantifier} (\
                 SELECT cts.trace_id FROM ({}) cts \
                 WHERE cts.project_id = ? AND {condition}\
                 )",
                column("trace_id"),
                dialect.canonical_trace_sessions_relation(),
            ));
            values.push(QueryValue::String(params.project_id.to_string()));
            values.extend(filter_values);
            continue;
        }

        let twin = filter.positive_twin();
        let rendered = twin.as_ref().unwrap_or(filter);
        let mut filter_values = Vec::new();
        let inner = dialect.render_filter_column(
            rendered,
            columns::map_session_column_to_spans(rendered.column()),
            "n",
            &mut filter_values,
        );
        let (condition, scoped_values) = session_scope_condition(
            &column("session_id"),
            params.project_id.as_str(),
            &inner,
            filter_values,
            twin.is_some(),
            backend,
        );
        conditions.push(condition);
        values.extend(scoped_values);
    }

    (conditions.join(" AND "), values)
}

fn session_sort(params: &ListSessionsParams) -> (&'static str, &'static str) {
    let direction = params
        .order_by
        .as_ref()
        .map(|order| match order.direction {
            OrderDirection::Asc => "ASC",
            OrderDirection::Desc => "DESC",
        })
        .unwrap_or("DESC");
    let column = params
        .order_by
        .as_ref()
        .map(|order| order.column.as_str())
        .unwrap_or("timestamp_start");
    let field = match column {
        "start_time" => "min_ts",
        "end_time" => "max_ts",
        "total_cost" => "total_cost",
        "trace_count" => "trace_count",
        "span_count" => "span_count",
        "observation_count" => "observation_count",
        _ => "min_ts",
    };
    (field, direction)
}

fn canonical_session_relation(backend: Backend) -> String {
    let dialect = analytics_dialect(backend);
    let canonical_name = dialect
        .canonical_session_column()
        .rsplit('.')
        .next()
        .expect("canonical session column");
    format!(
        "SELECT canonical.project_id, canonical.trace_id, \
         canonical.{canonical_name} AS session_id \
         FROM ({}) canonical",
        dialect.canonical_trace_sessions_relation()
    )
}

fn session_count_query(params: &ListSessionsParams, backend: Backend) -> ParameterizedQuery {
    let dialect = analytics_dialect(backend);
    let canonical_name = dialect
        .canonical_session_column()
        .rsplit('.')
        .next()
        .expect("canonical session column");
    let (where_clause, values) = session_conditions(params, "", backend);
    let count = match backend {
        Backend::Duckdb => "COUNT",
        Backend::Clickhouse => "count",
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    ParameterizedQuery {
        sql: format!(
            "SELECT {count}(DISTINCT ts.{canonical_name}) AS cnt \
             FROM ({}) ts \
             WHERE (ts.project_id, ts.trace_id) IN (\
               SELECT project_id, trace_id FROM {} WHERE {where_clause}\
             )",
            dialect.canonical_trace_sessions_relation(),
            dialect.span_page_relation(),
        ),
        params: values,
    }
}

fn session_projection(backend: Backend) -> String {
    let totals = [
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "reasoning_tokens",
        "input_cost",
        "output_cost",
        "cache_read_cost",
        "cache_write_cost",
        "reasoning_cost",
        "total_cost",
    ]
    .iter()
    .map(|column| {
        let cast = if backend == Backend::Duckdb && column.ends_with("_cost") {
            "::DOUBLE"
        } else {
            ""
        };
        format!("    COALESCE(MAX(gt2.{column}), 0){cast} AS {column}")
    })
    .collect::<Vec<_>>()
    .join(",\n");
    match backend {
        Backend::Duckdb => format!(
            r#"f.session_id,
    FIRST(s.user_id ORDER BY s.timestamp_start, s.span_id)
        FILTER (WHERE s.user_id IS NOT NULL) AS user_id,
    FIRST(s.environment ORDER BY s.timestamp_start, s.span_id)
        FILTER (WHERE s.environment IS NOT NULL) AS environment,
    EPOCH_US(MIN(s.timestamp_start)) AS start_time,
    EPOCH_US(MAX(COALESCE(s.timestamp_end, s.timestamp_start))) AS end_time,
    COUNT(DISTINCT s.trace_id) AS trace_count,
    COUNT(*) AS span_count,
    COUNT(*) FILTER (WHERE s.observation_type != 'span') AS observation_count,
{totals}"#
        ),
        Backend::Clickhouse => format!(
            r#"toNullable(f.session_id) AS session_id,
    argMinIf(s.user_id, (s.timestamp_start, s.span_id),
             s.user_id IS NOT NULL) AS user_id,
    argMinIf(s.environment, (s.timestamp_start, s.span_id),
             s.environment IS NOT NULL) AS environment,
    toInt64(toUnixTimestamp64Micro(min(s.timestamp_start))) AS start_time,
    toInt64(toUnixTimestamp64Micro(
        max(coalesce(s.timestamp_end, s.timestamp_start)))) AS end_time,
    count(DISTINCT s.trace_id) AS trace_count,
    count() AS span_count,
    countIf(s.observation_type != 'span') AS observation_count,
{totals}"#
        ),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

fn duckdb_session_page(params: &ListSessionsParams) -> PageQuery {
    let source = DuckdbAnalyticsDialect.span_page_relation();
    let trace_sessions = canonical_session_relation(Backend::Duckdb);
    let (where_clause, values) = session_conditions(params, "sp", Backend::Duckdb);
    let (sort_field, sort_direction) = session_sort(params);
    let offset = params.page.saturating_sub(1) * params.limit;
    let gen_totals = duckdb_gen_totals_joined_sql(
        "stg.session_id",
        "JOIN session_traces stg \
         ON stg.project_id = g.project_id AND stg.trace_id = g.trace_id",
        "1 = 1",
    );
    let sql = format!(
        r#"WITH trace_sessions AS ({trace_sessions}),
matching_sessions AS (
    SELECT DISTINCT ts.project_id, ts.session_id
    FROM {source} sp
    JOIN trace_sessions ts
      ON ts.project_id = sp.project_id AND ts.trace_id = sp.trace_id
    WHERE {where_clause}
),
session_traces AS (
    SELECT ts.project_id, ts.session_id, ts.trace_id
    FROM trace_sessions ts
    JOIN matching_sessions ms
      ON ms.project_id = ts.project_id AND ms.session_id = ts.session_id
),
gen_totals AS (
    {gen_totals}
),
filtered_sessions AS (
    SELECT
        st.project_id,
        st.session_id,
        MIN(sp.timestamp_start) AS min_ts,
        MAX(COALESCE(sp.timestamp_end, sp.timestamp_start)) AS max_ts,
        COALESCE(MAX(gt.total_cost), 0)::DOUBLE AS total_cost,
        COUNT(DISTINCT sp.trace_id) AS trace_count,
        COUNT(*) AS span_count,
        COUNT(*) FILTER (WHERE sp.observation_type != 'span') AS observation_count
    FROM session_traces st
    JOIN {source} sp
      ON sp.project_id = st.project_id AND sp.trace_id = st.trace_id
    LEFT JOIN gen_totals gt ON gt.session_id = st.session_id
    GROUP BY st.project_id, st.session_id
    ORDER BY {sort_field} {sort_direction}, min_ts {sort_direction}, st.session_id ASC
    LIMIT {limit} OFFSET {offset}
)
SELECT
{projection}
FROM filtered_sessions f
JOIN session_traces st
  ON st.project_id = f.project_id AND st.session_id = f.session_id
JOIN {source} s
  ON s.project_id = st.project_id AND s.trace_id = st.trace_id
LEFT JOIN gen_totals gt2 ON gt2.session_id = f.session_id
GROUP BY f.session_id, f.min_ts, f.{sort_field}
ORDER BY f.{sort_field} {sort_direction}, f.min_ts {sort_direction}, f.session_id ASC"#,
        limit = params.limit,
        projection = session_projection(Backend::Duckdb),
    );
    PageQuery {
        count: session_count_query(params, Backend::Duckdb),
        rows: ParameterizedQuery {
            sql,
            params: values,
        },
    }
}

fn clickhouse_session_time_scoped_dedup(params: &ListSessionsParams) -> (String, Vec<QueryValue>) {
    if params.from_timestamp.is_none() && params.to_timestamp.is_none() {
        return (clickhouse_dedup_lookup(""), Vec::new());
    }
    let mut scope = "project_id = ?".to_string();
    let mut values = vec![QueryValue::String(params.project_id.to_string())];
    if let Some(from) = &params.from_timestamp {
        scope.push_str(" AND timestamp_start >= fromUnixTimestamp64Micro(?)");
        values.push(QueryValue::Int64(from.timestamp_micros()));
    }
    if let Some(to) = &params.to_timestamp {
        scope.push_str(" AND timestamp_start <= fromUnixTimestamp64Micro(?)");
        values.push(QueryValue::Int64(to.timestamp_micros()));
    }
    (
        clickhouse_dedup_lookup(&format!(
            "trace_id IN (SELECT DISTINCT trace_id FROM otel_spans WHERE {scope})"
        )),
        values,
    )
}

fn clickhouse_session_page(params: &ListSessionsParams) -> PageQuery {
    let source = ClickhouseAnalyticsDialect.span_page_relation();
    let trace_sessions = canonical_session_relation(Backend::Clickhouse);
    let (where_clause, values) = session_conditions(params, "sp", Backend::Clickhouse);
    let (dedup, dedup_scope_values) = clickhouse_session_time_scoped_dedup(params);
    let (sort_field, sort_direction) = session_sort(params);
    let offset = params.page.saturating_sub(1) * params.limit;
    let gen_totals = clickhouse_gen_totals_joined_cte(
        Some("stg.session_id"),
        "JOIN session_traces stg \
         ON stg.project_id = g.project_id AND stg.trace_id = g.trace_id",
        "1 = 1",
    );
    let sql = format!(
        r#"WITH {dedup},
trace_sessions AS ({trace_sessions}),
matching_sessions AS (
    SELECT DISTINCT ts.project_id AS project_id, ts.session_id AS session_id
    FROM {source} sp
    JOIN trace_sessions ts
      ON ts.project_id = sp.project_id AND ts.trace_id = sp.trace_id
    WHERE {where_clause}
),
session_traces AS (
    SELECT ts.project_id AS project_id, ts.session_id AS session_id,
           ts.trace_id AS trace_id
    FROM trace_sessions ts
    JOIN matching_sessions ms
      ON ms.project_id = ts.project_id AND ms.session_id = ts.session_id
),
{gen_totals},
filtered_sessions AS (
    SELECT
        st.project_id AS project_id,
        st.session_id AS session_id,
        min(sp.timestamp_start) AS min_ts,
        max(coalesce(sp.timestamp_end, sp.timestamp_start)) AS max_ts,
        coalesce(max(gt.total_cost), 0) AS total_cost,
        count(DISTINCT sp.trace_id) AS trace_count,
        count() AS span_count,
        countIf(sp.observation_type != 'span') AS observation_count
    FROM session_traces st
    JOIN {source} sp
      ON sp.project_id = st.project_id AND sp.trace_id = st.trace_id
    LEFT JOIN gen_totals gt ON gt.session_id = st.session_id
    GROUP BY st.project_id, st.session_id
    ORDER BY {sort_field} {sort_direction}, min_ts {sort_direction}, st.session_id ASC
    LIMIT {limit} OFFSET {offset}
)
SELECT
{projection}
FROM filtered_sessions f
JOIN session_traces sto
  ON sto.project_id = f.project_id AND sto.session_id = f.session_id
JOIN {source} s
  ON s.project_id = sto.project_id AND s.trace_id = sto.trace_id
LEFT JOIN gen_totals gt2 ON gt2.session_id = f.session_id
GROUP BY f.session_id, f.min_ts, f.{sort_field}
ORDER BY f.{sort_field} {sort_direction}, f.min_ts {sort_direction}, f.session_id ASC"#,
        limit = params.limit,
        projection = session_projection(Backend::Clickhouse),
    );
    let mut row_values = vec![QueryValue::String(params.project_id.to_string())];
    row_values.extend(dedup_scope_values);
    row_values.extend(values);
    PageQuery {
        count: session_count_query(params, Backend::Clickhouse),
        rows: ParameterizedQuery {
            sql,
            params: row_values,
        },
    }
}

fn trace_conditions(
    params: &ListTracesParams,
    alias: &str,
    backend: Backend,
) -> (String, Vec<QueryValue>) {
    let dialect = analytics_dialect(backend);
    let column = |name: &str| {
        if alias.is_empty() {
            name.to_string()
        } else {
            format!("{alias}.{name}")
        }
    };
    let trace_column = column("trace_id");
    let source = dialect.span_page_relation();
    let mut conditions = vec![format!("{} = ?", column("project_id"))];
    let mut values = vec![QueryValue::String(params.project_id.to_string())];

    if let Some(session_id) = params.session_id.as_deref() {
        conditions.push(format!(
            "{trace_column} IN ({})",
            dialect.traces_of_session_relation()
        ));
        values.extend(dialect.traces_of_session_values(params.project_id.as_str(), session_id));
    }
    if let Some(user_id) = params.user_id.as_deref() {
        conditions.push(format!(
            "{trace_column} IN (SELECT DISTINCT trace_id FROM {source} \
             WHERE project_id = ? AND user_id = ?)"
        ));
        values.push(QueryValue::String(params.project_id.to_string()));
        values.push(QueryValue::String(user_id.to_string()));
    }
    if let Some(environments) = &params.environment
        && !environments.is_empty()
    {
        conditions.push(format!(
            "{trace_column} IN (SELECT DISTINCT trace_id FROM {source} \
             WHERE project_id = ? AND environment IN ({}))",
            placeholders(environments.len())
        ));
        values.push(QueryValue::String(params.project_id.to_string()));
        values.extend(environments.iter().cloned().map(QueryValue::String));
    }
    if let Some(from) = &params.from_timestamp {
        conditions.push(dialect.timestamp_comparison(&column("timestamp_start"), ">="));
        values.push(dialect.timestamp_value(from));
    }
    if let Some(to) = &params.to_timestamp {
        conditions.push(dialect.timestamp_comparison(&column("timestamp_start"), "<="));
        values.push(dialect.timestamp_value(to));
    }

    let mut aggregate_conditions = Vec::new();
    let mut aggregate_values = Vec::new();
    let mut aggregate_needs_totals = false;
    for filter in &params.filters {
        if filter.is_vacuous() {
            continue;
        }
        if filter.column() == "session_id" {
            let twin = filter.positive_twin();
            let rendered = twin.as_ref().unwrap_or(filter);
            let quantifier = if twin.is_some() { "NOT IN" } else { "IN" };
            let mut inner = Vec::new();
            let condition = dialect.render_filter_against(
                rendered,
                dialect.canonical_session_column(),
                &mut inner,
            );
            conditions.push(format!(
                "{trace_column} {quantifier} (SELECT cts.trace_id FROM ({}) cts \
                 WHERE cts.project_id = ? AND {condition})",
                dialect.canonical_trace_sessions_relation()
            ));
            values.push(QueryValue::String(params.project_id.to_string()));
            values.extend(inner);
            continue;
        }

        if let Some(expression) = trace_aggregate_expression(filter.column(), backend) {
            if let Some(twin) = filter.positive_twin() {
                let mut inner = Vec::new();
                let condition = dialect.render_filter_against(&twin, &expression, &mut inner);
                let (prelude, join, join_values) = if expression.contains("gtf.") {
                    trace_totals_context(params, backend)
                } else {
                    (String::new(), String::new(), Vec::new())
                };
                conditions.push(format!(
                    "{trace_column} NOT IN ({prelude}SELECT n.trace_id FROM {source} n {join} \
                     WHERE n.project_id = ? GROUP BY n.project_id, n.trace_id \
                     HAVING {condition})"
                ));
                values.extend(join_values);
                values.push(QueryValue::String(params.project_id.to_string()));
                values.extend(inner);
                continue;
            }

            aggregate_conditions.push(dialect.render_filter_against(
                filter,
                &expression,
                &mut aggregate_values,
            ));
            aggregate_needs_totals |= expression.contains("gtf.");
            continue;
        }

        let twin = filter.positive_twin();
        let rendered = twin.as_ref().unwrap_or(filter);
        let quantifier = if twin.is_some() { "NOT IN" } else { "IN" };
        let mut inner = Vec::new();
        let condition = dialect.render_filter_column(
            rendered,
            columns::map_trace_column_to_spans(rendered.column()),
            "n",
            &mut inner,
        );
        conditions.push(format!(
            "{trace_column} {quantifier} (SELECT n.trace_id FROM {source} n \
             WHERE n.project_id = ? AND {condition})"
        ));
        values.push(QueryValue::String(params.project_id.to_string()));
        values.extend(inner);
    }

    if !aggregate_conditions.is_empty() {
        let (prelude, join, join_values) = if aggregate_needs_totals {
            trace_totals_context(params, backend)
        } else {
            (String::new(), String::new(), Vec::new())
        };
        conditions.push(format!(
            "{trace_column} IN ({prelude}SELECT n.trace_id FROM {source} n {join} \
             WHERE n.project_id = ? GROUP BY n.project_id, n.trace_id HAVING {})",
            aggregate_conditions.join(" AND ")
        ));
        values.extend(join_values);
        values.push(QueryValue::String(params.project_id.to_string()));
        values.extend(aggregate_values);
    }

    (conditions.join(" AND "), values)
}

fn trace_aggregate_expression(column: &str, backend: Backend) -> Option<String> {
    let display_dialect = match backend {
        Backend::Duckdb => crate::display::DisplayNameDialect::DuckDb,
        Backend::Clickhouse => crate::display::DisplayNameDialect::ClickHouse,
        Backend::Sqlite | Backend::Postgres => return None,
    };
    let totals = |column: &str| {
        let aggregate = match backend {
            Backend::Duckdb => "MAX",
            Backend::Clickhouse => "max",
            Backend::Sqlite | Backend::Postgres => unreachable!(),
        };
        Some(format!("COALESCE({aggregate}(gtf.{column}), 0)"))
    };
    match column {
        "trace_name" => Some(crate::display::trace_display_name("n", display_dialect)),
        "session_id" | "user_id" | "environment" => Some(crate::display::trace_display_first(
            column,
            "n",
            display_dialect,
        )),
        "start_time" => Some(match backend {
            Backend::Duckdb => "MIN(n.timestamp_start)".to_string(),
            Backend::Clickhouse => "min(n.timestamp_start)".to_string(),
            _ => unreachable!(),
        }),
        "end_time" => Some(match backend {
            Backend::Duckdb => "MAX(COALESCE(n.timestamp_end, n.timestamp_start))".to_string(),
            Backend::Clickhouse => "max(coalesce(n.timestamp_end, n.timestamp_start))".to_string(),
            _ => unreachable!(),
        }),
        "duration_ms" => Some(match backend {
            Backend::Duckdb => "DATE_DIFF('millisecond', MIN(n.timestamp_start), \
                 MAX(COALESCE(n.timestamp_end, n.timestamp_start)))"
                .to_string(),
            Backend::Clickhouse => "dateDiff('millisecond', min(n.timestamp_start), \
                 max(coalesce(n.timestamp_end, n.timestamp_start)))"
                .to_string(),
            _ => unreachable!(),
        }),
        "input_tokens" | "output_tokens" | "total_tokens" | "cache_read_tokens"
        | "cache_write_tokens" | "reasoning_tokens" | "input_cost" | "output_cost"
        | "cache_read_cost" | "cache_write_cost" | "reasoning_cost" | "total_cost" => {
            totals(column)
        }
        _ => None,
    }
}

fn trace_totals_context(
    params: &ListTracesParams,
    backend: Backend,
) -> (String, String, Vec<QueryValue>) {
    match backend {
        Backend::Duckdb => {
            let mut scope = "g.project_id = ?".to_string();
            let mut values = vec![QueryValue::String(params.project_id.to_string())];
            if let Some(from) = &params.from_timestamp {
                scope.push_str(" AND g.timestamp_start >= ?");
                values.push(QueryValue::String(from.to_rfc3339()));
            }
            if let Some(to) = &params.to_timestamp {
                scope.push_str(" AND g.timestamp_start <= ?");
                values.push(QueryValue::String(to.to_rfc3339()));
            }
            (
                String::new(),
                format!(
                    "LEFT JOIN ({}) gtf ON gtf.trace_id = n.trace_id",
                    duckdb_gen_totals_sql(&scope)
                ),
                values,
            )
        }
        Backend::Clickhouse => {
            let mut scope = "g.project_id = ?".to_string();
            let mut values = vec![
                QueryValue::String(params.project_id.to_string()),
                QueryValue::String(params.project_id.to_string()),
            ];
            if let Some(from) = &params.from_timestamp {
                scope.push_str(" AND g.timestamp_start >= fromUnixTimestamp64Micro(?)");
                values.push(QueryValue::Int64(from.timestamp_micros()));
            }
            if let Some(to) = &params.to_timestamp {
                scope.push_str(" AND g.timestamp_start <= fromUnixTimestamp64Micro(?)");
                values.push(QueryValue::Int64(to.timestamp_micros()));
            }
            (
                format!(
                    "WITH {}, {} ",
                    clickhouse_dedup_lookup(""),
                    clickhouse_gen_totals_cte(Some("g.trace_id"), &scope)
                ),
                "LEFT JOIN gen_totals gtf ON gtf.trace_id = n.trace_id".to_string(),
                values,
            )
        }
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

fn duckdb_gen_totals_sql(where_clause: &str) -> String {
    duckdb_gen_totals_joined_sql("g.trace_id", "", where_clause)
}

fn duckdb_gen_totals_joined_sql(key: &str, join: &str, where_clause: &str) -> String {
    let source = DuckdbAnalyticsDialect.span_page_relation();
    let bare_key = key.rsplit('.').next().unwrap_or(key);
    let join = if join.is_empty() {
        String::new()
    } else {
        format!("\n        {join}")
    };
    format!(
        r#"SELECT
            {key} AS {bare_key},
            COALESCE(SUM(gen_ai_usage_input_tokens), 0) AS input_tokens,
            COALESCE(SUM(gen_ai_usage_output_tokens), 0) AS output_tokens,
            COALESCE(SUM(gen_ai_usage_total_tokens), 0) AS total_tokens,
            COALESCE(SUM(gen_ai_usage_cache_read_tokens), 0) AS cache_read_tokens,
            COALESCE(SUM(gen_ai_usage_cache_write_tokens), 0) AS cache_write_tokens,
            COALESCE(SUM(gen_ai_usage_reasoning_tokens), 0) AS reasoning_tokens,
            COALESCE(SUM(gen_ai_cost_input), 0) AS input_cost,
            COALESCE(SUM(gen_ai_cost_output), 0) AS output_cost,
            COALESCE(SUM(gen_ai_cost_cache_read), 0) AS cache_read_cost,
            COALESCE(SUM(gen_ai_cost_cache_write), 0) AS cache_write_cost,
            COALESCE(SUM(gen_ai_cost_reasoning), 0) AS reasoning_cost,
            COALESCE(SUM(gen_ai_cost_total), 0) AS total_cost
        FROM {source} g{join}
        WHERE {where_clause}
          AND (
              (g.observation_type = 'generation'
               AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens
                     + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens
                     + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0
                    OR g.gen_ai_cost_total > 0)
               AND NOT EXISTS (
                   SELECT 1 FROM {source} c
                   WHERE c.parent_span_id = g.span_id
                     AND c.trace_id = g.trace_id
                     AND c.project_id = g.project_id
                     AND c.observation_type = 'generation'
                     AND ((c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens
                           + c.gen_ai_usage_total_tokens + c.gen_ai_usage_cache_read_tokens
                           + c.gen_ai_usage_cache_write_tokens + c.gen_ai_usage_reasoning_tokens) > 0
                          OR c.gen_ai_cost_total > 0)
               ))
              OR
              ((g.observation_type IS NULL OR g.observation_type != 'generation')
               AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens
                     + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens
                     + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0
                    OR g.gen_ai_cost_total > 0)
               AND NOT EXISTS (
                   SELECT 1 FROM {source} gen
                   WHERE gen.trace_id = g.trace_id
                     AND gen.project_id = g.project_id
                     AND gen.observation_type = 'generation'
                     AND ((gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens
                           + gen.gen_ai_usage_total_tokens + gen.gen_ai_usage_cache_read_tokens
                           + gen.gen_ai_usage_cache_write_tokens + gen.gen_ai_usage_reasoning_tokens) > 0
                          OR gen.gen_ai_cost_total > 0)
               )
               AND NOT EXISTS (
                   SELECT 1 FROM {source} p
                   WHERE p.span_id = g.parent_span_id
                     AND p.trace_id = g.trace_id
                     AND p.project_id = g.project_id
                     AND ((p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens
                           + p.gen_ai_usage_total_tokens + p.gen_ai_usage_cache_read_tokens
                           + p.gen_ai_usage_cache_write_tokens + p.gen_ai_usage_reasoning_tokens) > 0
                          OR p.gen_ai_cost_total > 0)
               ))
          )
        GROUP BY {key}"#
    )
}

fn clickhouse_dedup_lookup(extra_where: &str) -> String {
    let extra = if extra_where.is_empty() {
        String::new()
    } else {
        format!("\n          AND {extra_where}")
    };
    format!(
        r#"dedup_lookup AS (
        SELECT span_id, parent_span_id, trace_id, observation_type
        FROM otel_spans FINAL
        WHERE project_id = ?
          AND ((gen_ai_usage_input_tokens + gen_ai_usage_output_tokens
                + gen_ai_usage_total_tokens + gen_ai_usage_cache_read_tokens
                + gen_ai_usage_cache_write_tokens + gen_ai_usage_reasoning_tokens) > 0
               OR gen_ai_cost_total > 0){extra}
    )"#
    )
}

const CLICKHOUSE_TOKEN_DEDUP_CONDITION: &str = r#"(
      (g.observation_type = 'generation'
       AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens
             + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens
             + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0
            OR g.gen_ai_cost_total > 0)
       AND (g.trace_id, g.span_id) NOT IN (
           SELECT trace_id, parent_span_id FROM dedup_lookup
           WHERE observation_type = 'generation' AND parent_span_id IS NOT NULL
       ))
      OR
      ((g.observation_type IS NULL OR g.observation_type != 'generation')
       AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens
             + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens
             + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0
            OR g.gen_ai_cost_total > 0)
       AND g.trace_id NOT IN (
           SELECT DISTINCT trace_id FROM dedup_lookup
           WHERE observation_type = 'generation'
       )
       AND (g.parent_span_id IS NULL OR (g.trace_id, g.parent_span_id) NOT IN (
           SELECT trace_id, span_id FROM dedup_lookup
       )))
  )"#;

fn clickhouse_gen_totals_cte(key: Option<&str>, scope: &str) -> String {
    clickhouse_gen_totals_joined_cte(key, "", scope)
}

fn clickhouse_gen_totals_joined_cte(key: Option<&str>, join: &str, scope: &str) -> String {
    let select_key = key
        .map(|key| {
            let bare = key.rsplit('.').next().unwrap_or(key);
            format!("{key} AS {bare},\n                ")
        })
        .unwrap_or_default();
    let group_by = key
        .map(|key| format!("\n        GROUP BY {key}"))
        .unwrap_or_default();
    let join = if join.is_empty() {
        String::new()
    } else {
        format!("\n        {join}")
    };
    format!(
        r#"gen_totals AS (
        SELECT
            {select_key}sum(gen_ai_usage_input_tokens) AS input_tokens,
            sum(gen_ai_usage_output_tokens) AS output_tokens,
            sum(gen_ai_usage_total_tokens) AS total_tokens,
            sum(gen_ai_usage_cache_read_tokens) AS cache_read_tokens,
            sum(gen_ai_usage_cache_write_tokens) AS cache_write_tokens,
            sum(gen_ai_usage_reasoning_tokens) AS reasoning_tokens,
            sum(toFloat64(gen_ai_cost_input)) AS input_cost,
            sum(toFloat64(gen_ai_cost_output)) AS output_cost,
            sum(toFloat64(gen_ai_cost_cache_read)) AS cache_read_cost,
            sum(toFloat64(gen_ai_cost_cache_write)) AS cache_write_cost,
            sum(toFloat64(gen_ai_cost_reasoning)) AS reasoning_cost,
            sum(toFloat64(gen_ai_cost_total)) AS total_cost
        FROM otel_spans g FINAL{join}
        WHERE {scope}
          AND {CLICKHOUSE_TOKEN_DEDUP_CONDITION}{group_by}
    )"#
    )
}

fn trace_sort(params: &ListTracesParams) -> (&'static str, &'static str) {
    let direction = params
        .order_by
        .as_ref()
        .map(|order| match order.direction {
            OrderDirection::Asc => "ASC",
            OrderDirection::Desc => "DESC",
        })
        .unwrap_or("DESC");
    let column = params
        .order_by
        .as_ref()
        .map(|order| order.column.as_str())
        .unwrap_or("timestamp_start");
    let field = match column {
        "start_time" => "min_ts",
        "end_time" => "max_ts",
        "duration_ms" => "duration_ms",
        "total_cost" => "total_cost",
        "total_tokens" => "total_tokens",
        "observation_count" => "observation_count",
        _ => "min_ts",
    };
    (field, direction)
}

fn duckdb_trace_projection() -> String {
    format!(
        r#"t.trace_id,
    {trace_name} AS trace_name,
    MIN(s.timestamp_start) AS start_time,
    MAX(COALESCE(s.timestamp_end, s.timestamp_start)) AS end_time,
    DATE_DIFF('millisecond', MIN(s.timestamp_start),
              MAX(COALESCE(s.timestamp_end, s.timestamp_start))) AS duration_ms,
    FIRST(s.session_id ORDER BY s.timestamp_start, s.span_id)
        FILTER (WHERE s.session_id IS NOT NULL) AS session_id,
    FIRST(s.user_id ORDER BY s.timestamp_start, s.span_id)
        FILTER (WHERE s.user_id IS NOT NULL) AS user_id,
    FIRST(s.environment ORDER BY s.timestamp_start, s.span_id)
        FILTER (WHERE s.environment IS NOT NULL) AS environment,
    COUNT(*) AS span_count,
    COALESCE(MAX(gt2.input_tokens), 0) AS input_tokens,
    COALESCE(MAX(gt2.output_tokens), 0) AS output_tokens,
    COALESCE(MAX(gt2.total_tokens), 0) AS total_tokens,
    COALESCE(MAX(gt2.cache_read_tokens), 0) AS cache_read_tokens,
    COALESCE(MAX(gt2.cache_write_tokens), 0) AS cache_write_tokens,
    COALESCE(MAX(gt2.reasoning_tokens), 0) AS reasoning_tokens,
    COALESCE(MAX(gt2.input_cost), 0)::DOUBLE AS input_cost,
    COALESCE(MAX(gt2.output_cost), 0)::DOUBLE AS output_cost,
    COALESCE(MAX(gt2.cache_read_cost), 0)::DOUBLE AS cache_read_cost,
    COALESCE(MAX(gt2.cache_write_cost), 0)::DOUBLE AS cache_write_cost,
    COALESCE(MAX(gt2.reasoning_cost), 0)::DOUBLE AS reasoning_cost,
    COALESCE(MAX(gt2.total_cost), 0)::DOUBLE AS total_cost,
    TO_JSON(LIST_DISTINCT(FLATTEN(LIST(s.tags::JSON::VARCHAR[])))) AS tags,
    COUNT(*) FILTER (WHERE s.observation_type != 'span') AS observation_count,
    TO_JSON(FIRST(s.metadata ORDER BY s.timestamp_start, s.span_id)
        FILTER (WHERE s.parent_span_id IS NULL)) AS metadata,
    COALESCE(
        FIRST(s.input_preview ORDER BY s.timestamp_start, s.span_id)
            FILTER (WHERE s.parent_span_id IS NULL AND s.input_preview IS NOT NULL),
        FIRST(s.input_preview ORDER BY s.timestamp_start, s.span_id)
            FILTER (WHERE s.input_preview IS NOT NULL)
    ) AS input_preview,
    COALESCE(
        FIRST(s.output_preview ORDER BY s.timestamp_start DESC, s.span_id DESC)
            FILTER (WHERE s.parent_span_id IS NULL AND s.output_preview IS NOT NULL),
        FIRST(s.output_preview ORDER BY s.timestamp_start DESC, s.span_id DESC)
            FILTER (WHERE s.output_preview IS NOT NULL)
    ) AS output_preview,
    bool_or(s.status_code = 'ERROR') AS has_error"#,
        trace_name =
            crate::display::trace_display_name("s", crate::display::DisplayNameDialect::DuckDb)
    )
}

fn clickhouse_trace_projection() -> String {
    let totals = [
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "reasoning_tokens",
        "input_cost",
        "output_cost",
        "cache_read_cost",
        "cache_write_cost",
        "reasoning_cost",
        "total_cost",
    ]
    .iter()
    .map(|column| format!("    coalesce(max(gt2.{column}), 0) AS {column},"))
    .collect::<Vec<_>>()
    .join("\n");
    format!(
        r#"t.trace_id AS trace_id,
    {trace_name} AS trace_name,
    toInt64(toUnixTimestamp64Micro(min(s.timestamp_start))) AS start_time,
    toInt64(toUnixTimestamp64Micro(
        max(coalesce(s.timestamp_end, s.timestamp_start)))) AS end_time,
    dateDiff('millisecond', min(s.timestamp_start),
             max(coalesce(s.timestamp_end, s.timestamp_start))) AS duration_ms,
    argMinIf(s.session_id, (s.timestamp_start, s.span_id),
             s.session_id IS NOT NULL) AS session_id,
    argMinIf(s.user_id, (s.timestamp_start, s.span_id),
             s.user_id IS NOT NULL) AS user_id,
    argMinIf(s.environment, (s.timestamp_start, s.span_id),
             s.environment IS NOT NULL) AS environment,
    count() AS span_count,
{totals}
    toNullable(toJSONString(arrayDistinct(arrayFlatten(groupArray(
        JSONExtract(ifNull(s.tags, '[]'), 'Array(String)')
    ))))) AS tags,
    countIf(s.observation_type != 'span') AS observation_count,
    argMinIf(s.metadata, (s.timestamp_start, s.span_id),
             s.parent_span_id IS NULL) AS metadata,
    COALESCE(
        argMinIf(s.input_preview, (s.timestamp_start, s.span_id),
                 s.parent_span_id IS NULL AND s.input_preview IS NOT NULL
                 AND s.input_preview != ''),
        argMinIf(s.input_preview, (s.timestamp_start, s.span_id),
                 s.input_preview IS NOT NULL AND s.input_preview != '')
    ) AS input_preview,
    COALESCE(
        argMaxIf(s.output_preview, (s.timestamp_start, s.span_id),
                 s.parent_span_id IS NULL AND s.output_preview IS NOT NULL
                 AND s.output_preview != ''),
        argMaxIf(s.output_preview, (s.timestamp_start, s.span_id),
                 s.output_preview IS NOT NULL AND s.output_preview != '')
    ) AS output_preview,
    coalesce(max(s.status_code = 'ERROR'), 0) AS has_error"#,
        trace_name =
            crate::display::trace_display_name("s", crate::display::DisplayNameDialect::ClickHouse)
    )
}

fn duckdb_trace_page(params: &ListTracesParams) -> PageQuery {
    let source = DuckdbAnalyticsDialect.span_page_relation();
    let (count_where, count_values) = trace_conditions(params, "s", Backend::Duckdb);
    let count_sql = if params.include_nongenai {
        format!("SELECT COUNT(DISTINCT s.trace_id) FROM {source} s WHERE {count_where}")
    } else {
        format!(
            "SELECT COUNT(*) FROM (\
             SELECT s.trace_id FROM {source} s WHERE {count_where} \
             GROUP BY s.project_id, s.trace_id \
             HAVING COUNT(*) FILTER (WHERE {}) > 0) counted",
            crate::display::genai_span_predicate("s")
        )
    };

    let (where_g, values_g) = trace_conditions(params, "g", Backend::Duckdb);
    let (where_sp, values_sp) = trace_conditions(params, "sp", Backend::Duckdb);
    let (sort_field, sort_direction) = trace_sort(params);
    let offset = params.page.saturating_sub(1) * params.limit;
    let having = if params.include_nongenai {
        String::new()
    } else {
        "HAVING observation_count > 0 OR genai_span_count > 0".to_string()
    };
    let data_sql = format!(
        r#"WITH gen_totals AS (
    {gen_totals}
),
filtered_traces AS (
    SELECT
        sp.project_id,
        sp.trace_id,
        MIN(sp.timestamp_start) AS min_ts,
        MAX(COALESCE(sp.timestamp_end, sp.timestamp_start)) AS max_ts,
        DATE_DIFF('millisecond', MIN(sp.timestamp_start),
                  MAX(COALESCE(sp.timestamp_end, sp.timestamp_start))) AS duration_ms,
        COALESCE(MAX(gt.total_cost), 0)::DOUBLE AS total_cost,
        COALESCE(MAX(gt.total_tokens), 0) AS total_tokens,
        COUNT(*) FILTER (WHERE sp.observation_type != 'span') AS observation_count,
        COUNT(*) FILTER (WHERE {genai_sp}) AS genai_span_count
    FROM {source} sp
    LEFT JOIN gen_totals gt ON sp.trace_id = gt.trace_id
    WHERE {where_sp}
    GROUP BY sp.project_id, sp.trace_id
    {having}
    ORDER BY {sort_field} {sort_direction}, min_ts {sort_direction}, sp.trace_id ASC
    LIMIT {limit} OFFSET {offset}
)
SELECT
{projection}
FROM filtered_traces t
JOIN {source} s ON t.project_id = s.project_id AND t.trace_id = s.trace_id
LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id
GROUP BY t.trace_id, t.min_ts, t.{sort_field}
ORDER BY t.{sort_field} {sort_direction}, t.min_ts {sort_direction}, t.trace_id ASC"#,
        gen_totals = duckdb_gen_totals_sql(&where_g),
        genai_sp = crate::display::genai_span_predicate("sp"),
        limit = params.limit,
        projection = duckdb_trace_projection(),
    );
    let mut row_values = values_g;
    row_values.extend(values_sp);

    PageQuery {
        count: ParameterizedQuery {
            sql: count_sql,
            params: count_values,
        },
        rows: ParameterizedQuery {
            sql: data_sql,
            params: row_values,
        },
    }
}

fn clickhouse_time_scoped_dedup(params: &ListTracesParams) -> (String, Vec<QueryValue>) {
    if params.from_timestamp.is_none() && params.to_timestamp.is_none() {
        return (clickhouse_dedup_lookup(""), Vec::new());
    }
    let mut scope = "project_id = ?".to_string();
    let mut values = vec![QueryValue::String(params.project_id.to_string())];
    if let Some(from) = &params.from_timestamp {
        scope.push_str(" AND timestamp_start >= fromUnixTimestamp64Micro(?)");
        values.push(QueryValue::Int64(from.timestamp_micros()));
    }
    if let Some(to) = &params.to_timestamp {
        scope.push_str(" AND timestamp_start <= fromUnixTimestamp64Micro(?)");
        values.push(QueryValue::Int64(to.timestamp_micros()));
    }
    (
        clickhouse_dedup_lookup(&format!(
            "trace_id IN (SELECT DISTINCT trace_id FROM otel_spans WHERE {scope})"
        )),
        values,
    )
}

fn clickhouse_trace_page(params: &ListTracesParams) -> PageQuery {
    let source = ClickhouseAnalyticsDialect.span_page_relation();
    let (count_where, count_values) = trace_conditions(params, "s", Backend::Clickhouse);
    let count_sql = if params.include_nongenai {
        format!(
            "SELECT count() AS cnt FROM (SELECT s.trace_id FROM {source} s \
             WHERE {count_where} GROUP BY s.project_id, s.trace_id)"
        )
    } else {
        format!(
            "SELECT count() AS cnt FROM (SELECT s.trace_id FROM {source} s \
             WHERE {count_where} GROUP BY s.project_id, s.trace_id \
             HAVING countIf({}) > 0)",
            crate::display::genai_span_predicate("s")
        )
    };

    let (where_g, values_g) = trace_conditions(params, "g", Backend::Clickhouse);
    let (where_sp, values_sp) = trace_conditions(params, "sp", Backend::Clickhouse);
    let (dedup, dedup_scope_values) = clickhouse_time_scoped_dedup(params);
    let (sort_field, sort_direction) = trace_sort(params);
    let offset = params.page.saturating_sub(1) * params.limit;
    let having = if params.include_nongenai {
        String::new()
    } else {
        "HAVING observation_count > 0 OR genai_span_count > 0".to_string()
    };
    let data_sql = format!(
        r#"WITH {dedup},
{gen_totals},
filtered_traces AS (
    SELECT
        sp.project_id,
        sp.trace_id,
        min(sp.timestamp_start) AS min_ts,
        max(coalesce(sp.timestamp_end, sp.timestamp_start)) AS max_ts,
        dateDiff('millisecond', min(sp.timestamp_start),
                 max(coalesce(sp.timestamp_end, sp.timestamp_start))) AS duration_ms,
        coalesce(max(gt.total_cost), 0) AS total_cost,
        coalesce(max(gt.total_tokens), 0) AS total_tokens,
        countIf(sp.observation_type != 'span') AS observation_count,
        countIf({genai_sp}) AS genai_span_count
    FROM otel_spans sp FINAL
    LEFT JOIN gen_totals gt ON sp.trace_id = gt.trace_id
    WHERE {where_sp}
    GROUP BY sp.project_id, sp.trace_id
    {having}
    ORDER BY {sort_field} {sort_direction}, min_ts {sort_direction}, sp.trace_id ASC
    LIMIT {limit} OFFSET {offset}
)
SELECT
{projection}
FROM filtered_traces t
JOIN otel_spans s FINAL ON t.project_id = s.project_id AND t.trace_id = s.trace_id
LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id
GROUP BY t.trace_id, t.min_ts, t.{sort_field}
ORDER BY t.{sort_field} {sort_direction}, t.min_ts {sort_direction}, t.trace_id ASC"#,
        gen_totals = clickhouse_gen_totals_cte(Some("g.trace_id"), &where_g),
        genai_sp = crate::display::genai_span_predicate("sp"),
        limit = params.limit,
        projection = clickhouse_trace_projection(),
    );
    let mut row_values = vec![QueryValue::String(params.project_id.to_string())];
    row_values.extend(dedup_scope_values);
    row_values.extend(values_g);
    row_values.extend(values_sp);

    PageQuery {
        count: ParameterizedQuery {
            sql: count_sql,
            params: count_values,
        },
        rows: ParameterizedQuery {
            sql: data_sql,
            params: row_values,
        },
    }
}

fn push_optional_eq(
    conditions: &mut Vec<String>,
    values: &mut Vec<QueryValue>,
    column: &str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        conditions.push(format!("{column} = ?"));
        values.push(QueryValue::String(value.to_string()));
    }
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

fn span_order(params: &ListSpansParams) -> String {
    let (column, direction) = params
        .order_by
        .as_ref()
        .map(|order| {
            let column = match order.column.as_str() {
                "start_time" | "timestamp_start" => "timestamp_start",
                "end_time" | "timestamp_end" => "timestamp_end",
                "duration_ms" => "duration_ms",
                "span_name" => "span_name",
                _ => "timestamp_start",
            };
            let direction = match order.direction {
                OrderDirection::Asc => "ASC",
                OrderDirection::Desc => "DESC",
            };
            (column, direction)
        })
        .unwrap_or(("timestamp_start", "DESC"));
    format!("{column} {direction}, trace_id ASC, span_id ASC")
}

impl SelectStatement {
    pub fn operation(&self) -> QueryOperation {
        self.operation
    }

    pub fn render(&self, backend: Backend) -> RenderedQuery {
        let dialect = analytics_dialect(backend);

        let projection = match self.projection {
            Projection::SpanDetail => dialect.span_detail_projection(),
        };
        let relation = match self.relation {
            Relation::Spans => dialect.winning_spans_relation(),
        };

        let predicates = self
            .predicates
            .iter()
            .enumerate()
            .map(|(index, predicate)| {
                format!(
                    "{} = {}",
                    predicate.column.name(),
                    dialect.placeholder(index + 1)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let suffix = dialect.after_where();
        let limit = self
            .limit
            .map(|limit| format!("\nLIMIT {limit}"))
            .unwrap_or_default();

        RenderedQuery {
            sql: format!(
                "SELECT\n{projection}\nFROM {relation}\nWHERE {predicates}{suffix}{limit}"
            ),
            bindings: self
                .predicates
                .iter()
                .map(|predicate| predicate.binding)
                .collect(),
        }
    }
}

fn analytics_dialect(backend: Backend) -> &'static dyn AnalyticsDialect {
    match backend {
        Backend::Duckdb => &DuckdbAnalyticsDialect,
        Backend::Clickhouse => &ClickhouseAnalyticsDialect,
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    }
}

/// Capabilities needed to lower analytical reads.
trait AnalyticsDialect {
    fn placeholder(&self, index: usize) -> String;
    fn winning_spans_relation(&self) -> &'static str;
    fn after_where(&self) -> &'static str;
    fn span_detail_projection(&self) -> &'static str;
    fn span_page_relation(&self) -> &'static str;
    fn traces_of_session_relation(&self) -> &'static str;
    fn traces_of_session_values(&self, project_id: &str, session_id: &str) -> Vec<QueryValue>;
    fn canonical_trace_sessions_relation(&self) -> &'static str;
    fn canonical_session_column(&self) -> &'static str;
    fn timestamp_comparison(&self, column: &str, operator: &str) -> String;
    fn timestamp_value(&self, value: &chrono::DateTime<chrono::Utc>) -> QueryValue;
    fn span_count_sql(&self, source: &str, where_clause: &str) -> String;
    fn render_filter_column(
        &self,
        filter: &Filter,
        column: &str,
        alias: &str,
        values: &mut Vec<QueryValue>,
    ) -> String;
    fn render_filter_against(
        &self,
        filter: &Filter,
        expression: &str,
        values: &mut Vec<QueryValue>,
    ) -> String;
}

struct DuckdbAnalyticsDialect;

impl AnalyticsDialect for DuckdbAnalyticsDialect {
    fn placeholder(&self, _index: usize) -> String {
        "?".to_string()
    }

    fn winning_spans_relation(&self) -> &'static str {
        "otel_spans"
    }

    fn after_where(&self) -> &'static str {
        "\nQUALIFY ROW_NUMBER() OVER (\
         PARTITION BY project_id, trace_id, span_id \
         ORDER BY ingested_at DESC, rowid DESC) = 1"
    }

    fn span_page_relation(&self) -> &'static str {
        "(SELECT * FROM otel_spans \
         QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
         ORDER BY ingested_at DESC, rowid DESC) = 1)"
    }

    fn span_detail_projection(&self) -> &'static str {
        r#"    trace_id,
    span_id,
    parent_span_id,
    span_name,
    span_kind,
    span_category,
    observation_type,
    framework,
    status_code,
    EPOCH_US(timestamp_start) AS start_time,
    CASE WHEN timestamp_end IS NOT NULL THEN EPOCH_US(timestamp_end) END AS end_time,
    duration_ms,
    environment,
    (raw_span->'resource'->'attributes')::VARCHAR AS resource_attributes,
    session_id,
    user_id,
    gen_ai_system,
    gen_ai_request_model,
    gen_ai_agent_name,
    gen_ai_finish_reasons,
    gen_ai_usage_input_tokens,
    gen_ai_usage_output_tokens,
    gen_ai_usage_total_tokens,
    gen_ai_usage_cache_read_tokens,
    gen_ai_usage_cache_write_tokens,
    gen_ai_usage_reasoning_tokens,
    gen_ai_cost_input::DOUBLE AS gen_ai_cost_input,
    gen_ai_cost_output::DOUBLE AS gen_ai_cost_output,
    gen_ai_cost_cache_read::DOUBLE AS gen_ai_cost_cache_read,
    gen_ai_cost_cache_write::DOUBLE AS gen_ai_cost_cache_write,
    gen_ai_cost_reasoning::DOUBLE AS gen_ai_cost_reasoning,
    gen_ai_cost_total::DOUBLE AS gen_ai_cost_total,
    gen_ai_usage_details::VARCHAR AS gen_ai_usage_details,
    metadata::VARCHAR AS metadata,
    (raw_span->'attributes')::VARCHAR AS attributes,
    input_preview,
    output_preview,
    raw_span::VARCHAR AS raw_span,
    EPOCH_US(ingested_at) AS ingested_at_us,
    scope_name,
    scope_version"#
    }

    fn traces_of_session_relation(&self) -> &'static str {
        "SELECT trace_id FROM ( \
           SELECT trace_id, arg_min(session_id, (timestamp_start, span_id)) AS canonical_session \
           FROM (SELECT * FROM otel_spans \
                 WHERE project_id = ? \
                 AND trace_id IN (SELECT trace_id FROM otel_spans \
                                  WHERE project_id = ? AND session_id = ?) \
                 QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                            ORDER BY ingested_at DESC, rowid DESC) = 1) \
           WHERE session_id IS NOT NULL AND session_id != '' \
           GROUP BY trace_id \
         ) WHERE canonical_session = ?"
    }

    fn traces_of_session_values(&self, project_id: &str, session_id: &str) -> Vec<QueryValue> {
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(project_id.to_string()),
            QueryValue::String(session_id.to_string()),
            QueryValue::String(session_id.to_string()),
        ]
    }

    fn canonical_trace_sessions_relation(&self) -> &'static str {
        "SELECT project_id, trace_id, \
           arg_min(session_id, (timestamp_start, span_id)) AS session_id \
         FROM (SELECT * FROM otel_spans \
               QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                          ORDER BY ingested_at DESC, rowid DESC) = 1) \
         WHERE session_id IS NOT NULL AND session_id != '' \
         GROUP BY project_id, trace_id"
    }

    fn canonical_session_column(&self) -> &'static str {
        "cts.session_id"
    }

    fn timestamp_comparison(&self, column: &str, operator: &str) -> String {
        format!("{column} {operator} ?")
    }

    fn timestamp_value(&self, value: &chrono::DateTime<chrono::Utc>) -> QueryValue {
        QueryValue::String(value.to_rfc3339())
    }

    fn span_count_sql(&self, source: &str, where_clause: &str) -> String {
        format!(
            "SELECT COUNT(*) FROM (SELECT DISTINCT trace_id, span_id \
             FROM {source} WHERE {where_clause}) _c"
        )
    }

    fn render_filter_column(
        &self,
        filter: &Filter,
        column: &str,
        alias: &str,
        values: &mut Vec<QueryValue>,
    ) -> String {
        render_filter(
            FilterFlavor::Duckdb,
            filter,
            FilterTarget::Column { column, alias },
            values,
        )
    }

    fn render_filter_against(
        &self,
        filter: &Filter,
        expression: &str,
        values: &mut Vec<QueryValue>,
    ) -> String {
        render_filter(
            FilterFlavor::Duckdb,
            filter,
            FilterTarget::Expression(expression),
            values,
        )
    }
}

struct ClickhouseAnalyticsDialect;

impl AnalyticsDialect for ClickhouseAnalyticsDialect {
    fn placeholder(&self, _index: usize) -> String {
        "?".to_string()
    }

    fn winning_spans_relation(&self) -> &'static str {
        "otel_spans FINAL"
    }

    fn after_where(&self) -> &'static str {
        ""
    }

    fn span_page_relation(&self) -> &'static str {
        "(SELECT * FROM otel_spans FINAL)"
    }

    fn span_detail_projection(&self) -> &'static str {
        r#"    trace_id,
    span_id,
    parent_span_id,
    span_name,
    span_kind,
    span_category,
    observation_type,
    framework,
    status_code,
    toInt64(toUnixTimestamp64Micro(timestamp_start)) AS start_time,
    if(timestamp_end IS NOT NULL, toInt64(toUnixTimestamp64Micro(timestamp_end)), NULL) AS end_time,
    duration_ms,
    environment,
    JSONExtractRaw(raw_span, 'resource', 'attributes') AS resource_attributes,
    session_id,
    user_id,
    gen_ai_system,
    gen_ai_request_model,
    gen_ai_agent_name,
    gen_ai_finish_reasons,
    gen_ai_usage_input_tokens,
    gen_ai_usage_output_tokens,
    gen_ai_usage_total_tokens,
    gen_ai_usage_cache_read_tokens,
    gen_ai_usage_cache_write_tokens,
    gen_ai_usage_reasoning_tokens,
    toFloat64(gen_ai_cost_input) AS gen_ai_cost_input,
    toFloat64(gen_ai_cost_output) AS gen_ai_cost_output,
    toFloat64(gen_ai_cost_cache_read) AS gen_ai_cost_cache_read,
    toFloat64(gen_ai_cost_cache_write) AS gen_ai_cost_cache_write,
    toFloat64(gen_ai_cost_reasoning) AS gen_ai_cost_reasoning,
    toFloat64(gen_ai_cost_total) AS gen_ai_cost_total,
    gen_ai_usage_details,
    metadata,
    JSONExtractRaw(raw_span, 'attributes') AS attributes,
    input_preview,
    output_preview,
    raw_span,
    toInt64(toUnixTimestamp64Micro(ingested_at)) AS ingested_at_us,
    scope_name,
    scope_version"#
    }

    fn traces_of_session_relation(&self) -> &'static str {
        "SELECT trace_id FROM ( \
           SELECT trace_id, argMin(assumeNotNull(session_id), (timestamp_start, span_id)) \
                  AS canonical_session \
           FROM otel_spans FINAL \
           WHERE project_id = ? \
           AND trace_id IN (SELECT trace_id FROM otel_spans \
                            WHERE project_id = ? AND session_id = ?) \
           AND session_id IS NOT NULL AND session_id != '' \
           GROUP BY trace_id \
         ) WHERE canonical_session = ?"
    }

    fn traces_of_session_values(&self, project_id: &str, session_id: &str) -> Vec<QueryValue> {
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(project_id.to_string()),
            QueryValue::String(session_id.to_string()),
            QueryValue::String(session_id.to_string()),
        ]
    }

    fn canonical_trace_sessions_relation(&self) -> &'static str {
        "SELECT project_id, trace_id, \
           argMin(assumeNotNull(session_id), (timestamp_start, span_id)) AS canonical_session \
         FROM otel_spans FINAL \
         WHERE session_id IS NOT NULL AND session_id != '' \
         GROUP BY project_id, trace_id"
    }

    fn canonical_session_column(&self) -> &'static str {
        "cts.canonical_session"
    }

    fn timestamp_comparison(&self, column: &str, operator: &str) -> String {
        format!("{column} {operator} fromUnixTimestamp64Micro(?)")
    }

    fn timestamp_value(&self, value: &chrono::DateTime<chrono::Utc>) -> QueryValue {
        QueryValue::Int64(value.timestamp_micros())
    }

    fn span_count_sql(&self, source: &str, where_clause: &str) -> String {
        format!("SELECT count() AS cnt FROM {source} WHERE {where_clause}")
    }

    fn render_filter_column(
        &self,
        filter: &Filter,
        column: &str,
        alias: &str,
        values: &mut Vec<QueryValue>,
    ) -> String {
        render_filter(
            FilterFlavor::Clickhouse,
            filter,
            FilterTarget::Column { column, alias },
            values,
        )
    }

    fn render_filter_against(
        &self,
        filter: &Filter,
        expression: &str,
        values: &mut Vec<QueryValue>,
    ) -> String {
        render_filter(
            FilterFlavor::Clickhouse,
            filter,
            FilterTarget::Expression(expression),
            values,
        )
    }
}

#[derive(Clone, Copy)]
enum FilterFlavor {
    Duckdb,
    Clickhouse,
}

enum FilterTarget<'a> {
    Column { column: &'a str, alias: &'a str },
    Expression(&'a str),
}

impl FilterTarget<'_> {
    fn expression(&self) -> String {
        match self {
            Self::Column { column, alias: "" } => (*column).to_string(),
            Self::Column { column, alias } => format!("{alias}.{column}"),
            Self::Expression(expression) => (*expression).to_string(),
        }
    }

    fn column(&self) -> Option<&str> {
        match self {
            Self::Column { column, .. } => Some(column),
            Self::Expression(_) => None,
        }
    }
}

fn render_filter(
    flavor: FilterFlavor,
    filter: &Filter,
    target: FilterTarget<'_>,
    values: &mut Vec<QueryValue>,
) -> String {
    if matches!(target, FilterTarget::Column { .. }) && !is_plain_identifier(filter.column()) {
        return "1 = 0".to_string();
    }

    let expression = target.expression();
    match filter {
        Filter::Datetime {
            operator, value, ..
        } => {
            let operator = match operator {
                DatetimeOp::Gt => ">",
                DatetimeOp::Lt => "<",
                DatetimeOp::Gte => ">=",
                DatetimeOp::Lte => "<=",
            };
            match flavor {
                FilterFlavor::Duckdb => {
                    values.push(QueryValue::String(value.clone()));
                    format!("{expression} {operator} ?")
                }
                FilterFlavor::Clickhouse => match chrono::DateTime::parse_from_rfc3339(value) {
                    Ok(value) => {
                        values.push(QueryValue::Int64(value.timestamp_micros()));
                        format!("{expression} {operator} fromUnixTimestamp64Micro(?)")
                    }
                    Err(_) => "1 = 0".to_string(),
                },
            }
        }
        Filter::String {
            operator, value, ..
        } => {
            let (pattern, comparison) = match operator {
                StringOp::Eq => (value.clone(), "="),
                StringOp::Contains => (format!("%{}%", escape_like_pattern(value)), "LIKE"),
                StringOp::StartsWith => (format!("{}%", escape_like_pattern(value)), "LIKE"),
                StringOp::EndsWith => (format!("%{}", escape_like_pattern(value)), "LIKE"),
            };
            values.push(QueryValue::String(pattern));
            let escape = match (flavor, operator) {
                (
                    FilterFlavor::Duckdb,
                    StringOp::Contains | StringOp::StartsWith | StringOp::EndsWith,
                ) => " ESCAPE '\\'",
                _ => "",
            };
            format!("{expression} {comparison} ?{escape}")
        }
        Filter::Number {
            operator, value, ..
        } => {
            let operator = match operator {
                NumberOp::Eq => "=",
                NumberOp::Gt => ">",
                NumberOp::Lt => "<",
                NumberOp::Gte => ">=",
                NumberOp::Lte => "<=",
            };
            let expression = match (flavor, target.column()) {
                (FilterFlavor::Clickhouse, Some(column))
                    if [
                        "gen_ai_cost_input",
                        "gen_ai_cost_output",
                        "gen_ai_cost_cache_read",
                        "gen_ai_cost_cache_write",
                        "gen_ai_cost_reasoning",
                        "gen_ai_cost_total",
                    ]
                    .contains(&column) =>
                {
                    format!("toFloat64({expression})")
                }
                _ => expression,
            };
            values.push(match flavor {
                FilterFlavor::Duckdb => QueryValue::String(value.to_string()),
                FilterFlavor::Clickhouse => QueryValue::Float64(*value),
            });
            format!("{expression} {operator} ?")
        }
        Filter::StringOptions {
            operator, value, ..
        } => {
            if value.is_empty() {
                return "1 = 1".to_string();
            }
            if target.column() == Some("tags") {
                values.extend(value.iter().cloned().map(QueryValue::String));
                return match flavor {
                    FilterFlavor::Duckdb => {
                        let tests = value
                            .iter()
                            .map(|_| {
                                let test = format!(
                                    "list_contains(from_json(ifnull({expression}, '[]'), \
                                     '[\"VARCHAR\"]'), ?)"
                                );
                                match operator {
                                    OptionsOp::AnyOf => test,
                                    OptionsOp::NoneOf => format!("NOT {test}"),
                                }
                            })
                            .collect::<Vec<_>>();
                        let join = match operator {
                            OptionsOp::AnyOf => " OR ",
                            OptionsOp::NoneOf => " AND ",
                        };
                        format!("({})", tests.join(join))
                    }
                    FilterFlavor::Clickhouse => {
                        let extracted =
                            format!("JSONExtract(ifNull({expression}, '[]'), 'Array(String)')");
                        let condition =
                            format!("hasAny({extracted}, [{}])", placeholders(value.len()));
                        match operator {
                            OptionsOp::AnyOf => condition,
                            OptionsOp::NoneOf => format!("NOT {condition}"),
                        }
                    }
                };
            }
            values.extend(value.iter().cloned().map(QueryValue::String));
            let operator = match operator {
                OptionsOp::AnyOf => "IN",
                OptionsOp::NoneOf => "NOT IN",
            };
            format!("{expression} {operator} ({})", placeholders(value.len()))
        }
        Filter::Boolean {
            operator, value, ..
        } => {
            let (operator, literal) = match flavor {
                FilterFlavor::Duckdb => (
                    match operator {
                        BooleanOp::Eq => "=",
                        BooleanOp::Ne => "<>",
                    },
                    if *value { "TRUE" } else { "FALSE" },
                ),
                FilterFlavor::Clickhouse => (
                    match operator {
                        BooleanOp::Eq => "=",
                        BooleanOp::Ne => "!=",
                    },
                    if *value { "true" } else { "false" },
                ),
            };
            format!("{expression} {operator} {literal}")
        }
        Filter::Null { operator, .. } => match operator {
            NullOp::IsNull => format!("{expression} IS NULL"),
            NullOp::IsNotNull => format!("{expression} IS NOT NULL"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use sideseat_ports::filters::{Filter, NumberOp, OptionsOp, StringOp};
    use sideseat_ports::types::ProjectId;

    #[test]
    fn span_point_read_declares_bind_order_once() {
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = span_by_id().render(backend);
            assert_eq!(
                query.bindings(),
                &[Binding::ProjectId, Binding::TraceId, Binding::SpanId]
            );
            assert_eq!(query.sql().matches('?').count(), 3);
        }
    }

    #[test]
    fn span_point_read_lowers_deduplication_as_a_capability() {
        let duckdb = span_by_id().render(Backend::Duckdb);
        assert!(duckdb.sql().contains("QUALIFY ROW_NUMBER()"));
        assert!(duckdb.sql().contains("rowid DESC"));
        assert!(!duckdb.sql().contains(" FINAL"));

        let clickhouse = span_by_id().render(Backend::Clickhouse);
        assert!(clickhouse.sql().contains("FROM otel_spans FINAL"));
        assert!(!clickhouse.sql().contains("QUALIFY"));
    }

    #[test]
    fn registry_is_derived_from_real_typed_operations() {
        assert_eq!(
            MIGRATED_OPERATIONS,
            &[
                span_by_id().operation(),
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
                QueryOperation::CountSpansByProject,
                QueryOperation::GetSpansForTrace,
                QueryOperation::GetEventsForSpan,
                QueryOperation::GetLinksForSpan,
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
            ]
        );
        assert_eq!(
            MIGRATED_OPERATIONS
                .iter()
                .map(|operation| operation.name())
                .collect::<Vec<_>>(),
            vec![
                "get_span",
                "list_spans",
                "list_traces",
                "delete_traces",
                "delete_spans",
                "delete_project_data",
                "upsert_spans",
                "upsert_metrics",
                "upsert_logs",
                "list_metrics",
                "get_metric",
                "aggregate_metrics",
                "get_metric_filter_options",
                "list_logs",
                "get_log",
                "get_log_filter_options",
                "get_trace_session_pairs",
                "get_session_ids_for_traces",
                "get_trace_ids_for_sessions",
                "get_span_counts_bulk",
                "traces_without_spans",
                "file_reference_fields_for_traces",
                "span_body_fields_for_traces",
                "span_body_backfill_page",
                "count_project_rows",
                "count_spans_by_project",
                "get_spans_for_trace",
                "get_events_for_span",
                "get_links_for_span",
                "get_trace",
                "get_traces_for_session",
                "get_session",
                "list_sessions",
                "get_feed_spans",
                "get_trace_filter_options",
                "get_trace_tags_options",
                "get_span_filter_options",
                "get_session_filter_options",
                "get_messages",
                "get_project_messages",
                "get_project_stats",
                "max_ingested_at_us",
                "enforce_retention",
                "delete_sessions",
            ]
        );
    }

    #[test]
    fn session_page_keeps_values_bound_and_entity_negations_canonical() {
        let params = ListSessionsParams {
            project_id: ProjectId::from("tenant-'quoted"),
            page: 2,
            limit: 17,
            user_id: Some("user-'quoted".to_string()),
            environment: Some(vec!["prod".to_string(), "stage".to_string()]),
            from_timestamp: Some(
                DateTime::parse_from_rfc3339("2026-09-20T10:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            to_timestamp: Some(
                DateTime::parse_from_rfc3339("2026-09-21T10:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            filters: vec![
                Filter::StringOptions {
                    column: "session_id".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["session-'forbidden".to_string()],
                },
                Filter::StringOptions {
                    column: "gen_ai_request_model".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["model-'forbidden".to_string()],
                },
                Filter::Number {
                    column: "gen_ai_cost_total".to_string(),
                    operator: NumberOp::Gt,
                    value: 1.25,
                },
            ],
            ..Default::default()
        };

        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let page = list_sessions(&params, backend);
            for query in [&page.count, &page.rows] {
                assert_eq!(query.sql().matches('?').count(), query.params().len());
                assert!(!query.sql().contains("tenant-'quoted"));
                assert!(!query.sql().contains("user-'quoted"));
                assert!(!query.sql().contains("forbidden"));
            }
            assert!(page.rows.sql().contains("sp.trace_id NOT IN"));
            assert!(page.rows.sql().contains("sp.session_id NOT IN"));
            assert!(page.rows.sql().contains("n.gen_ai_request_model IN (?)"));
            assert!(page.rows.sql().contains("LIMIT 17 OFFSET 17"));
        }

        let duckdb = list_sessions(&params, Backend::Duckdb);
        assert!(
            duckdb
                .rows
                .params()
                .iter()
                .any(|value| matches!(value, QueryValue::String(value) if value == "1.25"))
        );
        let clickhouse = list_sessions(&params, Backend::Clickhouse);
        assert!(
            clickhouse
                .rows
                .params()
                .iter()
                .any(|value| matches!(value, QueryValue::Float64(value) if *value == 1.25))
        );
        assert!(
            clickhouse
                .rows
                .params()
                .iter()
                .any(|value| matches!(value, QueryValue::Int64(_)))
        );
    }

    #[test]
    fn feed_page_puts_watermark_and_cursor_values_in_total_key_order() {
        let params = FeedSpansParams {
            project_id: ProjectId::from("tenant-'quoted"),
            limit: 13,
            cursor: Some((77, "span-'quoted".to_string(), "trace-'quoted".to_string())),
            start_time: Some(
                DateTime::parse_from_rfc3339("2026-09-20T10:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            end_time: Some(
                DateTime::parse_from_rfc3339("2026-09-21T10:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            is_observation: Some(true),
            ingested_before_us: Some(999),
        };

        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = feed_spans(&params, backend);
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("span-'quoted"));
            assert!(!query.sql().contains("trace-'quoted"));
            assert_eq!(query.params().first(), Some(&QueryValue::Int64(999)));
            assert_eq!(
                query.params().get(1),
                Some(&QueryValue::String("tenant-'quoted".to_string()))
            );
            assert_eq!(query.params().get(2), Some(&QueryValue::Int64(77)));
            assert!(
                query
                    .sql()
                    .contains("ORDER BY ingested_at DESC, span_id DESC, trace_id DESC")
            );
            assert!(query.sql().ends_with("LIMIT 13"));
            assert!(query.sql().contains("AS ingested_at_us"));
        }

        let duckdb = feed_spans(&params, Backend::Duckdb);
        assert!(duckdb.sql().contains("QUALIFY ROW_NUMBER()"));
        assert!(duckdb.sql().contains("rowid DESC"));
        assert!(matches!(
            duckdb.params().get(5),
            Some(QueryValue::String(value)) if value.starts_with("2026-09-20")
        ));

        let clickhouse = feed_spans(&params, Backend::Clickhouse);
        assert!(
            clickhouse
                .sql()
                .contains("LIMIT 1 BY project_id, trace_id, span_id")
        );
        assert!(matches!(
            clickhouse.params().get(5),
            Some(QueryValue::Int64(_))
        ));
    }

    #[test]
    fn filter_option_queries_validate_columns_and_read_winning_entities() {
        let from = DateTime::parse_from_rfc3339("2026-09-20T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2026-09-21T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let requested = vec![
            "environment".to_string(),
            "session_id".to_string(),
            "trace_name".to_string(),
            "environment; SELECT secret".to_string(),
        ];

        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let trace = trace_filter_options(
                "tenant-'quoted",
                &requested,
                Some(&from),
                Some(&to),
                backend,
            );
            assert_eq!(trace.len(), 3);
            let span = span_filter_options(
                "tenant-'quoted",
                &["status_code".to_string(), "bad column".to_string()],
                Some(&from),
                Some(&to),
                true,
                backend,
            );
            assert_eq!(span.len(), 1);
            let session = session_filter_options(
                "tenant-'quoted",
                &["environment".to_string(), "bad column".to_string()],
                Some(&from),
                Some(&to),
                backend,
            );
            assert_eq!(session.len(), 1);
            let tags = trace_tag_options("tenant-'quoted", Some(&from), Some(&to), backend);

            for query in trace
                .iter()
                .map(|option| &option.query)
                .chain(span.iter().map(|option| &option.query))
                .chain(session.iter().map(|option| &option.query))
                .chain(std::iter::once(&tags))
            {
                assert_eq!(query.sql().matches('?').count(), query.params().len());
                assert!(!query.sql().contains("tenant-'quoted"));
                assert!(!query.sql().contains("bad column"));
                assert!(!query.sql().contains("SELECT secret"));
                match backend {
                    Backend::Duckdb => {
                        assert!(query.sql().contains("QUALIFY ROW_NUMBER()"));
                        assert!(query.sql().contains("rowid DESC"));
                        assert!(matches!(
                            query.params().get(1),
                            Some(QueryValue::String(value)) if value.starts_with("2026-09-20")
                        ));
                    }
                    Backend::Clickhouse => {
                        assert!(query.sql().contains("FINAL"));
                        assert!(
                            query.sql().contains("toNullable("),
                            "filter-option values must match the adapter's Nullable(String) row"
                        );
                        assert!(
                            !query.sql().contains("FINAL s")
                                && !query.sql().contains("FINAL sp")
                                && !query.sql().contains("FINAL g"),
                            "ClickHouse requires table aliases before FINAL; winner relations use subqueries"
                        );
                        assert!(matches!(query.params().get(1), Some(QueryValue::Int64(_))));
                    }
                    Backend::Sqlite | Backend::Postgres => unreachable!(),
                }
            }

            assert!(session[0].query.sql().contains("COUNT(DISTINCT cts."));
            assert!(!session[0].query.sql().contains("s.session_id IS NOT NULL"));
            assert!(
                span[0]
                    .query
                    .sql()
                    .contains("gen_ai_request_model IS NOT NULL")
            );
        }
    }

    #[test]
    fn filtered_span_page_keeps_values_out_of_sql_and_in_driver_types() {
        let params = ListSpansParams {
            project_id: ProjectId::from("tenant-with-'quotes"),
            trace_id: Some("trace-value".to_string()),
            environment: Some(vec!["prod".to_string(), "stage".to_string()]),
            filters: vec![
                Filter::String {
                    column: "span_name".to_string(),
                    operator: StringOp::Contains,
                    value: "50%_'quoted".to_string(),
                },
                Filter::Number {
                    column: "gen_ai_cost_total".to_string(),
                    operator: NumberOp::Gt,
                    value: 1.25,
                },
            ],
            limit: 25,
            ..Default::default()
        };

        let duckdb = list_spans(&params, Backend::Duckdb);
        let clickhouse = list_spans(&params, Backend::Clickhouse);

        for page in [&duckdb, &clickhouse] {
            assert!(!page.rows.sql().contains("tenant-with-"));
            assert!(!page.rows.sql().contains("trace-value"));
            assert!(!page.rows.sql().contains("quoted"));
            assert_eq!(page.count.params(), page.rows.params());
            assert!(page.rows.sql().ends_with("LIMIT 25 OFFSET 0"));
        }
        assert!(matches!(
            duckdb.rows.params().last(),
            Some(QueryValue::String(value)) if value == "1.25"
        ));
        assert!(matches!(
            clickhouse.rows.params().last(),
            Some(QueryValue::Float64(value)) if *value == 1.25
        ));
        assert!(duckdb.rows.sql().contains("ESCAPE '\\'"));
        assert!(
            clickhouse
                .rows
                .sql()
                .contains("toFloat64(gen_ai_cost_total) > ?")
        );
    }

    #[test]
    fn session_parameter_uses_the_canonical_trace_membership_relation() {
        let params = ListSpansParams {
            project_id: ProjectId::from("p"),
            session_id: Some("session-a".to_string()),
            limit: 10,
            ..Default::default()
        };

        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let page = list_spans(&params, backend);
            assert!(page.rows.sql().contains("arg_") || page.rows.sql().contains("argMin"));
            assert!(page.rows.sql().contains("canonical_session = ?"));
            assert_eq!(page.rows.params().len(), 5);
            assert_eq!(
                page.rows.params(),
                &[
                    QueryValue::String("p".to_string()),
                    QueryValue::String("p".to_string()),
                    QueryValue::String("p".to_string()),
                    QueryValue::String("session-a".to_string()),
                    QueryValue::String("session-a".to_string()),
                ]
            );
        }
    }

    #[test]
    fn trace_page_keeps_every_value_bound_in_statement_order() {
        let params = ListTracesParams {
            project_id: ProjectId::from("tenant-with-'quotes"),
            user_id: Some("user-'quoted".to_string()),
            environment: Some(vec!["prod".to_string(), "stage".to_string()]),
            from_timestamp: Some(
                "2026-01-02T03:04:05Z"
                    .parse::<DateTime<Utc>>()
                    .expect("valid timestamp"),
            ),
            to_timestamp: Some(
                "2026-02-03T04:05:06Z"
                    .parse::<DateTime<Utc>>()
                    .expect("valid timestamp"),
            ),
            filters: vec![
                Filter::String {
                    column: "trace_name".to_string(),
                    operator: StringOp::Contains,
                    value: "50%_'quoted".to_string(),
                },
                Filter::Number {
                    column: "total_cost".to_string(),
                    operator: NumberOp::Gt,
                    value: 1.25,
                },
            ],
            page: 2,
            limit: 25,
            ..Default::default()
        };

        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let page = list_traces(&params, backend);
            for query in [&page.count, &page.rows] {
                assert_eq!(query.sql().matches('?').count(), query.params().len());
                assert!(!query.sql().contains("tenant-with-"));
                assert!(!query.sql().contains("user-'quoted"));
                assert!(!query.sql().contains("50%_'quoted"));
                assert!(!query.sql().contains("1.25"));
            }
            assert!(page.rows.sql().contains("LIMIT 25 OFFSET 25"));
        }
    }

    #[test]
    fn trace_aggregate_filters_preserve_backend_driver_types() {
        let params = ListTracesParams {
            project_id: ProjectId::from("p"),
            filters: vec![Filter::Number {
                column: "total_tokens".to_string(),
                operator: NumberOp::Gte,
                value: 42.5,
            }],
            limit: 10,
            ..Default::default()
        };

        let duckdb = list_traces(&params, Backend::Duckdb);
        let clickhouse = list_traces(&params, Backend::Clickhouse);

        assert!(
            duckdb
                .rows
                .sql()
                .contains("COALESCE(MAX(gtf.total_tokens), 0)")
        );
        assert!(
            duckdb
                .rows
                .params()
                .iter()
                .any(|value| matches!(value, QueryValue::String(value) if value == "42.5"))
        );
        assert!(
            clickhouse
                .rows
                .sql()
                .contains("COALESCE(max(gtf.total_tokens), 0)")
        );
        assert!(
            clickhouse
                .rows
                .params()
                .iter()
                .any(|value| matches!(value, QueryValue::Float64(value) if *value == 42.5))
        );
    }

    #[test]
    fn negated_trace_session_filter_uses_canonical_entity_complement() {
        let params = ListTracesParams {
            project_id: ProjectId::from("p"),
            filters: vec![Filter::StringOptions {
                column: "session_id".to_string(),
                operator: OptionsOp::NoneOf,
                value: vec!["session-a".to_string(), "session-b".to_string()],
            }],
            limit: 10,
            ..Default::default()
        };

        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let page = list_traces(&params, backend);
            for query in [&page.count, &page.rows] {
                assert!(query.sql().contains("NOT IN"));
                assert!(query.sql().contains("arg_min") || query.sql().contains("argMin"));
                assert!(!query.sql().contains("session-a"));
                assert_eq!(query.sql().matches('?').count(), query.params().len());
            }
        }
    }

    #[test]
    fn membership_reads_put_the_watermark_before_tenant_and_id_values() {
        let trace_ids = vec!["trace-'one".to_string(), "trace-two".to_string()];
        let session_ids = vec!["session-'one".to_string(), "session-two".to_string()];

        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let pairs = trace_session_pairs("tenant-'quoted", &trace_ids, Some(123), backend)
                .expect("pairs");
            let sessions = session_ids_for_traces("tenant-'quoted", &trace_ids, Some(123), backend)
                .expect("sessions");
            let traces = trace_ids_for_sessions("tenant-'quoted", &session_ids, Some(123), backend)
                .expect("traces");
            for query in [&pairs, &sessions, &traces] {
                assert_eq!(query.sql().matches('?').count(), query.params().len());
                assert!(!query.sql().contains("tenant-'quoted"));
                assert!(!query.sql().contains("trace-'one"));
                assert!(!query.sql().contains("session-'one"));
            }
            match backend {
                Backend::Duckdb => {
                    assert_eq!(
                        pairs.params().first(),
                        Some(&QueryValue::String("tenant-'quoted".to_string()))
                    );
                    assert_eq!(
                        pairs.params().get(1),
                        Some(&QueryValue::String("123".to_string()))
                    );
                    assert!(pairs.sql().contains("WHERE project_id = ? AND EPOCH_US"));
                    for query in [&sessions, &traces] {
                        assert_eq!(
                            query.params().first(),
                            Some(&QueryValue::String("123".to_string()))
                        );
                    }
                }
                Backend::Clickhouse => {
                    for query in [&pairs, &sessions, &traces] {
                        assert_eq!(query.params().first(), Some(&QueryValue::Int64(123)));
                    }
                }
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            }
            for query in [&sessions, &traces] {
                assert_eq!(
                    query.params().get(1),
                    Some(&QueryValue::String("tenant-'quoted".to_string()))
                );
            }
        }
    }

    #[test]
    fn membership_reads_use_canonical_earliest_span_semantics() {
        let trace_ids = vec!["trace".to_string()];
        let sessions = vec!["session".to_string()];
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let pairs = trace_session_pairs("p", &trace_ids, None, backend).expect("trace pairs");
            let reverse =
                trace_ids_for_sessions("p", &sessions, None, backend).expect("session traces");
            for query in [&pairs, &reverse] {
                assert!(query.sql().contains("timestamp_start, span_id"));
                assert!(
                    query.sql().contains("canonical_session") || query.sql().contains("AS session")
                );
            }
            match backend {
                Backend::Duckdb => assert!(reverse.sql().contains("ROW_NUMBER()")),
                Backend::Clickhouse => assert!(reverse.sql().contains("FINAL")),
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            }
        }
    }

    #[test]
    fn bulk_span_counts_use_winning_rows_and_tuple_bindings() {
        let spans = vec![
            ("trace-'a".to_string(), "span-a".to_string()),
            ("trace-b".to_string(), "span-b".to_string()),
        ];
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = span_counts_bulk("tenant-'quoted", &spans, backend).expect("counts");
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert_eq!(query.params().len(), 5);
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("trace-'a"));
            match backend {
                Backend::Duckdb => {
                    assert!(query.sql().contains("QUALIFY ROW_NUMBER()"));
                    assert!(query.sql().contains("json_array_length"));
                }
                Backend::Clickhouse => {
                    assert!(query.sql().contains("FROM otel_spans FINAL"));
                    assert!(query.sql().contains("JSONLength"));
                }
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            }
        }
    }

    #[test]
    fn cleanup_trace_reads_share_winner_and_tenant_scoping() {
        let traces = vec!["trace-'a".to_string(), "trace-b".to_string()];
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let alive = surviving_trace_ids("tenant-'quoted", &traces, backend).expect("alive");
            let fields = file_reference_fields("tenant-'quoted", &traces, backend).expect("fields");
            for query in [&alive, &fields] {
                assert_eq!(query.sql().matches('?').count(), query.params().len());
                assert!(query.sql().contains("project_id = ?"));
                assert!(!query.sql().contains("tenant-'quoted"));
                assert!(!query.sql().contains("trace-'a"));
            }
            match backend {
                Backend::Duckdb => assert!(alive.sql().contains("QUALIFY ROW_NUMBER()")),
                Backend::Clickhouse => {
                    assert!(alive.sql().contains("FROM otel_spans FINAL"));
                    assert!(fields.sql().contains("coalesce(messages, '')"));
                }
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            }
        }
    }

    #[test]
    fn maintenance_count_plans_are_explicit_and_parameterized() {
        let duckdb = project_row_count("tenant-'quoted", Backend::Duckdb, None);
        assert!(duckdb.metrics_table_exists.is_none());
        let clickhouse = project_row_count(
            "tenant-'quoted",
            Backend::Clickhouse,
            Some("otel_metrics_local"),
        );
        assert!(clickhouse.metrics_table_exists.is_some());
        for query in [
            &duckdb.spans,
            &duckdb.metrics,
            &clickhouse.spans,
            clickhouse
                .metrics_table_exists
                .as_ref()
                .expect("table existence"),
            &clickhouse.metrics,
        ] {
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-'quoted"));
        }

        let projects = vec!["tenant-'one".to_string(), "tenant-two".to_string()];
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = span_counts_by_project(&projects, backend).expect("project counts");
            assert_eq!(query.sql().matches('?').count(), 2);
            assert_eq!(query.params().len(), 2);
            assert!(!query.sql().contains("tenant-'one"));
        }
        assert!(span_counts_by_project(&[], Backend::Duckdb).is_none());
    }

    #[test]
    fn trace_span_detail_read_reuses_projection_and_winner_capabilities() {
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = spans_for_trace("tenant-'quoted", "trace-'quoted", backend);
            assert_eq!(query.sql().matches('?').count(), 2);
            assert_eq!(query.params().len(), 2);
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("trace-'quoted"));
            assert!(query.sql().contains("scope_name"));
            assert!(query.sql().contains("ORDER BY timestamp_start"));
            match backend {
                Backend::Duckdb => assert!(query.sql().contains("QUALIFY ROW_NUMBER()")),
                Backend::Clickhouse => assert!(query.sql().contains("FROM otel_spans FINAL")),
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            }
        }
    }

    #[test]
    fn point_span_json_reads_bind_identity_and_preserve_array_order() {
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let events =
                events_for_span("tenant-'quoted", "trace-'quoted", "span-'quoted", backend);
            let links = links_for_span("tenant-'quoted", "trace-'quoted", "span-'quoted", backend);
            for query in [&events, &links] {
                assert_eq!(query.sql().matches('?').count(), 3);
                assert_eq!(query.params().len(), 3);
                assert!(!query.sql().contains("tenant-'quoted"));
                assert!(!query.sql().contains("trace-'quoted"));
                assert!(!query.sql().contains("span-'quoted"));
            }
            assert!(events.sql().contains("event_index"));
            match backend {
                Backend::Duckdb => {
                    assert!(events.sql().contains("rowid DESC"));
                    assert!(links.sql().contains("WITH ORDINALITY"));
                }
                Backend::Clickhouse => {
                    assert!(events.sql().contains("FROM otel_spans FINAL"));
                    assert!(links.sql().contains("JSONLength(raw_span, 'links')"));
                }
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            }
        }
    }

    #[test]
    fn trace_point_aggregate_reuses_list_projection_and_token_dedup() {
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = trace_by_id("tenant-'quoted", "trace-'quoted", backend);
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("trace-'quoted"));
            assert!(query.sql().contains("total_tokens"));
            assert!(query.sql().contains("input_preview"));
            match backend {
                Backend::Duckdb => {
                    assert_eq!(query.params().len(), 4);
                    assert!(query.sql().contains("QUALIFY ROW_NUMBER()"));
                    assert!(query.sql().contains("timestamp_start, s.span_id"));
                    assert!(query.sql().contains("NOT EXISTS"));
                }
                Backend::Clickhouse => {
                    assert_eq!(query.params().len(), 6);
                    assert!(query.sql().contains("timestamp_start, s.span_id"));
                    assert!(query.sql().contains("dedup_lookup AS"));
                    assert!(query.sql().contains("NOT IN"));
                }
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            }
        }
    }

    #[test]
    fn session_trace_aggregate_composes_canonical_membership_and_shared_projection() {
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = traces_for_session("tenant-'quoted", "session-'quoted", backend);
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("session-'quoted"));
            assert!(query.sql().contains("canonical_session = ?"));
            assert!(query.sql().contains("input_preview"));
            assert!(query.sql().contains("total_tokens"));
            assert!(query.sql().contains("ORDER BY"));
            match backend {
                Backend::Duckdb => assert_eq!(query.params().len(), 6),
                Backend::Clickhouse => {
                    assert_eq!(query.params().len(), 7);
                    assert!(query.sql().contains("dedup_lookup AS"));
                }
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            }
        }
    }

    #[test]
    fn session_point_aggregate_uses_canonical_membership_and_shared_totals() {
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = session_by_id("tenant-'quoted", "session-'quoted", backend);
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("session-'quoted"));
            assert!(query.sql().contains("canonical_session = ?"));
            assert!(query.sql().contains("gen_totals_by_trace"));
            assert!(query.sql().contains("session_totals"));
            assert!(query.sql().contains("COUNT(DISTINCT s.trace_id)"));
            match backend {
                Backend::Duckdb => assert_eq!(query.params().len(), 7),
                Backend::Clickhouse => {
                    assert_eq!(query.params().len(), 8);
                    assert!(query.sql().contains("dedup_lookup AS"));
                }
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            }
        }
    }

    #[test]
    fn pressure_candidates_are_winning_held_aware_bounded_and_parameterized() {
        let now = chrono::DateTime::from_timestamp(1_795_000_000, 0).unwrap();
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = oldest_reclaimable_spans("tenant-'quoted", backend, 10_000, now, 100);
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(query.sql().contains("hold_until IS NULL OR hold_until < ?"));
            assert!(query.sql().contains("bytes_before < ?"));
            assert!(query.sql().contains("pressure_rank <= ?"));
            match backend {
                Backend::Duckdb => assert!(query.sql().contains("QUALIFY ROW_NUMBER()")),
                Backend::Clickhouse => assert!(query.sql().contains("otel_spans FINAL")),
                Backend::Sqlite | Backend::Postgres => unreachable!(),
            }
        }
    }
}
