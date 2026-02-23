//! Query repository for OTEL API queries (ClickHouse backend)
//!
//! Provides the same interface as DuckDB query repository but uses ClickHouse SQL.

use chrono::{DateTime, Utc};
use clickhouse::{Client, Row};
use serde::Deserialize;

// ============================================================================
// Token Dedup SQL Fragments
// ============================================================================
// ClickHouse does not support correlated NOT EXISTS subqueries on the same table
// (always evaluates EXISTS as true). Use materialized CTE + NOT IN instead.

/// Build the dedup_lookup CTE with optional extra WHERE conditions.
///
/// Always requires 1 bind parameter: project_id.
/// `extra_where` is appended to narrow the materialized set:
/// - `""` for list queries (full project scan — required for correctness)
/// - `"trace_id = ?"` for single-trace queries (+1 bind param)
/// - `"trace_id IN (SELECT trace_id FROM session_traces)"` for session queries (no extra binds)
pub(super) fn build_dedup_lookup_cte(extra_where: &str) -> String {
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
          AND (gen_ai_usage_input_tokens + gen_ai_usage_output_tokens) > 0{extra}
    )"#
    )
}

/// Anti-join condition for gen_totals, replacing 3 correlated NOT EXISTS subqueries.
/// Requires dedup_lookup CTE to be defined earlier in the WITH clause.
///
/// Each subquery is scoped to the same trace as the outer row via tuple NOT IN
/// (e.g. `(g.trace_id, g.span_id) NOT IN (SELECT trace_id, parent_span_id ...)`)
/// to maintain correctness without correlation: generation spans in trace A must
/// not affect token dedup for unrelated trace B. ClickHouse does not support
/// correlated subqueries inside IN/NOT IN (Code 48).
pub(super) const TOKEN_DEDUP_CONDITION: &str = r#"(
                  (g.observation_type = 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND (g.trace_id, g.span_id) NOT IN (
                       SELECT trace_id, parent_span_id FROM dedup_lookup
                       WHERE observation_type = 'generation'
                         AND parent_span_id IS NOT NULL
                   ))
                  OR
                  (g.observation_type != 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND g.trace_id NOT IN (
                       SELECT DISTINCT trace_id FROM dedup_lookup
                       WHERE observation_type = 'generation'
                   )
                   AND (g.parent_span_id IS NULL OR (g.trace_id, g.parent_span_id) NOT IN (
                       SELECT trace_id, span_id FROM dedup_lookup
                   )))
              )"#;

/// Build a dedup CTE scoped to traces that have at least one span in the
/// given time range. The CTE still loads ALL spans for those traces (no time
/// filter on the CTE itself) so that generation spans outside the time window
/// are still considered for token dedup.
///
/// Returns `(cte_sql, extra_bind_params)`. The extra params must be bound
/// between the base CTE project_id param and the rest of the query params.
///
/// When no time filters are provided, falls back to the unscoped CTE.
pub(super) fn build_time_scoped_dedup(
    project_id: &str,
    from: Option<&DateTime<Utc>>,
    to: Option<&DateTime<Utc>>,
) -> (String, Vec<QueryParam>) {
    if from.is_none() && to.is_none() {
        return (build_dedup_lookup_cte(""), vec![]);
    }

    let mut time_conds = vec!["project_id = ?".to_string()];
    let mut extra_binds = vec![QueryParam::String(project_id.to_string())];

    if let Some(f) = from {
        time_conds.push("timestamp_start >= fromUnixTimestamp64Micro(?)".to_string());
        extra_binds.push(QueryParam::Int64(f.timestamp_micros()));
    }
    if let Some(t) = to {
        time_conds.push("timestamp_start <= fromUnixTimestamp64Micro(?)".to_string());
        extra_binds.push(QueryParam::Int64(t.timestamp_micros()));
    }

    let scope = format!(
        "trace_id IN (SELECT DISTINCT trace_id FROM otel_spans WHERE {})",
        time_conds.join(" AND ")
    );

    (build_dedup_lookup_cte(&scope), extra_binds)
}

// ============================================================================
// Parameterized Query Builder
// ============================================================================

/// Query parameter that can be bound to ClickHouse queries.
/// All user-controllable values MUST go through this enum for SQL injection safety.
#[derive(Clone)]
pub(super) enum QueryParam {
    /// String parameter (bound as-is)
    String(String),
    /// Integer parameter (used for timestamps as microseconds)
    Int64(i64),
}

/// Builder for constructing parameterized SQL WHERE clauses.
///
/// Collects conditions and their parameter values, then allows binding
/// all parameters to a ClickHouse query in order.
///
/// # SQL Injection Safety
/// All values that could potentially come from user input are parameterized.
/// Table names and column names are NOT parameterized but are validated
/// against whitelists before use.
#[derive(Default)]
struct ConditionBuilder {
    /// SQL conditions (public for special cases like tuple comparisons)
    pub conditions: Vec<String>,
    /// Parameter values to bind (public for special cases)
    pub params: Vec<QueryParam>,
}

impl ConditionBuilder {
    fn new() -> Self {
        Self::default()
    }

    /// Add an equality condition: `column = ?`
    fn add_eq(&mut self, column: &str, value: &str) {
        self.conditions.push(format!("{} = ?", column));
        self.params.push(QueryParam::String(value.to_string()));
    }

    /// Add an IN condition: `column IN (?, ?, ...)`
    fn add_in(&mut self, column: &str, values: &[String]) {
        if values.is_empty() {
            return;
        }
        let placeholders: Vec<&str> = values.iter().map(|_| "?").collect();
        self.conditions
            .push(format!("{} IN ({})", column, placeholders.join(", ")));
        for v in values {
            self.params.push(QueryParam::String(v.clone()));
        }
    }

    /// Add a raw condition without parameters (for static conditions only)
    ///
    /// # Safety
    /// The condition string must NOT contain any user input.
    fn add_raw(&mut self, condition: &str) {
        self.conditions.push(condition.to_string());
    }

    /// Add a timestamp >= condition using parameterized microseconds
    ///
    /// Uses `fromUnixTimestamp64Micro(?)` for type-safe binding.
    fn add_timestamp_gte(&mut self, column: &str, ts: &DateTime<Utc>) {
        self.conditions
            .push(format!("{} >= fromUnixTimestamp64Micro(?)", column));
        self.params.push(QueryParam::Int64(ts.timestamp_micros()));
    }

    /// Add a timestamp <= condition using parameterized microseconds
    fn add_timestamp_lte(&mut self, column: &str, ts: &DateTime<Utc>) {
        self.conditions
            .push(format!("{} <= fromUnixTimestamp64Micro(?)", column));
        self.params.push(QueryParam::Int64(ts.timestamp_micros()));
    }

    /// Add a timestamp < condition using parameterized microseconds
    fn add_timestamp_lt(&mut self, column: &str, ts: &DateTime<Utc>) {
        self.conditions
            .push(format!("{} < fromUnixTimestamp64Micro(?)", column));
        self.params.push(QueryParam::Int64(ts.timestamp_micros()));
    }

    /// Build the WHERE clause (without "WHERE" keyword)
    fn build(&self) -> String {
        self.conditions.join(" AND ")
    }

    /// Bind all collected parameters to a query.
    /// Returns a query ready for execution.
    fn bind_to(&self, mut query: clickhouse::query::Query) -> clickhouse::query::Query {
        for param in &self.params {
            query = match param {
                QueryParam::String(s) => query.bind(s),
                QueryParam::Int64(i) => query.bind(i),
            };
        }
        query
    }

    /// Bind parameters multiple times (for queries with repeated WHERE clauses in CTEs)
    fn bind_to_n(
        &self,
        mut query: clickhouse::query::Query,
        times: usize,
    ) -> clickhouse::query::Query {
        for _ in 0..times {
            for param in &self.params {
                query = match param {
                    QueryParam::String(s) => query.bind(s),
                    QueryParam::Int64(i) => query.bind(i),
                };
            }
        }
        query
    }
}

use crate::core::constants::{QUERY_MAX_FILTER_SUGGESTIONS, QUERY_MAX_SPANS_PER_TRACE};
use crate::data::clickhouse::ClickhouseError;
use crate::data::types::{
    EventRow, FeedSpansParams, LinkRow, ListSessionsParams, ListSpansParams, ListTracesParams,
    SessionRow, SpanRow, TraceRow, deduplicate_spans, parse_finish_reasons, parse_tags,
};
use crate::utils::time::parse_iso_timestamp;

/// ClickHouse row for trace queries
#[derive(Row, Deserialize)]
struct ChTraceRow {
    trace_id: String,
    trace_name: Option<String>,
    start_time: i64,
    end_time: i64,
    duration_ms: i64,
    session_id: Option<String>,
    user_id: Option<String>,
    environment: Option<String>,
    span_count: u64,
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    input_cost: f64,
    output_cost: f64,
    cache_read_cost: f64,
    cache_write_cost: f64,
    reasoning_cost: f64,
    total_cost: f64,
    tags: Option<String>,
    observation_count: u64,
    metadata: Option<String>,
    input_preview: Option<String>,
    output_preview: Option<String>,
    has_error: u8,
}

impl From<ChTraceRow> for TraceRow {
    fn from(row: ChTraceRow) -> Self {
        Self {
            trace_id: row.trace_id,
            trace_name: row.trace_name,
            start_time: DateTime::from_timestamp_micros(row.start_time)
                .unwrap_or(DateTime::UNIX_EPOCH),
            end_time: Some(
                DateTime::from_timestamp_micros(row.end_time).unwrap_or(DateTime::UNIX_EPOCH),
            ),
            duration_ms: Some(row.duration_ms),
            session_id: row.session_id,
            user_id: row.user_id,
            environment: row.environment,
            span_count: row.span_count as i64,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            total_tokens: row.total_tokens,
            cache_read_tokens: row.cache_read_tokens,
            cache_write_tokens: row.cache_write_tokens,
            reasoning_tokens: row.reasoning_tokens,
            input_cost: row.input_cost,
            output_cost: row.output_cost,
            cache_read_cost: row.cache_read_cost,
            cache_write_cost: row.cache_write_cost,
            reasoning_cost: row.reasoning_cost,
            total_cost: row.total_cost,
            tags: parse_tags(&row.tags),
            observation_count: row.observation_count as i64,
            metadata: row.metadata,
            input_preview: row.input_preview,
            output_preview: row.output_preview,
            has_error: row.has_error != 0,
        }
    }
}

/// ClickHouse row for span queries
#[derive(Row, Deserialize)]
struct ChSpanRow {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    span_name: Option<String>,
    span_kind: Option<String>,
    span_category: Option<String>,
    observation_type: Option<String>,
    framework: Option<String>,
    status_code: Option<String>,
    start_time: i64,
    end_time: Option<i64>,
    duration_ms: Option<i64>,
    environment: Option<String>,
    resource_attributes: Option<String>,
    session_id: Option<String>,
    user_id: Option<String>,
    gen_ai_system: Option<String>,
    gen_ai_request_model: Option<String>,
    gen_ai_agent_name: Option<String>,
    gen_ai_finish_reasons: Option<String>,
    gen_ai_usage_input_tokens: i64,
    gen_ai_usage_output_tokens: i64,
    gen_ai_usage_total_tokens: i64,
    gen_ai_usage_cache_read_tokens: i64,
    gen_ai_usage_cache_write_tokens: i64,
    gen_ai_usage_reasoning_tokens: i64,
    gen_ai_cost_input: f64,
    gen_ai_cost_output: f64,
    gen_ai_cost_cache_read: f64,
    gen_ai_cost_cache_write: f64,
    gen_ai_cost_reasoning: f64,
    gen_ai_cost_total: f64,
    gen_ai_usage_details: Option<String>,
    metadata: Option<String>,
    attributes: Option<String>,
    input_preview: Option<String>,
    output_preview: Option<String>,
    raw_span: Option<String>,
    ingested_at_us: i64,
}

impl From<ChSpanRow> for SpanRow {
    fn from(row: ChSpanRow) -> Self {
        Self {
            trace_id: row.trace_id,
            span_id: row.span_id,
            parent_span_id: row.parent_span_id,
            span_name: row.span_name,
            span_kind: row.span_kind,
            span_category: row.span_category,
            observation_type: row.observation_type,
            framework: row.framework,
            status_code: row.status_code,
            timestamp_start: DateTime::from_timestamp_micros(row.start_time)
                .unwrap_or(DateTime::UNIX_EPOCH),
            timestamp_end: row.end_time.and_then(DateTime::from_timestamp_micros),
            duration_ms: row.duration_ms,
            environment: row.environment,
            resource_attributes: row.resource_attributes,
            session_id: row.session_id,
            user_id: row.user_id,
            gen_ai_system: row.gen_ai_system,
            gen_ai_request_model: row.gen_ai_request_model,
            gen_ai_agent_name: row.gen_ai_agent_name,
            gen_ai_finish_reasons: parse_finish_reasons(&row.gen_ai_finish_reasons),
            gen_ai_usage_input_tokens: row.gen_ai_usage_input_tokens,
            gen_ai_usage_output_tokens: row.gen_ai_usage_output_tokens,
            gen_ai_usage_total_tokens: row.gen_ai_usage_total_tokens,
            gen_ai_usage_cache_read_tokens: row.gen_ai_usage_cache_read_tokens,
            gen_ai_usage_cache_write_tokens: row.gen_ai_usage_cache_write_tokens,
            gen_ai_usage_reasoning_tokens: row.gen_ai_usage_reasoning_tokens,
            gen_ai_cost_input: row.gen_ai_cost_input,
            gen_ai_cost_output: row.gen_ai_cost_output,
            gen_ai_cost_cache_read: row.gen_ai_cost_cache_read,
            gen_ai_cost_cache_write: row.gen_ai_cost_cache_write,
            gen_ai_cost_reasoning: row.gen_ai_cost_reasoning,
            gen_ai_cost_total: row.gen_ai_cost_total,
            gen_ai_usage_details: row.gen_ai_usage_details,
            metadata: row.metadata,
            attributes: row.attributes,
            input_preview: row.input_preview,
            output_preview: row.output_preview,
            raw_span: row.raw_span,
            ingested_at: DateTime::from_timestamp_micros(row.ingested_at_us)
                .unwrap_or(DateTime::UNIX_EPOCH),
        }
    }
}

/// ClickHouse row for session queries
#[derive(Row, Deserialize)]
struct ChSessionRow {
    session_id: Option<String>,
    user_id: Option<String>,
    environment: Option<String>,
    start_time: i64,
    end_time: i64,
    trace_count: u64,
    span_count: u64,
    observation_count: u64,
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    input_cost: f64,
    output_cost: f64,
    cache_read_cost: f64,
    cache_write_cost: f64,
    reasoning_cost: f64,
    total_cost: f64,
}

impl From<ChSessionRow> for SessionRow {
    fn from(row: ChSessionRow) -> Self {
        Self {
            session_id: row.session_id.unwrap_or_default(),
            user_id: row.user_id,
            environment: row.environment,
            start_time: DateTime::from_timestamp_micros(row.start_time)
                .unwrap_or(DateTime::UNIX_EPOCH),
            end_time: Some(
                DateTime::from_timestamp_micros(row.end_time).unwrap_or(DateTime::UNIX_EPOCH),
            ),
            trace_count: row.trace_count as i64,
            span_count: row.span_count as i64,
            observation_count: row.observation_count as i64,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            total_tokens: row.total_tokens,
            cache_read_tokens: row.cache_read_tokens,
            cache_write_tokens: row.cache_write_tokens,
            reasoning_tokens: row.reasoning_tokens,
            input_cost: row.input_cost,
            output_cost: row.output_cost,
            cache_read_cost: row.cache_read_cost,
            cache_write_cost: row.cache_write_cost,
            reasoning_cost: row.reasoning_cost,
            total_cost: row.total_cost,
        }
    }
}

/// ClickHouse row for event queries
#[derive(Row, Deserialize)]
struct ChEventRow {
    span_id: String,
    event_index: i32,
    event_timestamp: String,
    event_name: Option<String>,
    attributes: Option<String>,
}

impl From<ChEventRow> for EventRow {
    fn from(row: ChEventRow) -> Self {
        Self {
            span_id: row.span_id,
            event_index: row.event_index,
            event_time: parse_iso_timestamp(&row.event_timestamp),
            event_name: row.event_name,
            attributes: row.attributes,
        }
    }
}

/// ClickHouse row for link queries
#[derive(Row, Deserialize)]
struct ChLinkRow {
    span_id: String,
    linked_trace_id: String,
    linked_span_id: String,
    attributes: Option<String>,
}

impl From<ChLinkRow> for LinkRow {
    fn from(row: ChLinkRow) -> Self {
        Self {
            span_id: row.span_id,
            linked_trace_id: row.linked_trace_id,
            linked_span_id: row.linked_span_id,
            attributes: row.attributes,
        }
    }
}

/// List traces with pagination and filtering
pub async fn list_traces(
    client: &Client,
    params: &ListTracesParams,
) -> Result<(Vec<TraceRow>, u64), ClickhouseError> {
    // Build WHERE conditions using parameterized queries
    let mut cb = ConditionBuilder::new();
    cb.add_eq("project_id", &params.project_id);

    if let Some(ref sid) = params.session_id {
        // session_id is only on root spans; use trace_id subquery to include all spans
        cb.conditions.push("trace_id IN (SELECT DISTINCT trace_id FROM otel_spans FINAL WHERE project_id = ? AND session_id = ?)".to_string());
        cb.params
            .push(QueryParam::String(params.project_id.clone()));
        cb.params.push(QueryParam::String(sid.clone()));
    }
    if let Some(ref uid) = params.user_id {
        cb.add_eq("user_id", uid);
    }
    if let Some(ref envs) = params.environment
        && !envs.is_empty()
    {
        cb.add_in("environment", envs);
    }
    if let Some(ref from) = params.from_timestamp {
        cb.add_timestamp_gte("timestamp_start", from);
    }
    if let Some(ref to) = params.to_timestamp {
        cb.add_timestamp_lte("timestamp_start", to);
    }

    let where_clause = cb.build();

    let (count_sql, needs_double_bind) = if !params.include_nongenai {
        (
            format!(
                r#"SELECT count(DISTINCT trace_id) as cnt FROM otel_spans FINAL
                   WHERE {} AND trace_id IN (
                       SELECT trace_id FROM otel_spans FINAL WHERE {} AND observation_type != 'span'
                   )"#,
                where_clause, where_clause
            ),
            true,
        )
    } else {
        (
            format!(
                "SELECT count(DISTINCT trace_id) as cnt FROM otel_spans FINAL WHERE {}",
                where_clause
            ),
            false,
        )
    };

    // Bind parameters (twice if subquery used)
    let bind_times = if needs_double_bind { 2 } else { 1 };
    let total: u64 = cb
        .bind_to_n(client.query(&count_sql), bind_times)
        .fetch_one()
        .await?;

    // Determine sort and pagination
    let (sort_field, sort_dir) = params
        .order_by
        .as_ref()
        .map(|o| {
            let dir = match o.direction {
                crate::api::types::OrderDirection::Desc => "DESC",
                crate::api::types::OrderDirection::Asc => "ASC",
            };
            (o.column.as_str(), dir)
        })
        .unwrap_or(("timestamp_start", "DESC"));

    let ch_sort_field = match sort_field {
        "start_time" => "min_ts",
        "end_time" => "max_ts",
        "duration_ms" => "duration_ms",
        "total_cost" => "total_cost",
        "observation_count" => "observation_count",
        _ => "min_ts",
    };

    let offset = (params.page.saturating_sub(1)) * params.limit;

    // GenAI filter for having clause
    let having_clause = if !params.include_nongenai {
        "HAVING observation_count > 0"
    } else {
        ""
    };

    // Scope dedup CTE to traces in the time range when available.
    // The CTE loads ALL spans for matching traces (no time filter on CTE itself)
    // so generation spans outside the window still affect token dedup.
    let dedup = build_time_scoped_dedup(
        &params.project_id,
        params.from_timestamp.as_ref(),
        params.to_timestamp.as_ref(),
    );

    // Data query with CTEs
    let data_sql = format!(
        r#"
        WITH {dedup_cte},
        gen_totals AS (
            SELECT
                g.trace_id,
                sum(gen_ai_usage_input_tokens) AS input_tokens,
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
            FROM otel_spans g FINAL
            WHERE {where_clause}
              AND {dedup_condition}
            GROUP BY g.trace_id
        ),
        filtered_traces AS (
            SELECT
                sp.project_id,
                sp.trace_id,
                min(sp.timestamp_start) as min_ts,
                max(coalesce(sp.timestamp_end, sp.timestamp_start)) as max_ts,
                dateDiff('millisecond', min(sp.timestamp_start), max(coalesce(sp.timestamp_end, sp.timestamp_start))) as duration_ms,
                coalesce(max(gt.total_cost), 0) as total_cost,
                countIf(sp.observation_type != 'span') as observation_count
            FROM otel_spans sp FINAL
            LEFT JOIN gen_totals gt ON sp.trace_id = gt.trace_id
            WHERE {where_clause}
            GROUP BY sp.project_id, sp.trace_id
            {having_clause}
            ORDER BY {ch_sort_field} {sort_dir}
            LIMIT {limit} OFFSET {offset}
        )
        SELECT
            t.trace_id as trace_id,
            argMinIf(s.span_name, s.timestamp_start, s.parent_span_id IS NULL AND s.span_name IS NOT NULL) as trace_name,
            toInt64(toUnixTimestamp64Micro(min(s.timestamp_start))) as start_time,
            toInt64(toUnixTimestamp64Micro(max(coalesce(s.timestamp_end, s.timestamp_start)))) as end_time,
            dateDiff('millisecond', min(s.timestamp_start), max(coalesce(s.timestamp_end, s.timestamp_start))) as duration_ms,
            argMinIf(s.session_id, s.timestamp_start, s.session_id IS NOT NULL) as session_id,
            argMinIf(s.user_id, s.timestamp_start, s.user_id IS NOT NULL) as user_id,
            argMinIf(s.environment, s.timestamp_start, s.environment IS NOT NULL) as environment,
            count() AS span_count,
            coalesce(max(gt2.input_tokens), 0) AS input_tokens,
            coalesce(max(gt2.output_tokens), 0) AS output_tokens,
            coalesce(max(gt2.total_tokens), 0) AS total_tokens,
            coalesce(max(gt2.cache_read_tokens), 0) AS cache_read_tokens,
            coalesce(max(gt2.cache_write_tokens), 0) AS cache_write_tokens,
            coalesce(max(gt2.reasoning_tokens), 0) AS reasoning_tokens,
            coalesce(max(gt2.input_cost), 0) AS input_cost,
            coalesce(max(gt2.output_cost), 0) AS output_cost,
            coalesce(max(gt2.cache_read_cost), 0) AS cache_read_cost,
            coalesce(max(gt2.cache_write_cost), 0) AS cache_write_cost,
            coalesce(max(gt2.reasoning_cost), 0) AS reasoning_cost,
            coalesce(max(gt2.total_cost), 0) AS total_cost,
            any(s.tags) AS tags,
            countIf(s.observation_type != 'span') AS observation_count,
            argMinIf(s.metadata, s.timestamp_start, s.parent_span_id IS NULL) AS metadata,
            COALESCE(
                argMinIf(s.input_preview, s.timestamp_start, s.parent_span_id IS NULL AND s.input_preview IS NOT NULL AND s.input_preview != ''),
                argMinIf(s.input_preview, s.timestamp_start, s.input_preview IS NOT NULL AND s.input_preview != '')
            ) AS input_preview,
            COALESCE(
                argMinIf(s.output_preview, s.timestamp_start, s.parent_span_id IS NULL AND s.output_preview IS NOT NULL AND s.output_preview != ''),
                argMaxIf(s.output_preview, s.timestamp_start, s.output_preview IS NOT NULL AND s.output_preview != '')
            ) AS output_preview,
            coalesce(max(s.status_code = 'ERROR'), 0) AS has_error
        FROM filtered_traces t
        JOIN otel_spans s FINAL ON t.project_id = s.project_id AND t.trace_id = s.trace_id
        LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id
        GROUP BY t.trace_id, t.min_ts
        ORDER BY t.min_ts {sort_dir}
        "#,
        dedup_cte = dedup.0,
        dedup_condition = TOKEN_DEDUP_CONDITION,
        where_clause = where_clause,
        having_clause = having_clause,
        ch_sort_field = ch_sort_field,
        sort_dir = sort_dir,
        limit = params.limit,
        offset = offset
    );

    // Bind: dedup_lookup(project_id + time-scope params) + where_clause x2
    let mut query = client.query(&data_sql).bind(params.project_id.as_str());
    for param in &dedup.1 {
        query = match param {
            QueryParam::String(s) => query.bind(s.as_str()),
            QueryParam::Int64(i) => query.bind(i),
        };
    }
    let rows: Vec<ChTraceRow> = cb.bind_to_n(query, 2).fetch_all().await?;

    Ok((rows.into_iter().map(TraceRow::from).collect(), total))
}

/// Get spans for a specific trace
pub async fn get_spans_for_trace(
    client: &Client,
    project_id: &str,
    trace_id: &str,
) -> Result<Vec<SpanRow>, ClickhouseError> {
    let sql = format!(
        r#"
        SELECT
            trace_id,
            span_id,
            parent_span_id,
            span_name,
            span_kind,
            span_category,
            observation_type,
            framework,
            status_code,
            toInt64(toUnixTimestamp64Micro(timestamp_start)) as start_time,
            if(timestamp_end IS NOT NULL, toInt64(toUnixTimestamp64Micro(timestamp_end)), NULL) as end_time,
            duration_ms,
            environment,
            JSONExtractRaw(raw_span, 'resource', 'attributes') as resource_attributes,
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
            toFloat64(gen_ai_cost_input) as gen_ai_cost_input,
            toFloat64(gen_ai_cost_output) as gen_ai_cost_output,
            toFloat64(gen_ai_cost_cache_read) as gen_ai_cost_cache_read,
            toFloat64(gen_ai_cost_cache_write) as gen_ai_cost_cache_write,
            toFloat64(gen_ai_cost_reasoning) as gen_ai_cost_reasoning,
            toFloat64(gen_ai_cost_total) as gen_ai_cost_total,
            gen_ai_usage_details,
            metadata,
            JSONExtractRaw(raw_span, 'attributes') as attributes,
            input_preview,
            output_preview,
            raw_span,
            toInt64(toUnixTimestamp64Micro(ingested_at)) as ingested_at_us
        FROM otel_spans FINAL
        WHERE project_id = ? AND trace_id = ?
        ORDER BY timestamp_start
        LIMIT {}
    "#,
        QUERY_MAX_SPANS_PER_TRACE
    );

    let rows: Vec<ChSpanRow> = client
        .query(&sql)
        .bind(project_id)
        .bind(trace_id)
        .fetch_all()
        .await?;

    let spans: Vec<SpanRow> = rows.into_iter().map(SpanRow::from).collect();
    Ok(deduplicate_spans(spans))
}

/// Get a single span
pub async fn get_span(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Option<SpanRow>, ClickhouseError> {
    let sql = r#"
        SELECT
            trace_id,
            span_id,
            parent_span_id,
            span_name,
            span_kind,
            span_category,
            observation_type,
            framework,
            status_code,
            toInt64(toUnixTimestamp64Micro(timestamp_start)) as start_time,
            if(timestamp_end IS NOT NULL, toInt64(toUnixTimestamp64Micro(timestamp_end)), NULL) as end_time,
            duration_ms,
            environment,
            JSONExtractRaw(raw_span, 'resource', 'attributes') as resource_attributes,
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
            toFloat64(gen_ai_cost_input) as gen_ai_cost_input,
            toFloat64(gen_ai_cost_output) as gen_ai_cost_output,
            toFloat64(gen_ai_cost_cache_read) as gen_ai_cost_cache_read,
            toFloat64(gen_ai_cost_cache_write) as gen_ai_cost_cache_write,
            toFloat64(gen_ai_cost_reasoning) as gen_ai_cost_reasoning,
            toFloat64(gen_ai_cost_total) as gen_ai_cost_total,
            gen_ai_usage_details,
            metadata,
            JSONExtractRaw(raw_span, 'attributes') as attributes,
            input_preview,
            output_preview,
            raw_span,
            toInt64(toUnixTimestamp64Micro(ingested_at)) as ingested_at_us
        FROM otel_spans FINAL
        WHERE project_id = ? AND trace_id = ? AND span_id = ?
        LIMIT 1
    "#;

    let row: Option<ChSpanRow> = client
        .query(sql)
        .bind(project_id)
        .bind(trace_id)
        .bind(span_id)
        .fetch_optional()
        .await?;

    Ok(row.map(SpanRow::from))
}

/// List spans with pagination and filtering
pub async fn list_spans(
    client: &Client,
    params: &ListSpansParams,
) -> Result<(Vec<SpanRow>, u64), ClickhouseError> {
    // Build WHERE conditions using parameterized queries
    let mut cb = ConditionBuilder::new();
    cb.add_eq("project_id", &params.project_id);

    if let Some(ref tid) = params.trace_id {
        cb.add_eq("trace_id", tid);
    }
    if let Some(ref sid) = params.session_id {
        // session_id is only on root spans; use trace_id subquery to include all spans
        cb.conditions.push("trace_id IN (SELECT DISTINCT trace_id FROM otel_spans FINAL WHERE project_id = ? AND session_id = ?)".to_string());
        cb.params
            .push(QueryParam::String(params.project_id.clone()));
        cb.params.push(QueryParam::String(sid.clone()));
    }
    if let Some(ref uid) = params.user_id {
        cb.add_eq("user_id", uid);
    }
    if let Some(ref envs) = params.environment
        && !envs.is_empty()
    {
        cb.add_in("environment", envs);
    }
    if let Some(ref cat) = params.span_category {
        cb.add_eq("span_category", cat);
    }
    if let Some(ref obs) = params.observation_type {
        cb.add_eq("observation_type", obs);
    }
    if let Some(ref fw) = params.framework {
        cb.add_eq("framework", fw);
    }
    if let Some(ref model) = params.gen_ai_request_model {
        cb.add_eq("gen_ai_request_model", model);
    }
    if let Some(ref status) = params.status_code {
        cb.add_eq("status_code", status);
    }
    if let Some(ref from) = params.from_timestamp {
        cb.add_timestamp_gte("timestamp_start", from);
    }
    if let Some(ref to) = params.to_timestamp {
        cb.add_timestamp_lte("timestamp_start", to);
    }
    if params.is_observation == Some(true) {
        cb.add_raw("observation_type != 'span'");
    }

    let where_clause = cb.build();

    // Count query
    let count_sql = format!(
        "SELECT count() as cnt FROM otel_spans FINAL WHERE {}",
        where_clause
    );
    let total: u64 = cb.bind_to(client.query(&count_sql)).fetch_one().await?;

    // Order - use safe whitelist mapping for defense in depth
    let order = params
        .order_by
        .as_ref()
        .map(|o| {
            // Whitelist mapping for span columns (matches API validation in SPAN_SORTABLE)
            let col = match o.column.as_str() {
                "start_time" | "timestamp_start" => "timestamp_start",
                "end_time" | "timestamp_end" => "timestamp_end",
                "duration_ms" => "duration_ms",
                "span_name" => "span_name",
                _ => "timestamp_start", // Safe default for unknown columns
            };
            let dir = match o.direction {
                crate::api::types::OrderDirection::Desc => "DESC",
                crate::api::types::OrderDirection::Asc => "ASC",
            };
            format!("{} {}", col, dir)
        })
        .unwrap_or_else(|| "timestamp_start DESC".to_string());

    let offset = (params.page.saturating_sub(1)) * params.limit;

    let data_sql = format!(
        r#"
        SELECT
            trace_id,
            span_id,
            parent_span_id,
            span_name,
            span_kind,
            span_category,
            observation_type,
            framework,
            status_code,
            toInt64(toUnixTimestamp64Micro(timestamp_start)) as start_time,
            if(timestamp_end IS NOT NULL, toInt64(toUnixTimestamp64Micro(timestamp_end)), NULL) as end_time,
            duration_ms,
            environment,
            JSONExtractRaw(raw_span, 'resource', 'attributes') as resource_attributes,
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
            toFloat64(gen_ai_cost_input) as gen_ai_cost_input,
            toFloat64(gen_ai_cost_output) as gen_ai_cost_output,
            toFloat64(gen_ai_cost_cache_read) as gen_ai_cost_cache_read,
            toFloat64(gen_ai_cost_cache_write) as gen_ai_cost_cache_write,
            toFloat64(gen_ai_cost_reasoning) as gen_ai_cost_reasoning,
            toFloat64(gen_ai_cost_total) as gen_ai_cost_total,
            gen_ai_usage_details,
            metadata,
            JSONExtractRaw(raw_span, 'attributes') as attributes,
            input_preview,
            output_preview,
            raw_span,
            toInt64(toUnixTimestamp64Micro(ingested_at)) as ingested_at_us
        FROM otel_spans FINAL
        WHERE {}
        ORDER BY {}
        LIMIT {} OFFSET {}
        "#,
        where_clause, order, params.limit, offset
    );

    let rows: Vec<ChSpanRow> = cb.bind_to(client.query(&data_sql)).fetch_all().await?;

    Ok((rows.into_iter().map(SpanRow::from).collect(), total))
}

/// Get feed spans (cursor-based pagination for real-time updates)
pub async fn get_feed_spans(
    client: &Client,
    params: &FeedSpansParams,
) -> Result<Vec<SpanRow>, ClickhouseError> {
    // Build WHERE conditions using parameterized queries
    let mut cb = ConditionBuilder::new();
    cb.add_eq("project_id", &params.project_id);

    // Cursor condition - use parameterized comparison for both timestamp and span_id
    if let Some((cursor_time_us, cursor_span_id)) = &params.cursor {
        // For tuple comparison, both values are parameterized
        cb.conditions
            .push("(toInt64(toUnixTimestamp64Micro(ingested_at)), span_id) < (?, ?)".to_string());
        cb.params.push(QueryParam::Int64(*cursor_time_us));
        cb.params.push(QueryParam::String(cursor_span_id.clone()));
    }

    // Time filters
    if let Some(ref start) = params.start_time {
        cb.add_timestamp_gte("timestamp_start", start);
    }
    if let Some(ref end) = params.end_time {
        cb.add_timestamp_lt("timestamp_start", end);
    }

    if params.is_observation == Some(true) {
        cb.add_raw("observation_type != 'span'");
    }

    let where_clause = cb.build();

    let sql = format!(
        r#"
        SELECT
            trace_id,
            span_id,
            parent_span_id,
            span_name,
            span_kind,
            span_category,
            observation_type,
            framework,
            status_code,
            toInt64(toUnixTimestamp64Micro(timestamp_start)) as start_time,
            if(timestamp_end IS NOT NULL, toInt64(toUnixTimestamp64Micro(timestamp_end)), NULL) as end_time,
            duration_ms,
            environment,
            JSONExtractRaw(raw_span, 'resource', 'attributes') as resource_attributes,
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
            toFloat64(gen_ai_cost_input) as gen_ai_cost_input,
            toFloat64(gen_ai_cost_output) as gen_ai_cost_output,
            toFloat64(gen_ai_cost_cache_read) as gen_ai_cost_cache_read,
            toFloat64(gen_ai_cost_cache_write) as gen_ai_cost_cache_write,
            toFloat64(gen_ai_cost_reasoning) as gen_ai_cost_reasoning,
            toFloat64(gen_ai_cost_total) as gen_ai_cost_total,
            gen_ai_usage_details,
            metadata,
            JSONExtractRaw(raw_span, 'attributes') as attributes,
            input_preview,
            output_preview,
            raw_span,
            toInt64(toUnixTimestamp64Micro(ingested_at)) as ingested_at_us
        FROM otel_spans FINAL
        WHERE {}
        ORDER BY ingested_at DESC, span_id DESC
        LIMIT {}
        "#,
        where_clause, params.limit
    );

    let rows: Vec<ChSpanRow> = cb.bind_to(client.query(&sql)).fetch_all().await?;

    Ok(rows.into_iter().map(SpanRow::from).collect())
}

/// List sessions with pagination and filtering
pub async fn list_sessions(
    client: &Client,
    params: &ListSessionsParams,
) -> Result<(Vec<SessionRow>, u64), ClickhouseError> {
    // Build WHERE conditions using parameterized queries
    let mut cb = ConditionBuilder::new();
    cb.add_eq("project_id", &params.project_id);
    cb.add_raw("session_id IS NOT NULL");

    if let Some(ref uid) = params.user_id {
        cb.add_eq("user_id", uid);
    }
    if let Some(ref envs) = params.environment
        && !envs.is_empty()
    {
        cb.add_in("environment", envs);
    }
    if let Some(ref from) = params.from_timestamp {
        cb.add_timestamp_gte("timestamp_start", from);
    }
    if let Some(ref to) = params.to_timestamp {
        cb.add_timestamp_lte("timestamp_start", to);
    }

    let where_clause = cb.build();

    let count_sql = format!(
        "SELECT count(DISTINCT session_id) as cnt FROM otel_spans FINAL WHERE {}",
        where_clause
    );
    let total: u64 = cb.bind_to(client.query(&count_sql)).fetch_one().await?;

    // Determine sort
    let (sort_field, sort_dir) = params
        .order_by
        .as_ref()
        .map(|o| {
            let dir = match o.direction {
                crate::api::types::OrderDirection::Desc => "DESC",
                crate::api::types::OrderDirection::Asc => "ASC",
            };
            (o.column.as_str(), dir)
        })
        .unwrap_or(("timestamp_start", "DESC"));

    let ch_sort_field = match sort_field {
        "start_time" => "min_ts",
        "total_cost" => "total_cost",
        "trace_count" => "trace_count",
        "span_count" => "span_count",
        _ => "min_ts",
    };

    let offset = (params.page.saturating_sub(1)) * params.limit;

    let dedup = build_time_scoped_dedup(
        &params.project_id,
        params.from_timestamp.as_ref(),
        params.to_timestamp.as_ref(),
    );

    let data_sql = format!(
        r#"
        WITH {dedup_cte},
        gen_totals AS (
            SELECT
                g.session_id,
                sum(gen_ai_usage_input_tokens) AS input_tokens,
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
            FROM otel_spans g FINAL
            WHERE {where_clause}
              AND {dedup_condition}
            GROUP BY g.session_id
        ),
        filtered_sessions AS (
            SELECT
                sp.project_id,
                sp.session_id,
                min(sp.timestamp_start) as min_ts,
                coalesce(max(gt.total_cost), 0) as total_cost,
                count(DISTINCT sp.trace_id) as trace_count,
                count() as span_count
            FROM otel_spans sp FINAL
            LEFT JOIN gen_totals gt ON sp.session_id = gt.session_id
            WHERE {where_clause}
            GROUP BY sp.project_id, sp.session_id
            ORDER BY {ch_sort_field} {sort_dir}
            LIMIT {limit} OFFSET {offset}
        )
        SELECT
            f.session_id as session_id,
            argMinIf(s.user_id, s.timestamp_start, s.user_id IS NOT NULL) as user_id,
            argMinIf(s.environment, s.timestamp_start, s.environment IS NOT NULL) as environment,
            toInt64(toUnixTimestamp64Micro(min(s.timestamp_start))) as start_time,
            toInt64(toUnixTimestamp64Micro(max(coalesce(s.timestamp_end, s.timestamp_start)))) as end_time,
            count(DISTINCT s.trace_id) AS trace_count,
            count() AS span_count,
            countIf(s.observation_type != 'span') AS observation_count,
            coalesce(max(gt2.input_tokens), 0) AS input_tokens,
            coalesce(max(gt2.output_tokens), 0) AS output_tokens,
            coalesce(max(gt2.total_tokens), 0) AS total_tokens,
            coalesce(max(gt2.cache_read_tokens), 0) AS cache_read_tokens,
            coalesce(max(gt2.cache_write_tokens), 0) AS cache_write_tokens,
            coalesce(max(gt2.reasoning_tokens), 0) AS reasoning_tokens,
            coalesce(max(gt2.input_cost), 0) AS input_cost,
            coalesce(max(gt2.output_cost), 0) AS output_cost,
            coalesce(max(gt2.cache_read_cost), 0) AS cache_read_cost,
            coalesce(max(gt2.cache_write_cost), 0) AS cache_write_cost,
            coalesce(max(gt2.reasoning_cost), 0) AS reasoning_cost,
            coalesce(max(gt2.total_cost), 0) AS total_cost
        FROM filtered_sessions f
        JOIN otel_spans s FINAL ON f.project_id = s.project_id AND f.session_id = s.session_id
        LEFT JOIN gen_totals gt2 ON f.session_id = gt2.session_id
        GROUP BY f.session_id, f.min_ts
        ORDER BY f.min_ts {sort_dir}
        "#,
        dedup_cte = dedup.0,
        dedup_condition = TOKEN_DEDUP_CONDITION,
        where_clause = where_clause,
        ch_sort_field = ch_sort_field,
        sort_dir = sort_dir,
        limit = params.limit,
        offset = offset
    );

    // Bind: dedup_lookup(project_id + time-scope params) + where_clause x2
    let mut query = client.query(&data_sql).bind(params.project_id.as_str());
    for param in &dedup.1 {
        query = match param {
            QueryParam::String(s) => query.bind(s.as_str()),
            QueryParam::Int64(i) => query.bind(i),
        };
    }
    let rows: Vec<ChSessionRow> = cb.bind_to_n(query, 2).fetch_all().await?;

    Ok((rows.into_iter().map(SessionRow::from).collect(), total))
}

/// Get session details
/// session_id is only on root spans; uses session_traces CTE to find all traces,
/// then queries all spans from those traces.
pub async fn get_session(
    client: &Client,
    project_id: &str,
    session_id: &str,
) -> Result<Option<SessionRow>, ClickhouseError> {
    let sql = format!(
        r#"
        WITH session_traces AS (
            SELECT DISTINCT trace_id FROM otel_spans FINAL
            WHERE project_id = ? AND session_id = ?
        ),
        {dedup_cte},
        gen_totals AS (
            SELECT
                sum(gen_ai_usage_input_tokens) AS input_tokens,
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
            FROM otel_spans g FINAL
            WHERE g.project_id = ?
              AND g.trace_id IN (SELECT trace_id FROM session_traces)
              AND {dedup_condition}
        )
        SELECT
            toNullable(?) as session_id,
            argMinIf(s.user_id, s.timestamp_start, s.user_id IS NOT NULL) as user_id,
            argMinIf(s.environment, s.timestamp_start, s.environment IS NOT NULL) as environment,
            toInt64(toUnixTimestamp64Micro(min(s.timestamp_start))) as start_time,
            toInt64(toUnixTimestamp64Micro(max(coalesce(s.timestamp_end, s.timestamp_start)))) as end_time,
            count(DISTINCT s.trace_id) AS trace_count,
            count() AS span_count,
            countIf(s.observation_type != 'span') AS observation_count,
            coalesce(gt.input_tokens, 0) AS input_tokens,
            coalesce(gt.output_tokens, 0) AS output_tokens,
            coalesce(gt.total_tokens, 0) AS total_tokens,
            coalesce(gt.cache_read_tokens, 0) AS cache_read_tokens,
            coalesce(gt.cache_write_tokens, 0) AS cache_write_tokens,
            coalesce(gt.reasoning_tokens, 0) AS reasoning_tokens,
            coalesce(gt.input_cost, 0) AS input_cost,
            coalesce(gt.output_cost, 0) AS output_cost,
            coalesce(gt.cache_read_cost, 0) AS cache_read_cost,
            coalesce(gt.cache_write_cost, 0) AS cache_write_cost,
            coalesce(gt.reasoning_cost, 0) AS reasoning_cost,
            coalesce(gt.total_cost, 0) AS total_cost
        FROM otel_spans s FINAL
        CROSS JOIN gen_totals gt
        WHERE s.project_id = ?
          AND s.trace_id IN (SELECT trace_id FROM session_traces)
        GROUP BY gt.input_tokens, gt.output_tokens, gt.total_tokens,
                 gt.cache_read_tokens, gt.cache_write_tokens, gt.reasoning_tokens,
                 gt.input_cost, gt.output_cost, gt.cache_read_cost, gt.cache_write_cost,
                 gt.reasoning_cost, gt.total_cost
    "#,
        dedup_cte = build_dedup_lookup_cte("trace_id IN (SELECT trace_id FROM session_traces)"),
        dedup_condition = TOKEN_DEDUP_CONDITION,
    );

    // Bind order: session_traces(project_id, session_id), dedup_lookup(project_id),
    //             gen_totals(project_id), SELECT(session_id), main(project_id)
    let row: Option<ChSessionRow> = client
        .query(&sql)
        .bind(project_id)
        .bind(session_id)
        .bind(project_id)
        .bind(project_id)
        .bind(session_id)
        .bind(project_id)
        .fetch_optional()
        .await?;

    Ok(row.map(SessionRow::from))
}

/// Get events for a span (extracted from raw_span JSON)
pub async fn get_events_for_span(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Vec<EventRow>, ClickhouseError> {
    // Use JSONExtractArrayRaw to get events array, then parse
    // LIMIT prevents memory exhaustion with pathological data
    let sql = format!(
        r#"
        SELECT
            span_id,
            toInt32(arrayJoin(range(JSONLength(raw_span, 'events')))) as event_index,
            JSONExtractString(JSONExtractRaw(raw_span, 'events', arrayJoin(range(JSONLength(raw_span, 'events'))) + 1), 'timestamp') as event_timestamp,
            JSONExtractString(JSONExtractRaw(raw_span, 'events', arrayJoin(range(JSONLength(raw_span, 'events'))) + 1), 'name') as event_name,
            JSONExtractRaw(JSONExtractRaw(raw_span, 'events', arrayJoin(range(JSONLength(raw_span, 'events'))) + 1), 'attributes') as attributes
        FROM otel_spans FINAL
        WHERE project_id = ? AND trace_id = ? AND span_id = ?
          AND JSONLength(raw_span, 'events') > 0
        ORDER BY event_index
        LIMIT {}
    "#,
        QUERY_MAX_SPANS_PER_TRACE
    );

    let rows: Vec<ChEventRow> = client
        .query(&sql)
        .bind(project_id)
        .bind(trace_id)
        .bind(span_id)
        .fetch_all()
        .await?;

    Ok(rows.into_iter().map(EventRow::from).collect())
}

/// Get links for a span (extracted from raw_span JSON)
pub async fn get_links_for_span(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Vec<LinkRow>, ClickhouseError> {
    // LIMIT prevents memory exhaustion with pathological data
    let sql = format!(
        r#"
        SELECT
            span_id,
            JSONExtractString(JSONExtractRaw(raw_span, 'links', arrayJoin(range(JSONLength(raw_span, 'links'))) + 1), 'trace_id') as linked_trace_id,
            JSONExtractString(JSONExtractRaw(raw_span, 'links', arrayJoin(range(JSONLength(raw_span, 'links'))) + 1), 'span_id') as linked_span_id,
            JSONExtractRaw(JSONExtractRaw(raw_span, 'links', arrayJoin(range(JSONLength(raw_span, 'links'))) + 1), 'attributes') as attributes
        FROM otel_spans FINAL
        WHERE project_id = ? AND trace_id = ? AND span_id = ?
          AND JSONLength(raw_span, 'links') > 0
        LIMIT {}
    "#,
        QUERY_MAX_SPANS_PER_TRACE
    );

    let rows: Vec<ChLinkRow> = client
        .query(&sql)
        .bind(project_id)
        .bind(trace_id)
        .bind(span_id)
        .fetch_all()
        .await?;

    Ok(rows.into_iter().map(LinkRow::from).collect())
}

/// Get a single trace by ID
pub async fn get_trace(
    client: &Client,
    project_id: &str,
    trace_id: &str,
) -> Result<Option<TraceRow>, ClickhouseError> {
    let sql = format!(
        r#"
        WITH {dedup_cte},
        gen_totals AS (
            SELECT
                sum(gen_ai_usage_input_tokens) AS input_tokens,
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
            FROM otel_spans g FINAL
            WHERE g.project_id = ? AND g.trace_id = ?
              AND {dedup_condition}
        )
        SELECT
            s.trace_id as trace_id,
            argMinIf(s.span_name, s.timestamp_start, s.parent_span_id IS NULL AND s.span_name IS NOT NULL) as trace_name,
            toInt64(toUnixTimestamp64Micro(min(s.timestamp_start))) as start_time,
            toInt64(toUnixTimestamp64Micro(max(coalesce(s.timestamp_end, s.timestamp_start)))) as end_time,
            dateDiff('millisecond', min(s.timestamp_start), max(coalesce(s.timestamp_end, s.timestamp_start))) as duration_ms,
            argMinIf(s.session_id, s.timestamp_start, s.session_id IS NOT NULL) as session_id,
            argMinIf(s.user_id, s.timestamp_start, s.user_id IS NOT NULL) as user_id,
            argMinIf(s.environment, s.timestamp_start, s.environment IS NOT NULL) as environment,
            count() AS span_count,
            coalesce(gt.input_tokens, 0) AS input_tokens,
            coalesce(gt.output_tokens, 0) AS output_tokens,
            coalesce(gt.total_tokens, 0) AS total_tokens,
            coalesce(gt.cache_read_tokens, 0) AS cache_read_tokens,
            coalesce(gt.cache_write_tokens, 0) AS cache_write_tokens,
            coalesce(gt.reasoning_tokens, 0) AS reasoning_tokens,
            coalesce(gt.input_cost, 0) AS input_cost,
            coalesce(gt.output_cost, 0) AS output_cost,
            coalesce(gt.cache_read_cost, 0) AS cache_read_cost,
            coalesce(gt.cache_write_cost, 0) AS cache_write_cost,
            coalesce(gt.reasoning_cost, 0) AS reasoning_cost,
            coalesce(gt.total_cost, 0) AS total_cost,
            any(s.tags) AS tags,
            countIf(s.observation_type != 'span') AS observation_count,
            argMinIf(s.metadata, s.timestamp_start, s.parent_span_id IS NULL) AS metadata,
            COALESCE(
                argMinIf(s.input_preview, s.timestamp_start, s.parent_span_id IS NULL AND s.input_preview IS NOT NULL AND s.input_preview != ''),
                argMinIf(s.input_preview, s.timestamp_start, s.input_preview IS NOT NULL AND s.input_preview != '')
            ) AS input_preview,
            COALESCE(
                argMinIf(s.output_preview, s.timestamp_start, s.parent_span_id IS NULL AND s.output_preview IS NOT NULL AND s.output_preview != ''),
                argMaxIf(s.output_preview, s.timestamp_start, s.output_preview IS NOT NULL AND s.output_preview != '')
            ) AS output_preview,
            coalesce(max(s.status_code = 'ERROR'), 0) AS has_error
        FROM otel_spans s FINAL
        CROSS JOIN gen_totals gt
        WHERE s.project_id = ? AND s.trace_id = ?
        GROUP BY s.trace_id, gt.input_tokens, gt.output_tokens, gt.total_tokens,
                 gt.cache_read_tokens, gt.cache_write_tokens, gt.reasoning_tokens,
                 gt.input_cost, gt.output_cost, gt.cache_read_cost, gt.cache_write_cost,
                 gt.reasoning_cost, gt.total_cost
    "#,
        dedup_cte = build_dedup_lookup_cte("trace_id = ?"),
        dedup_condition = TOKEN_DEDUP_CONDITION,
    );

    // Bind order: dedup_lookup(project_id, trace_id), gen_totals(project_id, trace_id), main(project_id, trace_id)
    let row: Option<ChTraceRow> = client
        .query(&sql)
        .bind(project_id)
        .bind(trace_id)
        .bind(project_id)
        .bind(trace_id)
        .bind(project_id)
        .bind(trace_id)
        .fetch_optional()
        .await?;

    Ok(row.map(TraceRow::from))
}

/// Get traces for a session
/// session_id is only on root spans; uses session_traces CTE to find all traces,
/// then queries all spans from those traces.
pub async fn get_traces_for_session(
    client: &Client,
    project_id: &str,
    session_id: &str,
) -> Result<Vec<TraceRow>, ClickhouseError> {
    let sql = format!(
        r#"
        WITH session_traces AS (
            SELECT DISTINCT trace_id FROM otel_spans FINAL
            WHERE project_id = ? AND session_id = ?
        ),
        {dedup_cte},
        gen_totals AS (
            SELECT
                g.trace_id,
                sum(gen_ai_usage_input_tokens) AS input_tokens,
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
            FROM otel_spans g FINAL
            WHERE g.project_id = ?
              AND g.trace_id IN (SELECT trace_id FROM session_traces)
              AND {dedup_condition}
            GROUP BY g.trace_id
        )
        SELECT
            s.trace_id as trace_id,
            argMinIf(s.span_name, s.timestamp_start, s.parent_span_id IS NULL AND s.span_name IS NOT NULL) as trace_name,
            toInt64(toUnixTimestamp64Micro(min(s.timestamp_start))) as start_time,
            toInt64(toUnixTimestamp64Micro(max(coalesce(s.timestamp_end, s.timestamp_start)))) as end_time,
            dateDiff('millisecond', min(s.timestamp_start), max(coalesce(s.timestamp_end, s.timestamp_start))) as duration_ms,
            argMinIf(s.session_id, s.timestamp_start, s.session_id IS NOT NULL) as session_id,
            argMinIf(s.user_id, s.timestamp_start, s.user_id IS NOT NULL) as user_id,
            argMinIf(s.environment, s.timestamp_start, s.environment IS NOT NULL) as environment,
            count() AS span_count,
            coalesce(max(gt.input_tokens), 0) AS input_tokens,
            coalesce(max(gt.output_tokens), 0) AS output_tokens,
            coalesce(max(gt.total_tokens), 0) AS total_tokens,
            coalesce(max(gt.cache_read_tokens), 0) AS cache_read_tokens,
            coalesce(max(gt.cache_write_tokens), 0) AS cache_write_tokens,
            coalesce(max(gt.reasoning_tokens), 0) AS reasoning_tokens,
            coalesce(max(gt.input_cost), 0) AS input_cost,
            coalesce(max(gt.output_cost), 0) AS output_cost,
            coalesce(max(gt.cache_read_cost), 0) AS cache_read_cost,
            coalesce(max(gt.cache_write_cost), 0) AS cache_write_cost,
            coalesce(max(gt.reasoning_cost), 0) AS reasoning_cost,
            coalesce(max(gt.total_cost), 0) AS total_cost,
            any(s.tags) AS tags,
            countIf(s.observation_type != 'span') AS observation_count,
            argMinIf(s.metadata, s.timestamp_start, s.parent_span_id IS NULL) AS metadata,
            COALESCE(
                argMinIf(s.input_preview, s.timestamp_start, s.parent_span_id IS NULL AND s.input_preview IS NOT NULL AND s.input_preview != ''),
                argMinIf(s.input_preview, s.timestamp_start, s.input_preview IS NOT NULL AND s.input_preview != '')
            ) AS input_preview,
            COALESCE(
                argMinIf(s.output_preview, s.timestamp_start, s.parent_span_id IS NULL AND s.output_preview IS NOT NULL AND s.output_preview != ''),
                argMaxIf(s.output_preview, s.timestamp_start, s.output_preview IS NOT NULL AND s.output_preview != '')
            ) AS output_preview,
            coalesce(max(s.status_code = 'ERROR'), 0) AS has_error
        FROM otel_spans s FINAL
        LEFT JOIN gen_totals gt ON s.trace_id = gt.trace_id
        WHERE s.project_id = ?
          AND s.trace_id IN (SELECT trace_id FROM session_traces)
        GROUP BY s.trace_id
        ORDER BY min(s.timestamp_start) DESC
    "#,
        dedup_cte = build_dedup_lookup_cte("trace_id IN (SELECT trace_id FROM session_traces)"),
        dedup_condition = TOKEN_DEDUP_CONDITION,
    );

    // Bind order: session_traces(project_id, session_id), dedup_lookup(project_id),
    //             gen_totals(project_id), main(project_id)
    let rows: Vec<ChTraceRow> = client
        .query(&sql)
        .bind(project_id)
        .bind(session_id)
        .bind(project_id)
        .bind(project_id)
        .bind(project_id)
        .fetch_all()
        .await?;

    Ok(rows.into_iter().map(TraceRow::from).collect())
}

/// Get trace IDs for given session IDs
pub async fn get_trace_ids_for_sessions(
    client: &Client,
    project_id: &str,
    session_ids: &[String],
) -> Result<Vec<String>, ClickhouseError> {
    if session_ids.is_empty() {
        return Ok(vec![]);
    }

    let placeholders: Vec<&str> = session_ids.iter().map(|_| "?").collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        "SELECT DISTINCT trace_id FROM otel_spans FINAL WHERE project_id = ? AND session_id IN ({})",
        in_clause
    );

    #[derive(Row, Deserialize)]
    struct TraceIdRow {
        trace_id: String,
    }

    let mut query = client.query(&sql).bind(project_id);
    for sid in session_ids {
        query = query.bind(sid);
    }

    let rows: Vec<TraceIdRow> = query.fetch_all().await?;

    Ok(rows.into_iter().map(|r| r.trace_id).collect())
}

/// Get span counts (events and links) in bulk
pub async fn get_span_counts_bulk(
    client: &Client,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<
    std::collections::HashMap<(String, String), crate::data::types::SpanCounts>,
    ClickhouseError,
> {
    use crate::data::types::SpanCounts;
    use std::collections::HashMap;

    if spans.is_empty() {
        return Ok(HashMap::new());
    }

    let mut counts: HashMap<(String, String), SpanCounts> = HashMap::with_capacity(spans.len());

    // Build IN clause for (trace_id, span_id) pairs with parameterized placeholders
    let pairs: Vec<&str> = spans.iter().map(|_| "(?, ?)").collect();
    let in_clause = pairs.join(", ");

    let sql = format!(
        r#"SELECT
            trace_id,
            span_id,
            coalesce(JSONLength(raw_span, 'events'), 0) as event_count,
            coalesce(JSONLength(raw_span, 'links'), 0) as link_count
         FROM otel_spans FINAL
         WHERE project_id = ? AND (trace_id, span_id) IN ({})"#,
        in_clause
    );

    #[derive(Row, Deserialize)]
    struct CountRow {
        trace_id: String,
        span_id: String,
        event_count: u64,
        link_count: u64,
    }

    let mut query = client.query(&sql).bind(project_id);
    for (tid, sid) in spans {
        query = query.bind(tid).bind(sid);
    }

    let rows: Vec<CountRow> = query.fetch_all().await?;

    for row in rows {
        counts.insert(
            (row.trace_id, row.span_id),
            SpanCounts {
                event_count: row.event_count as i64,
                link_count: row.link_count as i64,
            },
        );
    }

    Ok(counts)
}

/// Delete traces by IDs
///
/// In distributed mode, `table` should be the local table name (e.g., `otel_spans_local`)
/// and `on_cluster` should be the ON CLUSTER clause (e.g., ` ON CLUSTER cluster_name`).
pub async fn delete_traces(
    client: &Client,
    table: &str,
    on_cluster: &str,
    project_id: &str,
    trace_ids: &[String],
) -> Result<u64, ClickhouseError> {
    if trace_ids.is_empty() {
        return Ok(0);
    }

    let placeholders: Vec<&str> = trace_ids.iter().map(|_| "?").collect();
    let in_clause = placeholders.join(", ");

    // ClickHouse uses lightweight deletes (mutations)
    // In distributed mode, must use local table with ON CLUSTER
    let sql = format!(
        "ALTER TABLE {}{} DELETE WHERE project_id = ? AND trace_id IN ({})",
        table, on_cluster, in_clause
    );

    let mut query = client.query(&sql).bind(project_id);
    for tid in trace_ids {
        query = query.bind(tid);
    }
    query.execute().await?;

    // Return count - mutations are async in ClickHouse so we estimate
    Ok(trace_ids.len() as u64)
}

/// Delete spans by (trace_id, span_id) pairs
///
/// In distributed mode, `table` should be the local table name and
/// `on_cluster` should be the ON CLUSTER clause.
pub async fn delete_spans(
    client: &Client,
    table: &str,
    on_cluster: &str,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<u64, ClickhouseError> {
    if spans.is_empty() {
        return Ok(0);
    }

    // Build IN clause for (trace_id, span_id) pairs with parameterized placeholders
    let pairs: Vec<&str> = spans.iter().map(|_| "(?, ?)").collect();
    let in_clause = pairs.join(", ");

    let sql = format!(
        "ALTER TABLE {}{} DELETE WHERE project_id = ? AND (trace_id, span_id) IN ({})",
        table, on_cluster, in_clause
    );

    let mut query = client.query(&sql).bind(project_id);
    for (tid, sid) in spans {
        query = query.bind(tid).bind(sid);
    }
    query.execute().await?;

    Ok(spans.len() as u64)
}

/// Delete sessions (all spans in the sessions)
///
/// In distributed mode, `table` should be the local table name and
/// `on_cluster` should be the ON CLUSTER clause.
pub async fn delete_sessions(
    client: &Client,
    table: &str,
    on_cluster: &str,
    project_id: &str,
    session_ids: &[String],
) -> Result<u64, ClickhouseError> {
    if session_ids.is_empty() {
        return Ok(0);
    }

    let placeholders: Vec<&str> = session_ids.iter().map(|_| "?").collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        "ALTER TABLE {}{} DELETE WHERE project_id = ? AND session_id IN ({})",
        table, on_cluster, in_clause
    );

    let mut query = client.query(&sql).bind(project_id);
    for sid in session_ids {
        query = query.bind(sid);
    }
    query.execute().await?;

    Ok(session_ids.len() as u64)
}

/// Delete all data for a project
///
/// In distributed mode, `spans_table` and `metrics_table` should be local table names
/// and `on_cluster` should be the ON CLUSTER clause.
pub async fn delete_project_data(
    client: &Client,
    spans_table: &str,
    metrics_table: &str,
    on_cluster: &str,
    project_id: &str,
) -> Result<u64, ClickhouseError> {
    // Count rows first (approximate) - use distributed table for count
    let count_sql = "SELECT count() FROM otel_spans FINAL WHERE project_id = ?";
    let count: u64 = client.query(count_sql).bind(project_id).fetch_one().await?;

    // Delete all spans for the project (parameterized query for safety)
    // In distributed mode, must use local table with ON CLUSTER
    let sql = format!(
        "ALTER TABLE {}{} DELETE WHERE project_id = ?",
        spans_table, on_cluster
    );
    client.query(&sql).bind(project_id).execute().await?;

    // Delete metrics too (best-effort - table may not exist in all deployments)
    let metrics_sql = format!(
        "ALTER TABLE {}{} DELETE WHERE project_id = ?",
        metrics_table, on_cluster
    );
    if let Err(e) = client.query(&metrics_sql).bind(project_id).execute().await {
        tracing::debug!("Metrics deletion skipped (table may not exist): {}", e);
    }

    Ok(count)
}

/// Count spans grouped by project for a set of project IDs.
pub async fn count_spans_by_project(
    client: &Client,
    project_ids: &[String],
) -> Result<std::collections::HashMap<String, u64>, ClickhouseError> {
    use std::collections::HashMap;

    if project_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders: Vec<&str> = project_ids.iter().map(|_| "?").collect();
    let sql = format!(
        "SELECT project_id, count() AS cnt FROM otel_spans FINAL WHERE project_id IN ({}) GROUP BY project_id",
        placeholders.join(", ")
    );

    #[derive(Row, Deserialize)]
    struct CountRow {
        project_id: String,
        cnt: u64,
    }

    let mut query = client.query(&sql);
    for pid in project_ids {
        query = query.bind(pid);
    }

    let rows: Vec<CountRow> = query.fetch_all().await?;

    let mut result = HashMap::new();
    for row in rows {
        result.insert(row.project_id, row.cnt);
    }

    Ok(result)
}

/// ClickHouse row for filter options
#[derive(Row, Deserialize)]
struct ChFilterOptionRow {
    value: Option<String>,
    count: u64,
}

/// Allowed columns for trace filter options (maps view column to span column)
const TRACE_FILTER_OPTION_COLUMNS: &[(&str, &str)] = &[
    ("trace_name", "span_name"),
    ("session_id", "session_id"),
    ("user_id", "user_id"),
    ("environment", "environment"),
];

/// Allowed columns for span filter options
const SPAN_FILTER_OPTION_COLUMNS: &[&str] = &[
    "span_name",
    "span_category",
    "observation_type",
    "framework",
    "gen_ai_system",
    "gen_ai_request_model",
    "status_code",
    "session_id",
    "user_id",
    "environment",
];

/// Allowed columns for session filter options
const SESSION_FILTER_OPTION_COLUMNS: &[&str] = &["user_id", "environment"];

/// Get trace filter options
pub async fn get_trace_filter_options(
    client: &Client,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<
    std::collections::HashMap<String, Vec<crate::data::traits::FilterOptionRow>>,
    ClickhouseError,
> {
    use crate::data::traits::FilterOptionRow;
    use std::collections::HashMap;

    let mut results: HashMap<String, Vec<FilterOptionRow>> = HashMap::new();

    // Build time filter conditions with parameterized timestamps
    let mut time_conditions = String::new();
    let mut time_params: Vec<i64> = Vec::new();
    if let Some(from) = from_timestamp {
        time_conditions.push_str(" AND timestamp_start >= fromUnixTimestamp64Micro(?)");
        time_params.push(from.timestamp_micros());
    }
    if let Some(to) = to_timestamp {
        time_conditions.push_str(" AND timestamp_start <= fromUnixTimestamp64Micro(?)");
        time_params.push(to.timestamp_micros());
    }

    for column in columns {
        // Find the spans table column name
        let span_column = TRACE_FILTER_OPTION_COLUMNS
            .iter()
            .find(|(view_col, _)| *view_col == column.as_str())
            .map(|(_, span_col)| *span_col);

        let span_column = match span_column {
            Some(col) => col,
            None => continue,
        };

        // For trace_name, only look at root spans
        let extra_condition = if column == "trace_name" {
            " AND parent_span_id IS NULL"
        } else {
            ""
        };

        let sql = format!(
            r#"
            SELECT {col} as value, count(DISTINCT trace_id) as count
            FROM otel_spans FINAL
            WHERE project_id = ?{time_cond}{extra_cond} AND {col} IS NOT NULL
            GROUP BY {col}
            ORDER BY count DESC
            LIMIT {limit}
            "#,
            col = span_column,
            time_cond = time_conditions,
            extra_cond = extra_condition,
            limit = QUERY_MAX_FILTER_SUGGESTIONS
        );

        let mut query = client.query(&sql).bind(project_id);
        for ts in &time_params {
            query = query.bind(ts);
        }
        let rows: Vec<ChFilterOptionRow> = query.fetch_all().await?;

        let options: Vec<FilterOptionRow> = rows
            .into_iter()
            .filter_map(|r| {
                r.value.map(|v| FilterOptionRow {
                    value: v,
                    count: r.count,
                })
            })
            .collect();

        results.insert(column.clone(), options);
    }

    Ok(results)
}

/// Get trace tags options
pub async fn get_trace_tags_options(
    client: &Client,
    project_id: &str,
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<Vec<crate::data::traits::FilterOptionRow>, ClickhouseError> {
    use crate::data::traits::FilterOptionRow;

    // Build time filter conditions with parameterized timestamps
    let mut time_conditions = String::new();
    let mut time_params: Vec<i64> = Vec::new();
    if let Some(from) = from_timestamp {
        time_conditions.push_str(" AND timestamp_start >= fromUnixTimestamp64Micro(?)");
        time_params.push(from.timestamp_micros());
    }
    if let Some(to) = to_timestamp {
        time_conditions.push_str(" AND timestamp_start <= fromUnixTimestamp64Micro(?)");
        time_params.push(to.timestamp_micros());
    }

    // ClickHouse: extract tags from JSON array and count distinct traces
    let sql = format!(
        r#"
        SELECT
            arrayJoin(JSONExtractArrayRaw(assumeNotNull(tags))) as value,
            count(DISTINCT trace_id) as count
        FROM otel_spans FINAL
        WHERE project_id = ?{time_cond} AND tags IS NOT NULL AND tags != '[]'
        GROUP BY value
        ORDER BY count DESC
        LIMIT {limit}
        "#,
        time_cond = time_conditions,
        limit = QUERY_MAX_FILTER_SUGGESTIONS
    );

    let mut query = client.query(&sql).bind(project_id);
    for ts in &time_params {
        query = query.bind(ts);
    }
    let rows: Vec<ChFilterOptionRow> = query.fetch_all().await?;

    // Clean up JSON string values (remove quotes)
    let options: Vec<FilterOptionRow> = rows
        .into_iter()
        .filter_map(|r| {
            r.value.map(|v| {
                let cleaned = v.trim_matches('"').to_string();
                FilterOptionRow {
                    value: cleaned,
                    count: r.count,
                }
            })
        })
        .collect();

    Ok(options)
}

/// Get span filter options
pub async fn get_span_filter_options(
    client: &Client,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
    observations_only: bool,
) -> Result<
    std::collections::HashMap<String, Vec<crate::data::traits::FilterOptionRow>>,
    ClickhouseError,
> {
    use crate::data::traits::FilterOptionRow;
    use std::collections::HashMap;

    let mut results: HashMap<String, Vec<FilterOptionRow>> = HashMap::new();

    // Build base conditions with parameterized timestamps
    let mut conditions = String::new();
    let mut time_params: Vec<i64> = Vec::new();
    if let Some(from) = from_timestamp {
        conditions.push_str(" AND timestamp_start >= fromUnixTimestamp64Micro(?)");
        time_params.push(from.timestamp_micros());
    }
    if let Some(to) = to_timestamp {
        conditions.push_str(" AND timestamp_start <= fromUnixTimestamp64Micro(?)");
        time_params.push(to.timestamp_micros());
    }
    if observations_only {
        conditions.push_str(" AND observation_type != 'span'");
    }

    for column in columns {
        // Validate column is allowed
        if !SPAN_FILTER_OPTION_COLUMNS.contains(&column.as_str()) {
            continue;
        }

        let sql = format!(
            r#"
            SELECT {col} as value, count() as count
            FROM otel_spans FINAL
            WHERE project_id = ?{cond} AND {col} IS NOT NULL
            GROUP BY {col}
            ORDER BY count DESC
            LIMIT {limit}
            "#,
            col = column,
            cond = conditions,
            limit = QUERY_MAX_FILTER_SUGGESTIONS
        );

        let mut query = client.query(&sql).bind(project_id);
        for ts in &time_params {
            query = query.bind(ts);
        }
        let rows: Vec<ChFilterOptionRow> = query.fetch_all().await?;

        let options: Vec<FilterOptionRow> = rows
            .into_iter()
            .filter_map(|r| {
                r.value.map(|v| FilterOptionRow {
                    value: v,
                    count: r.count,
                })
            })
            .collect();

        results.insert(column.clone(), options);
    }

    Ok(results)
}

/// Get session filter options
pub async fn get_session_filter_options(
    client: &Client,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<
    std::collections::HashMap<String, Vec<crate::data::traits::FilterOptionRow>>,
    ClickhouseError,
> {
    use crate::data::traits::FilterOptionRow;
    use std::collections::HashMap;

    let mut results: HashMap<String, Vec<FilterOptionRow>> = HashMap::new();

    // Build time conditions with parameterized timestamps
    let mut conditions = String::new();
    let mut time_params: Vec<i64> = Vec::new();
    if let Some(from) = from_timestamp {
        conditions.push_str(" AND timestamp_start >= fromUnixTimestamp64Micro(?)");
        time_params.push(from.timestamp_micros());
    }
    if let Some(to) = to_timestamp {
        conditions.push_str(" AND timestamp_start <= fromUnixTimestamp64Micro(?)");
        time_params.push(to.timestamp_micros());
    }

    for column in columns {
        // Validate column is allowed
        if !SESSION_FILTER_OPTION_COLUMNS.contains(&column.as_str()) {
            continue;
        }

        let sql = format!(
            r#"
            SELECT {col} as value, count(DISTINCT session_id) as count
            FROM otel_spans FINAL
            WHERE project_id = ?{cond} AND session_id IS NOT NULL AND {col} IS NOT NULL
            GROUP BY {col}
            ORDER BY count DESC
            LIMIT {limit}
            "#,
            col = column,
            cond = conditions,
            limit = QUERY_MAX_FILTER_SUGGESTIONS
        );

        let mut query = client.query(&sql).bind(project_id);
        for ts in &time_params {
            query = query.bind(ts);
        }
        let rows: Vec<ChFilterOptionRow> = query.fetch_all().await?;

        let options: Vec<FilterOptionRow> = rows
            .into_iter()
            .filter_map(|r| {
                r.value.map(|v| FilterOptionRow {
                    value: v,
                    count: r.count,
                })
            })
            .collect();

        results.insert(column.clone(), options);
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_traces_params_default() {
        let params = ListTracesParams::default();
        assert_eq!(params.page, 0);
        assert_eq!(params.limit, 0);
    }
}
