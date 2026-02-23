//! Query repository for OTEL API queries

use chrono::{DateTime, Utc};
use duckdb::{Connection, Row};

use crate::api::routes::otel::filters::{SqlParams, columns};
use crate::core::constants::{QUERY_MAX_FILTER_SUGGESTIONS, QUERY_MAX_SPANS_PER_TRACE};
use crate::data::duckdb::{DuckdbError, in_transaction};
use crate::data::types::{
    EventRow, FeedSpansParams, LinkRow, ListSessionsParams, ListSpansParams, ListTracesParams,
    SessionRow, SpanRow, TraceRow, parse_tags,
};
use crate::utils::time::{micros_to_datetime, parse_iso_timestamp};

/// Build span conditions with optional table alias.
/// Returns (WHERE clause, bind values).
fn build_trace_span_conditions(params: &ListTracesParams, alias: &str) -> (String, Vec<String>) {
    let mut conditions = Vec::new();
    let mut bind_values = Vec::new();
    let mut sql_params = SqlParams::default();

    // Helper to format column with alias
    let col = |name: &str| -> String {
        if alias.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", alias, name)
        }
    };

    // Project ID (required)
    conditions.push(format!("{} = ?", col("project_id")));
    bind_values.push(params.project_id.clone());

    // Optional filters
    if let Some(ref sid) = params.session_id {
        // session_id is only on root spans; use trace_id subquery to include all spans
        conditions.push(format!(
            "{} IN (SELECT DISTINCT trace_id FROM otel_spans WHERE project_id = ? AND session_id = ?)",
            col("trace_id")
        ));
        bind_values.push(params.project_id.clone());
        bind_values.push(sid.clone());
    }

    if let Some(ref uid) = params.user_id {
        conditions.push(format!("{} = ?", col("user_id")));
        bind_values.push(uid.clone());
    }

    if let Some(ref envs) = params.environment
        && !envs.is_empty()
    {
        let placeholders: Vec<&str> = envs.iter().map(|_| "?").collect();
        conditions.push(format!(
            "{} IN ({})",
            col("environment"),
            placeholders.join(", ")
        ));
        bind_values.extend(envs.iter().cloned());
    }

    if let Some(ref from) = params.from_timestamp {
        conditions.push(format!("{} >= ?", col("timestamp_start")));
        bind_values.push(from.to_rfc3339());
    }

    if let Some(ref to) = params.to_timestamp {
        conditions.push(format!("{} <= ?", col("timestamp_start")));
        bind_values.push(to.to_rfc3339());
    }

    // Advanced filters with aliasing
    for filter in &params.filters {
        conditions.push(filter.to_sql_aliased(
            &mut sql_params,
            columns::map_trace_column_to_spans,
            alias,
        ));
    }
    bind_values.extend(sql_params.values);

    (conditions.join(" AND "), bind_values)
}

/// List traces with pagination and filters
///
/// Optimized query strategy:
/// 1. Count distinct trace_ids from otel_spans (uses indexes)
/// 2. Use CTE to filter and paginate trace_ids first
/// 3. Then aggregate only those traces (avoids full table scan)
pub fn list_traces(
    conn: &Connection,
    params: &ListTracesParams,
) -> Result<(Vec<TraceRow>, u64), DuckdbError> {
    // Build WHERE clause without alias for count query (single table)
    let (span_where, bind_values) = build_trace_span_conditions(params, "");

    // Count query: when filtering to GenAI only, use HAVING to filter traces with GenAI spans
    let count_sql = if !params.include_nongenai {
        format!(
            r#"SELECT COUNT(*) FROM (
                SELECT trace_id FROM otel_spans_v WHERE {}
                GROUP BY trace_id
                HAVING COUNT(*) FILTER (WHERE observation_type != 'span') > 0
            ) t"#,
            span_where
        )
    } else {
        format!(
            "SELECT COUNT(DISTINCT trace_id) FROM otel_spans_v WHERE {}",
            span_where
        )
    };
    let total = execute_count(conn, &count_sql, &bind_values)?;

    // Determine sort field and direction
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

    // Map view column names to spans table column names for sorting
    let span_sort_field = match sort_field {
        "start_time" => "min_ts",
        "end_time" => "max_ts",
        "duration_ms" => "duration_ms",
        "total_cost" => "total_cost",
        "observation_count" => "observation_count",
        _ => "min_ts",
    };

    let offset = (params.page.saturating_sub(1)) * params.limit;

    // Build aliased WHERE clauses for CTEs
    // gen_totals CTE uses alias "g", filtered_traces CTE uses alias "sp"
    let (span_where_g, bind_values_g) = build_trace_span_conditions(params, "g");
    let (span_where_sp, bind_values_sp) = build_trace_span_conditions(params, "sp");

    // HAVING clause to filter traces with GenAI spans when not including non-GenAI traces
    let having_clause = if !params.include_nongenai {
        "HAVING observation_count > 0"
    } else {
        ""
    };

    let data_sql = format!(
        r#"
        WITH gen_totals AS (
            SELECT
                g.trace_id,
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
            FROM otel_spans_v g
            WHERE {span_where_g}
              AND (
                  (g.observation_type = 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND (c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens) > 0
                   ))
                  OR
                  (g.observation_type != 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND (gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens) > 0
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.project_id = g.project_id
                         AND (p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens) > 0
                   ))
              )
            GROUP BY g.trace_id
        ),
        filtered_traces AS (
            SELECT
                sp.project_id,
                sp.trace_id,
                MIN(sp.timestamp_start) as min_ts,
                MAX(COALESCE(sp.timestamp_end, sp.timestamp_start)) as max_ts,
                DATE_DIFF('millisecond', MIN(sp.timestamp_start), MAX(COALESCE(sp.timestamp_end, sp.timestamp_start))) as duration_ms,
                COALESCE(MAX(gt.total_cost), 0)::DOUBLE as total_cost,
                COUNT(*) FILTER (WHERE sp.observation_type != 'span') as observation_count
            FROM otel_spans_v sp
            LEFT JOIN gen_totals gt ON sp.trace_id = gt.trace_id
            WHERE {span_where_sp}
            GROUP BY sp.project_id, sp.trace_id
            {having_clause}
            ORDER BY {span_sort_field} {sort_dir}
            LIMIT {limit} OFFSET {offset}
        )
        SELECT
            t.trace_id,
            COALESCE(
                FIRST(s.span_name) FILTER (WHERE s.parent_span_id IS NULL AND s.span_name IS NOT NULL),
                FIRST(s.span_name ORDER BY s.timestamp_start) FILTER (WHERE s.span_name IS NOT NULL)
            ) AS trace_name,
            MIN(s.timestamp_start) AS start_time,
            MAX(COALESCE(s.timestamp_end, s.timestamp_start)) AS end_time,
            DATE_DIFF('millisecond', MIN(s.timestamp_start), MAX(COALESCE(s.timestamp_end, s.timestamp_start))) AS duration_ms,
            FIRST(s.session_id ORDER BY s.timestamp_start) FILTER (WHERE s.session_id IS NOT NULL) AS session_id,
            FIRST(s.user_id ORDER BY s.timestamp_start) FILTER (WHERE s.user_id IS NOT NULL) AS user_id,
            FIRST(s.environment ORDER BY s.timestamp_start) FILTER (WHERE s.environment IS NOT NULL) AS environment,
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
            TO_JSON(FIRST(s.metadata) FILTER (WHERE s.parent_span_id IS NULL)) AS metadata,
            COALESCE(
                FIRST(s.input_preview) FILTER (WHERE s.parent_span_id IS NULL AND s.input_preview IS NOT NULL),
                FIRST(s.input_preview ORDER BY s.timestamp_start) FILTER (WHERE s.input_preview IS NOT NULL)
            ) AS input_preview,
            COALESCE(
                FIRST(s.output_preview) FILTER (WHERE s.parent_span_id IS NULL AND s.output_preview IS NOT NULL),
                FIRST(s.output_preview ORDER BY s.timestamp_start DESC) FILTER (WHERE s.output_preview IS NOT NULL)
            ) AS output_preview,
            bool_or(s.status_code = 'ERROR') AS has_error
        FROM filtered_traces t
        JOIN otel_spans_v s ON t.project_id = s.project_id AND t.trace_id = s.trace_id
        LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id
        GROUP BY t.trace_id, t.min_ts
        ORDER BY t.min_ts {sort_dir}
        "#,
        span_where_g = span_where_g,
        span_where_sp = span_where_sp,
        span_sort_field = span_sort_field,
        sort_dir = sort_dir,
        limit = params.limit,
        offset = offset
    );

    // Combine bind values: gen_totals CTE first, then filtered_traces CTE
    let mut all_bind_values = bind_values_g;
    all_bind_values.extend(bind_values_sp);
    let rows = execute_trace_query(conn, &data_sql, &all_bind_values)?;

    Ok((rows, total))
}

/// Get a single trace by ID
pub fn get_trace(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
) -> Result<Option<TraceRow>, DuckdbError> {
    // Two-path token filter: (1) generation root spans (parent not a generation),
    // (2) non-generation token spans in traces without any generation spans.
    // Handles: Strands (agent->cycle->gen->botocore), LangGraph, structured output.
    let sql = r#"
        WITH gen_totals AS (
            SELECT
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
            FROM otel_spans_v g
            WHERE g.project_id = ? AND g.trace_id = ?
              AND (
                  (g.observation_type = 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND (c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens) > 0
                   ))
                  OR
                  (g.observation_type != 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND (gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens) > 0
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.project_id = g.project_id
                         AND (p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens) > 0
                   ))
              )
        )
        SELECT
            s.trace_id,
            COALESCE(
                FIRST(s.span_name) FILTER (WHERE s.parent_span_id IS NULL AND s.span_name IS NOT NULL),
                FIRST(s.span_name ORDER BY s.timestamp_start) FILTER (WHERE s.span_name IS NOT NULL)
            ) AS trace_name,
            EPOCH_US(MIN(s.timestamp_start)) AS start_time,
            EPOCH_US(MAX(COALESCE(s.timestamp_end, s.timestamp_start))) AS end_time,
            DATE_DIFF('millisecond', MIN(s.timestamp_start), MAX(COALESCE(s.timestamp_end, s.timestamp_start))) AS duration_ms,
            FIRST(s.session_id ORDER BY s.timestamp_start) FILTER (WHERE s.session_id IS NOT NULL) AS session_id,
            FIRST(s.user_id ORDER BY s.timestamp_start) FILTER (WHERE s.user_id IS NOT NULL) AS user_id,
            FIRST(s.environment ORDER BY s.timestamp_start) FILTER (WHERE s.environment IS NOT NULL) AS environment,
            COUNT(*) AS span_count,
            gt.input_tokens,
            gt.output_tokens,
            gt.total_tokens,
            gt.cache_read_tokens,
            gt.cache_write_tokens,
            gt.reasoning_tokens,
            gt.input_cost::DOUBLE,
            gt.output_cost::DOUBLE,
            gt.cache_read_cost::DOUBLE,
            gt.cache_write_cost::DOUBLE,
            gt.reasoning_cost::DOUBLE,
            gt.total_cost::DOUBLE,
            TO_JSON(LIST_DISTINCT(FLATTEN(LIST(s.tags::JSON::VARCHAR[])))) AS tags,
            COUNT(*) FILTER (WHERE s.observation_type != 'span') AS observation_count,
            TO_JSON(FIRST(s.metadata) FILTER (WHERE s.parent_span_id IS NULL)) AS metadata,
            COALESCE(
                FIRST(s.input_preview) FILTER (WHERE s.parent_span_id IS NULL AND s.input_preview IS NOT NULL),
                FIRST(s.input_preview ORDER BY s.timestamp_start) FILTER (WHERE s.input_preview IS NOT NULL)
            ) AS input_preview,
            COALESCE(
                FIRST(s.output_preview) FILTER (WHERE s.parent_span_id IS NULL AND s.output_preview IS NOT NULL),
                FIRST(s.output_preview ORDER BY s.timestamp_start DESC) FILTER (WHERE s.output_preview IS NOT NULL)
            ) AS output_preview,
            bool_or(s.status_code = 'ERROR') AS has_error
        FROM otel_spans_v s
        CROSS JOIN gen_totals gt
        WHERE s.project_id = ? AND s.trace_id = ?
        GROUP BY s.trace_id, gt.input_tokens, gt.output_tokens, gt.total_tokens,
                 gt.cache_read_tokens, gt.cache_write_tokens, gt.reasoning_tokens,
                 gt.input_cost, gt.output_cost, gt.cache_read_cost, gt.cache_write_cost,
                 gt.reasoning_cost, gt.total_cost
    "#;

    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([
        project_id, trace_id, // generation_roots
        project_id, trace_id, // main query
    ])?;

    if let Some(row) = rows.next()? {
        Ok(Some(row_to_trace(row)?))
    } else {
        Ok(None)
    }
}

/// List spans with pagination and filters
pub fn list_spans(
    conn: &Connection,
    params: &ListSpansParams,
) -> Result<(Vec<SpanRow>, u64), DuckdbError> {
    let mut conditions = vec!["project_id = ?".to_string()];
    let mut bind_values: Vec<String> = vec![params.project_id.clone()];
    let mut sql_params = SqlParams::default();

    if let Some(ref tid) = params.trace_id {
        conditions.push("trace_id = ?".to_string());
        bind_values.push(tid.clone());
    }

    if let Some(ref sid) = params.session_id {
        // session_id is only on root spans; use trace_id subquery to include all spans
        conditions.push(
            "trace_id IN (SELECT DISTINCT trace_id FROM otel_spans WHERE project_id = ? AND session_id = ?)".to_string()
        );
        bind_values.push(params.project_id.clone());
        bind_values.push(sid.clone());
    }

    if let Some(ref uid) = params.user_id {
        conditions.push("user_id = ?".to_string());
        bind_values.push(uid.clone());
    }

    if let Some(ref envs) = params.environment
        && !envs.is_empty()
    {
        let placeholders: Vec<&str> = envs.iter().map(|_| "?").collect();
        conditions.push(format!("environment IN ({})", placeholders.join(", ")));
        bind_values.extend(envs.iter().cloned());
    }

    if let Some(ref cat) = params.span_category {
        conditions.push("span_category = ?".to_string());
        bind_values.push(cat.clone());
    }

    if let Some(ref obs) = params.observation_type {
        conditions.push("observation_type = ?".to_string());
        bind_values.push(obs.clone());
    }

    if let Some(ref fw) = params.framework {
        conditions.push("framework = ?".to_string());
        bind_values.push(fw.clone());
    }

    if let Some(ref model) = params.gen_ai_request_model {
        conditions.push("gen_ai_request_model = ?".to_string());
        bind_values.push(model.clone());
    }

    if let Some(ref status) = params.status_code {
        conditions.push("status_code = ?".to_string());
        bind_values.push(status.clone());
    }

    if let Some(ref from) = params.from_timestamp {
        conditions.push("timestamp_start >= ?".to_string());
        bind_values.push(from.to_rfc3339());
    }

    if let Some(ref to) = params.to_timestamp {
        conditions.push("timestamp_start <= ?".to_string());
        bind_values.push(to.to_rfc3339());
    }

    // Filter to observations only (GenAI spans)
    if params.is_observation == Some(true) {
        conditions.push("observation_type != 'span'".to_string());
    }

    // Apply advanced filters - map API column names to DB columns
    for filter in &params.filters {
        conditions.push(filter.to_sql_aliased(&mut sql_params, columns::map_span_column, ""));
    }
    bind_values.extend(sql_params.values);

    let where_clause = conditions.join(" AND ");

    // Count query (deduplicated via view)
    let count_sql = format!("SELECT COUNT(*) FROM otel_spans_v WHERE {}", where_clause);
    let total = execute_count(conn, &count_sql, &bind_values)?;

    // Data query - map API column names (start_time/end_time) to DB columns (timestamp_start/timestamp_end)
    let order = params
        .order_by
        .as_ref()
        .map(|o| o.to_sql_mapped(columns::map_span_column))
        .unwrap_or_else(|| "timestamp_start DESC".to_string());

    let offset = (params.page.saturating_sub(1)) * params.limit;
    let data_sql = format!(
        "SELECT trace_id, span_id, parent_span_id, span_name, span_kind, span_category,
                observation_type, framework, status_code, timestamp_start, timestamp_end,
                duration_ms, environment, (raw_span->'resource'->'attributes')::VARCHAR, session_id, user_id,
                gen_ai_system, gen_ai_request_model, gen_ai_agent_name, gen_ai_finish_reasons,
                gen_ai_usage_input_tokens, gen_ai_usage_output_tokens,
                gen_ai_usage_total_tokens, gen_ai_usage_cache_read_tokens, gen_ai_usage_cache_write_tokens,
                gen_ai_usage_reasoning_tokens,
                gen_ai_cost_input::DOUBLE, gen_ai_cost_output::DOUBLE, gen_ai_cost_cache_read::DOUBLE, gen_ai_cost_cache_write::DOUBLE,
                gen_ai_cost_reasoning::DOUBLE, gen_ai_cost_total::DOUBLE,
                gen_ai_usage_details::VARCHAR, metadata::VARCHAR, (raw_span->'attributes')::VARCHAR,
                input_preview, output_preview, raw_span::VARCHAR, ingested_at
         FROM otel_spans_v WHERE {} ORDER BY {} LIMIT {} OFFSET {}",
        where_clause, order, params.limit, offset
    );

    let rows = execute_span_query(conn, &data_sql, &bind_values)?;

    Ok((rows, total))
}

/// Get spans for feed with cursor-based pagination.
///
/// Uses (ingested_at, span_id) cursor for stable pagination.
/// Ordered by ingested_at DESC for "latest first" feed semantics.
pub fn get_feed_spans(
    conn: &Connection,
    params: &FeedSpansParams,
) -> Result<Vec<SpanRow>, DuckdbError> {
    let mut conditions = vec!["project_id = ?".to_string()];
    let mut bind_values: Vec<String> = vec![params.project_id.clone()];

    // Cursor condition: (ingested_at, span_id) < (cursor_time, cursor_span_id)
    if let Some((cursor_time_us, cursor_span_id)) = &params.cursor {
        conditions.push("(EPOCH_US(ingested_at), span_id) < (?::BIGINT, ?)".to_string());
        bind_values.push(cursor_time_us.to_string());
        bind_values.push(cursor_span_id.clone());
    }

    // Event time filters
    if let Some(start) = &params.start_time {
        conditions.push("timestamp_start >= ?".to_string());
        bind_values.push(start.format("%Y-%m-%d %H:%M:%S%.6f").to_string());
    }
    if let Some(end) = &params.end_time {
        conditions.push("timestamp_start < ?".to_string());
        bind_values.push(end.format("%Y-%m-%d %H:%M:%S%.6f").to_string());
    }

    // Filter to observations only (GenAI spans)
    if params.is_observation == Some(true) {
        conditions.push("observation_type != 'span'".to_string());
    }

    let where_clause = conditions.join(" AND ");

    let data_sql = format!(
        "SELECT trace_id, span_id, parent_span_id, span_name, span_kind, span_category,
                observation_type, framework, status_code, timestamp_start, timestamp_end,
                duration_ms, environment, (raw_span->'resource'->'attributes')::VARCHAR, session_id, user_id,
                gen_ai_system, gen_ai_request_model, gen_ai_agent_name, gen_ai_finish_reasons,
                gen_ai_usage_input_tokens, gen_ai_usage_output_tokens,
                gen_ai_usage_total_tokens, gen_ai_usage_cache_read_tokens, gen_ai_usage_cache_write_tokens,
                gen_ai_usage_reasoning_tokens,
                gen_ai_cost_input::DOUBLE, gen_ai_cost_output::DOUBLE, gen_ai_cost_cache_read::DOUBLE, gen_ai_cost_cache_write::DOUBLE,
                gen_ai_cost_reasoning::DOUBLE, gen_ai_cost_total::DOUBLE,
                gen_ai_usage_details::VARCHAR, metadata::VARCHAR, (raw_span->'attributes')::VARCHAR,
                input_preview, output_preview, raw_span::VARCHAR, ingested_at
         FROM otel_spans_v WHERE {} ORDER BY ingested_at DESC, span_id DESC LIMIT {}",
        where_clause, params.limit
    );

    execute_span_query(conn, &data_sql, &bind_values)
}

/// Get spans for a trace (for trace detail view)
pub fn get_spans_for_trace(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
) -> Result<Vec<SpanRow>, DuckdbError> {
    let sql = format!(
        "SELECT trace_id, span_id, parent_span_id, span_name, span_kind, span_category,
               observation_type, framework, status_code, timestamp_start, timestamp_end,
               duration_ms, environment, (raw_span->'resource'->'attributes')::VARCHAR, session_id, user_id,
               gen_ai_system, gen_ai_request_model, gen_ai_agent_name, gen_ai_finish_reasons,
               gen_ai_usage_input_tokens, gen_ai_usage_output_tokens,
               gen_ai_usage_total_tokens, gen_ai_usage_cache_read_tokens, gen_ai_usage_cache_write_tokens,
               gen_ai_usage_reasoning_tokens,
               gen_ai_cost_input::DOUBLE, gen_ai_cost_output::DOUBLE, gen_ai_cost_cache_read::DOUBLE, gen_ai_cost_cache_write::DOUBLE,
               gen_ai_cost_reasoning::DOUBLE, gen_ai_cost_total::DOUBLE,
               gen_ai_usage_details::VARCHAR, metadata::VARCHAR, (raw_span->'attributes')::VARCHAR,
               input_preview, output_preview, raw_span::VARCHAR, ingested_at
               FROM otel_spans_v WHERE project_id = ? AND trace_id = ? ORDER BY timestamp_start LIMIT {}",
        QUERY_MAX_SPANS_PER_TRACE
    );

    execute_span_query(conn, &sql, &[project_id.to_string(), trace_id.to_string()])
}

/// Get a single span by trace_id and span_id
pub fn get_span(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Option<SpanRow>, DuckdbError> {
    let sql = "SELECT trace_id, span_id, parent_span_id, span_name, span_kind, span_category,
               observation_type, framework, status_code, timestamp_start, timestamp_end,
               duration_ms, environment, (raw_span->'resource'->'attributes')::VARCHAR, session_id, user_id,
               gen_ai_system, gen_ai_request_model, gen_ai_agent_name, gen_ai_finish_reasons,
               gen_ai_usage_input_tokens, gen_ai_usage_output_tokens,
               gen_ai_usage_total_tokens, gen_ai_usage_cache_read_tokens, gen_ai_usage_cache_write_tokens,
               gen_ai_usage_reasoning_tokens,
               gen_ai_cost_input::DOUBLE, gen_ai_cost_output::DOUBLE, gen_ai_cost_cache_read::DOUBLE, gen_ai_cost_cache_write::DOUBLE,
               gen_ai_cost_reasoning::DOUBLE, gen_ai_cost_total::DOUBLE,
               gen_ai_usage_details::VARCHAR, metadata::VARCHAR, (raw_span->'attributes')::VARCHAR,
               input_preview, output_preview, raw_span::VARCHAR, ingested_at
               FROM otel_spans_v WHERE project_id = ? AND trace_id = ? AND span_id = ?";

    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([project_id, trace_id, span_id])?;

    if let Some(row) = rows.next()? {
        Ok(Some(row_to_span(row)?))
    } else {
        Ok(None)
    }
}

/// Get events for a span (from raw_span JSON)
pub fn get_events_for_span(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Vec<EventRow>, DuckdbError> {
    // LIMIT prevents memory exhaustion with pathological data
    let sql = format!(
        "SELECT span_id, (ordinality - 1)::INTEGER as event_index,
                      event->>'timestamp' as event_timestamp,
                      event->>'name' as event_name,
                      (event->'attributes')::VARCHAR as attributes
               FROM otel_spans_v, UNNEST(CAST(raw_span->'events' AS JSON[])) WITH ORDINALITY AS t(event, ordinality)
               WHERE project_id = ? AND trace_id = ? AND span_id = ?
               ORDER BY ordinality
               LIMIT {}",
        QUERY_MAX_SPANS_PER_TRACE
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut query_rows = stmt.query([project_id, trace_id, span_id])?;
    let mut events = vec![];

    while let Some(row) = query_rows.next()? {
        let event_timestamp: String = row.get(2)?;
        events.push(EventRow {
            span_id: row.get(0)?,
            event_index: row.get::<_, i32>(1)?,
            event_time: parse_iso_timestamp(&event_timestamp),
            event_name: row.get(3)?,
            attributes: row.get(4)?,
        });
    }

    Ok(events)
}

/// Get links for a span (from raw_span JSON)
pub fn get_links_for_span(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Vec<LinkRow>, DuckdbError> {
    // LIMIT prevents memory exhaustion with pathological data
    let sql = format!(
        "SELECT span_id, link->>'trace_id' as linked_trace_id, link->>'span_id' as linked_span_id,
                (link->'attributes')::VARCHAR as attributes
               FROM otel_spans_v, UNNEST(CAST(raw_span->'links' AS JSON[])) WITH ORDINALITY AS t(link, ordinality)
               WHERE project_id = ? AND trace_id = ? AND span_id = ?
               ORDER BY ordinality
               LIMIT {}",
        QUERY_MAX_SPANS_PER_TRACE
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut query_rows = stmt.query([project_id, trace_id, span_id])?;
    let mut links = vec![];

    while let Some(row) = query_rows.next()? {
        links.push(LinkRow {
            span_id: row.get(0)?,
            linked_trace_id: row.get(1)?,
            linked_span_id: row.get(2)?,
            attributes: row.get(3)?,
        });
    }

    Ok(links)
}

/// Build session span conditions with optional table alias.
/// Returns (WHERE clause, bind values).
fn build_session_span_conditions(
    params: &ListSessionsParams,
    alias: &str,
) -> (String, Vec<String>) {
    let mut conditions = Vec::new();
    let mut bind_values = Vec::new();
    let mut sql_params = SqlParams::default();

    // Helper to format column with alias
    let col = |name: &str| -> String {
        if alias.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", alias, name)
        }
    };

    // Required filters
    conditions.push(format!("{} = ?", col("project_id")));
    bind_values.push(params.project_id.clone());
    conditions.push(format!("{} IS NOT NULL", col("session_id")));

    // Optional filters
    if let Some(ref uid) = params.user_id {
        conditions.push(format!("{} = ?", col("user_id")));
        bind_values.push(uid.clone());
    }

    if let Some(ref envs) = params.environment
        && !envs.is_empty()
    {
        let placeholders: Vec<&str> = envs.iter().map(|_| "?").collect();
        conditions.push(format!(
            "{} IN ({})",
            col("environment"),
            placeholders.join(", ")
        ));
        bind_values.extend(envs.iter().cloned());
    }

    if let Some(ref from) = params.from_timestamp {
        conditions.push(format!("{} >= ?", col("timestamp_start")));
        bind_values.push(from.to_rfc3339());
    }

    if let Some(ref to) = params.to_timestamp {
        conditions.push(format!("{} <= ?", col("timestamp_start")));
        bind_values.push(to.to_rfc3339());
    }

    // Advanced filters with aliasing
    for filter in &params.filters {
        conditions.push(filter.to_sql_aliased(
            &mut sql_params,
            columns::map_session_column_to_spans,
            alias,
        ));
    }
    bind_values.extend(sql_params.values);

    (conditions.join(" AND "), bind_values)
}

/// List sessions with pagination and filters
///
/// Optimized query strategy (same as list_traces):
/// 1. First get distinct session_ids from otel_spans (fast, uses indexes)
/// 2. Then aggregate only those sessions (avoids full table scan)
pub fn list_sessions(
    conn: &Connection,
    params: &ListSessionsParams,
) -> Result<(Vec<SessionRow>, u64), DuckdbError> {
    // Build WHERE clause without alias for count query (single table)
    let (span_where, bind_values) = build_session_span_conditions(params, "");

    let count_sql = format!(
        "SELECT COUNT(DISTINCT session_id) FROM otel_spans_v WHERE {}",
        span_where
    );
    let total = execute_count(conn, &count_sql, &bind_values)?;

    // Determine sort field and direction
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

    // Map view column names to spans table column names for sorting
    let span_sort_field = match sort_field {
        "start_time" => "min_ts",
        "total_cost" => "total_cost",
        "trace_count" => "trace_count",
        "span_count" => "span_count",
        _ => "min_ts",
    };

    let offset = (params.page.saturating_sub(1)) * params.limit;

    // Build aliased WHERE clauses for CTEs
    // gen_totals CTE uses alias "g", filtered_sessions CTE uses alias "sp"
    let (span_where_g, bind_values_g) = build_session_span_conditions(params, "g");
    let (span_where_sp, bind_values_sp) = build_session_span_conditions(params, "sp");

    let data_sql = format!(
        r#"
        WITH gen_totals AS (
            SELECT
                g.session_id,
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
            FROM otel_spans_v g
            WHERE {span_where_g}
              AND (
                  (g.observation_type = 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND (c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens) > 0
                   ))
                  OR
                  (g.observation_type != 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND (gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens) > 0
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.project_id = g.project_id
                         AND (p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens) > 0
                   ))
              )
            GROUP BY g.session_id
        ),
        filtered_sessions AS (
            SELECT
                sp.project_id,
                sp.session_id,
                MIN(sp.timestamp_start) as min_ts,
                COALESCE(MAX(gt.total_cost), 0)::DOUBLE as total_cost,
                COUNT(DISTINCT sp.trace_id) as trace_count,
                COUNT(*) as span_count
            FROM otel_spans_v sp
            LEFT JOIN gen_totals gt ON sp.session_id = gt.session_id
            WHERE {span_where_sp}
            GROUP BY sp.project_id, sp.session_id
            ORDER BY {span_sort_field} {sort_dir}
            LIMIT {limit} OFFSET {offset}
        )
        SELECT
            f.session_id,
            FIRST(s.user_id ORDER BY s.timestamp_start) FILTER (WHERE s.user_id IS NOT NULL) AS user_id,
            FIRST(s.environment ORDER BY s.timestamp_start) FILTER (WHERE s.environment IS NOT NULL) AS environment,
            MIN(s.timestamp_start) AS start_time,
            MAX(COALESCE(s.timestamp_end, s.timestamp_start)) AS end_time,
            COUNT(DISTINCT s.trace_id) AS trace_count,
            COUNT(*) AS span_count,
            COUNT(*) FILTER (WHERE s.observation_type != 'span') AS observation_count,
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
            COALESCE(MAX(gt2.total_cost), 0)::DOUBLE AS total_cost
        FROM filtered_sessions f
        JOIN otel_spans_v s ON f.project_id = s.project_id AND f.session_id = s.session_id
        LEFT JOIN gen_totals gt2 ON f.session_id = gt2.session_id
        GROUP BY f.session_id, f.min_ts
        ORDER BY f.min_ts {sort_dir}
        "#,
        span_where_g = span_where_g,
        span_where_sp = span_where_sp,
        span_sort_field = span_sort_field,
        sort_dir = sort_dir,
        limit = params.limit,
        offset = offset
    );

    // Combine bind values: gen_totals CTE first, then filtered_sessions CTE
    let mut all_bind_values = bind_values_g;
    all_bind_values.extend(bind_values_sp);
    let rows = execute_session_query(conn, &data_sql, &all_bind_values)?;

    Ok((rows, total))
}

/// Get a single session by ID
pub fn get_session(
    conn: &Connection,
    project_id: &str,
    session_id: &str,
) -> Result<Option<SessionRow>, DuckdbError> {
    // session_id is only on root spans; use session_traces CTE to find all traces,
    // then query all spans from those traces.
    // Two-path token filter: see get_trace for detailed explanation.
    let sql = r#"
        WITH session_traces AS (
            SELECT DISTINCT trace_id FROM otel_spans_v
            WHERE project_id = ? AND session_id = ?
        ),
        gen_totals AS (
            SELECT
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
            FROM otel_spans_v g
            WHERE g.project_id = ?
              AND g.trace_id IN (SELECT trace_id FROM session_traces)
              AND (
                  (g.observation_type = 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND (c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens) > 0
                   ))
                  OR
                  (g.observation_type != 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND (gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens) > 0
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.project_id = g.project_id
                         AND (p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens) > 0
                   ))
              )
        )
        SELECT
            ? AS session_id,
            FIRST(s.user_id ORDER BY s.timestamp_start) FILTER (WHERE s.user_id IS NOT NULL) AS user_id,
            FIRST(s.environment ORDER BY s.timestamp_start) FILTER (WHERE s.environment IS NOT NULL) AS environment,
            EPOCH_US(MIN(s.timestamp_start)) AS start_time,
            EPOCH_US(MAX(COALESCE(s.timestamp_end, s.timestamp_start))) AS end_time,
            COUNT(DISTINCT s.trace_id) AS trace_count,
            COUNT(*) AS span_count,
            COUNT(*) FILTER (WHERE s.observation_type != 'span') AS observation_count,
            gt.input_tokens,
            gt.output_tokens,
            gt.total_tokens,
            gt.cache_read_tokens,
            gt.cache_write_tokens,
            gt.reasoning_tokens,
            gt.input_cost::DOUBLE,
            gt.output_cost::DOUBLE,
            gt.cache_read_cost::DOUBLE,
            gt.cache_write_cost::DOUBLE,
            gt.reasoning_cost::DOUBLE,
            gt.total_cost::DOUBLE
        FROM otel_spans_v s
        CROSS JOIN gen_totals gt
        WHERE s.project_id = ?
          AND s.trace_id IN (SELECT trace_id FROM session_traces)
        GROUP BY gt.input_tokens, gt.output_tokens, gt.total_tokens,
                 gt.cache_read_tokens, gt.cache_write_tokens, gt.reasoning_tokens,
                 gt.input_cost, gt.output_cost, gt.cache_read_cost, gt.cache_write_cost,
                 gt.reasoning_cost, gt.total_cost
    "#;

    let mut stmt = conn.prepare(sql)?;
    // Bind order: session_traces(project_id, session_id), gen_totals(project_id),
    //             SELECT(session_id), main(project_id)
    let mut rows = stmt.query([
        project_id, session_id, // session_traces CTE
        project_id, // gen_totals CTE
        session_id, // SELECT session_id literal
        project_id, // main query
    ])?;

    if let Some(row) = rows.next()? {
        Ok(Some(row_to_session(row)?))
    } else {
        Ok(None)
    }
}

/// Get traces for a session (summary only)
///
/// session_id is only on root spans; uses session_traces CTE to find all traces,
/// then queries all spans from those traces.
pub fn get_traces_for_session(
    conn: &Connection,
    project_id: &str,
    session_id: &str,
) -> Result<Vec<TraceRow>, DuckdbError> {
    // Two-path token filter: see get_trace for detailed explanation.
    let sql = r#"
        WITH session_traces AS (
            SELECT DISTINCT trace_id FROM otel_spans_v
            WHERE project_id = ? AND session_id = ?
        ),
        gen_totals AS (
            SELECT
                g.trace_id,
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
            FROM otel_spans_v g
            WHERE g.project_id = ?
              AND g.trace_id IN (SELECT trace_id FROM session_traces)
              AND (
                  (g.observation_type = 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND (c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens) > 0
                   ))
                  OR
                  (g.observation_type != 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND (gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens) > 0
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.project_id = g.project_id
                         AND (p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens) > 0
                   ))
              )
            GROUP BY g.trace_id
        )
        SELECT
            s.trace_id,
            COALESCE(
                FIRST(s.span_name) FILTER (WHERE s.parent_span_id IS NULL AND s.span_name IS NOT NULL),
                FIRST(s.span_name ORDER BY s.timestamp_start) FILTER (WHERE s.span_name IS NOT NULL)
            ) AS trace_name,
            EPOCH_US(MIN(s.timestamp_start)) AS start_time,
            EPOCH_US(MAX(COALESCE(s.timestamp_end, s.timestamp_start))) AS end_time,
            DATE_DIFF('millisecond', MIN(s.timestamp_start), MAX(COALESCE(s.timestamp_end, s.timestamp_start))) AS duration_ms,
            FIRST(s.session_id ORDER BY s.timestamp_start) FILTER (WHERE s.session_id IS NOT NULL) AS session_id,
            FIRST(s.user_id ORDER BY s.timestamp_start) FILTER (WHERE s.user_id IS NOT NULL) AS user_id,
            FIRST(s.environment ORDER BY s.timestamp_start) FILTER (WHERE s.environment IS NOT NULL) AS environment,
            COUNT(*) AS span_count,
            COALESCE(gt.input_tokens, 0) AS input_tokens,
            COALESCE(gt.output_tokens, 0) AS output_tokens,
            COALESCE(gt.total_tokens, 0) AS total_tokens,
            COALESCE(gt.cache_read_tokens, 0) AS cache_read_tokens,
            COALESCE(gt.cache_write_tokens, 0) AS cache_write_tokens,
            COALESCE(gt.reasoning_tokens, 0) AS reasoning_tokens,
            COALESCE(gt.input_cost, 0)::DOUBLE AS input_cost,
            COALESCE(gt.output_cost, 0)::DOUBLE AS output_cost,
            COALESCE(gt.cache_read_cost, 0)::DOUBLE AS cache_read_cost,
            COALESCE(gt.cache_write_cost, 0)::DOUBLE AS cache_write_cost,
            COALESCE(gt.reasoning_cost, 0)::DOUBLE AS reasoning_cost,
            COALESCE(gt.total_cost, 0)::DOUBLE AS total_cost,
            TO_JSON(LIST_DISTINCT(FLATTEN(LIST(s.tags::JSON::VARCHAR[])))) AS tags,
            COUNT(*) FILTER (WHERE s.observation_type != 'span') AS observation_count,
            TO_JSON(FIRST(s.metadata) FILTER (WHERE s.parent_span_id IS NULL)) AS metadata,
            COALESCE(
                FIRST(s.input_preview) FILTER (WHERE s.parent_span_id IS NULL AND s.input_preview IS NOT NULL),
                FIRST(s.input_preview ORDER BY s.timestamp_start) FILTER (WHERE s.input_preview IS NOT NULL)
            ) AS input_preview,
            COALESCE(
                FIRST(s.output_preview) FILTER (WHERE s.parent_span_id IS NULL AND s.output_preview IS NOT NULL),
                FIRST(s.output_preview ORDER BY s.timestamp_start DESC) FILTER (WHERE s.output_preview IS NOT NULL)
            ) AS output_preview,
            bool_or(s.status_code = 'ERROR') AS has_error
        FROM otel_spans_v s
        LEFT JOIN gen_totals gt ON s.trace_id = gt.trace_id
        WHERE s.project_id = ?
          AND s.trace_id IN (SELECT trace_id FROM session_traces)
        GROUP BY s.trace_id, gt.input_tokens, gt.output_tokens, gt.total_tokens,
                 gt.cache_read_tokens, gt.cache_write_tokens, gt.reasoning_tokens,
                 gt.input_cost, gt.output_cost, gt.cache_read_cost, gt.cache_write_cost,
                 gt.reasoning_cost, gt.total_cost
        ORDER BY MIN(s.timestamp_start) DESC
    "#;

    // Bind order: session_traces(project_id, session_id), gen_totals(project_id), main(project_id)
    execute_trace_query(
        conn,
        sql,
        &[
            project_id.to_string(),
            session_id.to_string(), // session_traces CTE
            project_id.to_string(), // gen_totals CTE
            project_id.to_string(), // main query
        ],
    )
}

/// Span counts result
#[derive(Debug, Default)]
pub struct SpanCounts {
    pub event_count: i64,
    pub link_count: i64,
}

/// Bulk fetch event and link counts for multiple spans (from raw_span JSON)
/// Returns a HashMap keyed by (trace_id, span_id)
pub fn get_span_counts_bulk(
    conn: &Connection,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<std::collections::HashMap<(String, String), SpanCounts>, DuckdbError> {
    use std::collections::HashMap;

    if spans.is_empty() {
        return Ok(HashMap::new());
    }

    let mut counts: HashMap<(String, String), SpanCounts> = HashMap::with_capacity(spans.len());

    // Build IN clause using &str to avoid unnecessary String allocations
    let placeholders: Vec<&str> = spans.iter().map(|_| "(?, ?)").collect();
    let in_clause = placeholders.join(", ");

    // Query both counts in a single pass (extract from raw_span JSON)
    let sql = format!(
        "SELECT trace_id, span_id,
                COALESCE(json_array_length(raw_span->'events'), 0),
                COALESCE(json_array_length(raw_span->'links'), 0)
         FROM otel_spans_v
         WHERE project_id = ? AND (trace_id, span_id) IN ({})",
        in_clause
    );

    let mut params: Vec<String> = Vec::with_capacity(1 + spans.len() * 2);
    params.push(project_id.to_string());
    for (trace_id, span_id) in spans {
        params.push(trace_id.clone());
        params.push(span_id.clone());
    }

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn duckdb::ToSql> =
        params.iter().map(|v| v as &dyn duckdb::ToSql).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;

    while let Some(row) = rows.next()? {
        let trace_id: String = row.get(0)?;
        let span_id: String = row.get(1)?;
        let event_count: i64 = row.get(2)?;
        let link_count: i64 = row.get(3)?;
        counts.insert(
            (trace_id, span_id),
            SpanCounts {
                event_count,
                link_count,
            },
        );
    }

    // Add defaults for spans not found in DB
    for (trace_id, span_id) in spans {
        counts
            .entry((trace_id.clone(), span_id.clone()))
            .or_default();
    }

    Ok(counts)
}

// --- Helper functions ---

fn execute_count(conn: &Connection, sql: &str, bind_values: &[String]) -> Result<u64, DuckdbError> {
    let mut stmt = conn.prepare(sql)?;
    let params: Vec<&dyn duckdb::ToSql> = bind_values
        .iter()
        .map(|v| v as &dyn duckdb::ToSql)
        .collect();
    let count: i64 = stmt.query_row(params.as_slice(), |row| row.get(0))?;
    Ok(count as u64)
}

fn execute_trace_query(
    conn: &Connection,
    sql: &str,
    bind_values: &[String],
) -> Result<Vec<TraceRow>, DuckdbError> {
    let mut stmt = conn.prepare(sql)?;
    let params: Vec<&dyn duckdb::ToSql> = bind_values
        .iter()
        .map(|v| v as &dyn duckdb::ToSql)
        .collect();
    let mut query_rows = stmt.query(params.as_slice())?;
    let mut rows = vec![];

    while let Some(row) = query_rows.next()? {
        rows.push(row_to_trace(row)?);
    }

    Ok(rows)
}

fn row_to_trace(row: &Row<'_>) -> Result<TraceRow, DuckdbError> {
    let start_time_micros: i64 = row.get(2)?;
    let end_time_micros: Option<i64> = row.get(3)?;
    let tags_json: Option<String> = row.get(21)?;
    let metadata_json: Option<String> = row.get(23)?;

    Ok(TraceRow {
        trace_id: row.get(0)?,
        trace_name: row.get(1)?,
        start_time: micros_to_datetime(start_time_micros),
        end_time: end_time_micros.map(micros_to_datetime),
        duration_ms: row.get(4)?,
        session_id: row.get(5)?,
        user_id: row.get(6)?,
        environment: row.get(7)?,
        span_count: row.get(8)?,
        input_tokens: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
        output_tokens: row.get::<_, Option<i64>>(10)?.unwrap_or(0),
        total_tokens: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
        cache_read_tokens: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
        cache_write_tokens: row.get::<_, Option<i64>>(13)?.unwrap_or(0),
        reasoning_tokens: row.get::<_, Option<i64>>(14)?.unwrap_or(0),
        input_cost: row.get::<_, Option<f64>>(15)?.unwrap_or(0.0),
        output_cost: row.get::<_, Option<f64>>(16)?.unwrap_or(0.0),
        cache_read_cost: row.get::<_, Option<f64>>(17)?.unwrap_or(0.0),
        cache_write_cost: row.get::<_, Option<f64>>(18)?.unwrap_or(0.0),
        reasoning_cost: row.get::<_, Option<f64>>(19)?.unwrap_or(0.0),
        total_cost: row.get::<_, Option<f64>>(20)?.unwrap_or(0.0),
        tags: parse_tags(&tags_json),
        observation_count: row.get(22)?,
        metadata: metadata_json,
        input_preview: row.get(24)?,
        output_preview: row.get(25)?,
        has_error: row.get::<_, Option<bool>>(26)?.unwrap_or(false),
    })
}

fn execute_span_query(
    conn: &Connection,
    sql: &str,
    bind_values: &[String],
) -> Result<Vec<SpanRow>, DuckdbError> {
    let mut stmt = conn.prepare(sql)?;
    let params: Vec<&dyn duckdb::ToSql> = bind_values
        .iter()
        .map(|v| v as &dyn duckdb::ToSql)
        .collect();
    let mut query_rows = stmt.query(params.as_slice())?;
    let mut rows = vec![];

    while let Some(row) = query_rows.next()? {
        rows.push(row_to_span(row)?);
    }

    Ok(rows)
}

fn row_to_span(row: &Row<'_>) -> Result<SpanRow, DuckdbError> {
    let start_time_micros: i64 = row.get(9)?;
    let end_time_micros: Option<i64> = row.get(10)?;
    let ingested_at_micros: i64 = row.get(38)?;

    Ok(SpanRow {
        trace_id: row.get(0)?,
        span_id: row.get(1)?,
        parent_span_id: row.get(2)?,
        span_name: row.get(3)?,
        span_kind: row.get(4)?,
        span_category: row.get(5)?,
        observation_type: row.get(6)?,
        framework: row.get(7)?,
        status_code: row.get(8)?,
        timestamp_start: micros_to_datetime(start_time_micros),
        timestamp_end: end_time_micros.map(micros_to_datetime),
        duration_ms: row.get(11)?,
        environment: row.get(12)?,
        resource_attributes: row.get(13)?,
        session_id: row.get(14)?,
        user_id: row.get(15)?,
        gen_ai_system: row.get(16)?,
        gen_ai_request_model: row.get(17)?,
        gen_ai_agent_name: row.get(18)?,
        gen_ai_finish_reasons: row
            .get::<_, Option<String>>(19)?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        gen_ai_usage_input_tokens: row.get::<_, Option<i64>>(20)?.unwrap_or(0),
        gen_ai_usage_output_tokens: row.get::<_, Option<i64>>(21)?.unwrap_or(0),
        gen_ai_usage_total_tokens: row.get::<_, Option<i64>>(22)?.unwrap_or(0),
        gen_ai_usage_cache_read_tokens: row.get::<_, Option<i64>>(23)?.unwrap_or(0),
        gen_ai_usage_cache_write_tokens: row.get::<_, Option<i64>>(24)?.unwrap_or(0),
        gen_ai_usage_reasoning_tokens: row.get::<_, Option<i64>>(25)?.unwrap_or(0),
        gen_ai_cost_input: row.get::<_, Option<f64>>(26)?.unwrap_or(0.0),
        gen_ai_cost_output: row.get::<_, Option<f64>>(27)?.unwrap_or(0.0),
        gen_ai_cost_cache_read: row.get::<_, Option<f64>>(28)?.unwrap_or(0.0),
        gen_ai_cost_cache_write: row.get::<_, Option<f64>>(29)?.unwrap_or(0.0),
        gen_ai_cost_reasoning: row.get::<_, Option<f64>>(30)?.unwrap_or(0.0),
        gen_ai_cost_total: row.get::<_, Option<f64>>(31)?.unwrap_or(0.0),
        gen_ai_usage_details: row.get(32)?,
        metadata: row.get(33)?,
        attributes: row.get(34)?,
        input_preview: row.get(35)?,
        output_preview: row.get(36)?,
        raw_span: row.get(37)?,
        ingested_at: micros_to_datetime(ingested_at_micros),
    })
}

fn execute_session_query(
    conn: &Connection,
    sql: &str,
    bind_values: &[String],
) -> Result<Vec<SessionRow>, DuckdbError> {
    let mut stmt = conn.prepare(sql)?;
    let params: Vec<&dyn duckdb::ToSql> = bind_values
        .iter()
        .map(|v| v as &dyn duckdb::ToSql)
        .collect();
    let mut query_rows = stmt.query(params.as_slice())?;
    let mut rows = vec![];

    while let Some(row) = query_rows.next()? {
        rows.push(row_to_session(row)?);
    }

    Ok(rows)
}

fn row_to_session(row: &Row<'_>) -> Result<SessionRow, DuckdbError> {
    let start_time_micros: i64 = row.get(3)?;
    let end_time_micros: Option<i64> = row.get(4)?;

    Ok(SessionRow {
        session_id: row.get(0)?,
        user_id: row.get(1)?,
        environment: row.get(2)?,
        start_time: micros_to_datetime(start_time_micros),
        end_time: end_time_micros.map(micros_to_datetime),
        trace_count: row.get(5)?,
        span_count: row.get(6)?,
        observation_count: row.get(7)?,
        input_tokens: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
        output_tokens: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
        total_tokens: row.get::<_, Option<i64>>(10)?.unwrap_or(0),
        cache_read_tokens: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
        cache_write_tokens: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
        reasoning_tokens: row.get::<_, Option<i64>>(13)?.unwrap_or(0),
        input_cost: row.get::<_, Option<f64>>(14)?.unwrap_or(0.0),
        output_cost: row.get::<_, Option<f64>>(15)?.unwrap_or(0.0),
        cache_read_cost: row.get::<_, Option<f64>>(16)?.unwrap_or(0.0),
        cache_write_cost: row.get::<_, Option<f64>>(17)?.unwrap_or(0.0),
        reasoning_cost: row.get::<_, Option<f64>>(18)?.unwrap_or(0.0),
        total_cost: row.get::<_, Option<f64>>(19)?.unwrap_or(0.0),
    })
}

// --- Delete operations ---

/// Delete multiple traces and all related spans and messages
pub fn delete_traces(
    conn: &Connection,
    project_id: &str,
    trace_ids: &[String],
) -> Result<u64, DuckdbError> {
    if trace_ids.is_empty() {
        return Ok(0);
    }

    let placeholders: Vec<&str> = trace_ids.iter().map(|_| "?").collect();
    let in_clause = placeholders.join(", ");

    in_transaction(conn, |conn| {
        let mut all_params: Vec<String> = Vec::with_capacity(1 + trace_ids.len());
        all_params.push(project_id.to_string());
        all_params.extend(trace_ids.iter().cloned());

        // Delete spans (events, links, and messages are embedded)
        let sql = format!(
            "DELETE FROM otel_spans WHERE project_id = ? AND trace_id IN ({})",
            in_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn duckdb::ToSql> =
            all_params.iter().map(|v| v as &dyn duckdb::ToSql).collect();
        let deleted = stmt.execute(params.as_slice())?;

        Ok(deleted as u64)
    })
}

/// Get trace_ids for given session_ids
pub fn get_trace_ids_for_sessions(
    conn: &Connection,
    project_id: &str,
    session_ids: &[String],
) -> Result<Vec<String>, DuckdbError> {
    if session_ids.is_empty() {
        return Ok(vec![]);
    }

    let placeholders: Vec<&str> = session_ids.iter().map(|_| "?").collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        "SELECT DISTINCT trace_id FROM otel_spans WHERE project_id = ? AND session_id IN ({})",
        in_clause
    );
    let mut stmt = conn.prepare(&sql)?;

    let mut all_params: Vec<String> = Vec::with_capacity(1 + session_ids.len());
    all_params.push(project_id.to_string());
    all_params.extend(session_ids.iter().cloned());

    let params: Vec<&dyn duckdb::ToSql> =
        all_params.iter().map(|v| v as &dyn duckdb::ToSql).collect();
    let mut rows = stmt.query(params.as_slice())?;

    let mut trace_ids: Vec<String> = vec![];
    while let Some(row) = rows.next()? {
        trace_ids.push(row.get(0)?);
    }

    Ok(trace_ids)
}

/// Delete multiple sessions by deleting all traces with those session_ids
pub fn delete_sessions(
    conn: &Connection,
    project_id: &str,
    session_ids: &[String],
) -> Result<u64, DuckdbError> {
    let trace_ids = get_trace_ids_for_sessions(conn, project_id, session_ids)?;
    if trace_ids.is_empty() {
        return Ok(0);
    }
    delete_traces(conn, project_id, &trace_ids)
}

/// Delete specific spans by (trace_id, span_id) pairs
pub fn delete_spans(
    conn: &Connection,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<u64, DuckdbError> {
    if spans.is_empty() {
        return Ok(0);
    }

    in_transaction(conn, |conn| {
        // Use tuple IN for efficient multi-pair matching
        let placeholders: Vec<&str> = spans.iter().map(|_| "(?, ?)").collect();
        let in_clause = placeholders.join(", ");

        let sql = format!(
            "DELETE FROM otel_spans WHERE project_id = ? AND (trace_id, span_id) IN ({})",
            in_clause
        );
        let mut stmt = conn.prepare(&sql)?;

        // Build params: project_id, then alternating trace_id/span_id pairs
        let mut all_params: Vec<String> = Vec::with_capacity(1 + spans.len() * 2);
        all_params.push(project_id.to_string());
        for (trace_id, span_id) in spans {
            all_params.push(trace_id.clone());
            all_params.push(span_id.clone());
        }

        let params: Vec<&dyn duckdb::ToSql> =
            all_params.iter().map(|v| v as &dyn duckdb::ToSql).collect();
        let deleted = stmt.execute(params.as_slice())?;

        Ok(deleted as u64)
    })
}

/// Delete all OTEL data for a project
pub fn delete_project_data(conn: &Connection, project_id: &str) -> Result<u64, DuckdbError> {
    in_transaction(conn, |conn| {
        // Delete spans (events, links, and messages are embedded)
        let deleted = conn.execute("DELETE FROM otel_spans WHERE project_id = ?", [project_id])?;
        Ok(deleted as u64)
    })
}

/// Count spans grouped by project for a set of project IDs.
pub fn count_spans_by_project(
    conn: &Connection,
    project_ids: &[String],
) -> Result<std::collections::HashMap<String, u64>, DuckdbError> {
    use std::collections::HashMap;

    if project_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders: Vec<&str> = project_ids.iter().map(|_| "?").collect();
    let sql = format!(
        "SELECT project_id, COUNT(span_id) FROM otel_spans_v WHERE project_id IN ({}) GROUP BY project_id",
        placeholders.join(", ")
    );

    let params: Vec<&dyn duckdb::ToSql> = project_ids
        .iter()
        .map(|v| v as &dyn duckdb::ToSql)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params.as_slice())?;

    let mut result = HashMap::new();
    while let Some(row) = rows.next()? {
        let project_id: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        result.insert(project_id, count as u64);
    }

    Ok(result)
}

// --- Filter options queries ---

/// Result for filter option value with count
#[derive(Debug)]
pub struct FilterOptionRow {
    pub value: String,
    pub count: u64,
}

/// Columns allowed for filter options queries (must be categorical/low-cardinality)
/// Maps view column names to spans table column names
const TRACE_FILTER_OPTION_COLUMNS: &[(&str, &str)] = &[
    ("environment", "environment"),
    ("trace_name", "span_name"),
    ("user_id", "user_id"),
    ("session_id", "session_id"),
];

/// Get distinct values with counts for trace filter options
/// Optimized: queries otel_spans directly instead of the view, uses approximate counts
pub fn get_trace_filter_options(
    conn: &Connection,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<std::collections::HashMap<String, Vec<FilterOptionRow>>, DuckdbError> {
    use std::collections::HashMap;

    let mut results: HashMap<String, Vec<FilterOptionRow>> = HashMap::new();

    // Build base WHERE clause once
    let mut base_conditions = vec!["project_id = ?".to_string()];
    let mut base_params: Vec<String> = vec![project_id.to_string()];

    if let Some(from) = from_timestamp {
        base_conditions.push("timestamp_start >= ?".to_string());
        base_params.push(from.to_rfc3339());
    }

    if let Some(to) = to_timestamp {
        base_conditions.push("timestamp_start <= ?".to_string());
        base_params.push(to.to_rfc3339());
    }

    let base_where = base_conditions.join(" AND ");

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

        // For trace_name, only look at root spans (parent_span_id IS NULL)
        // Other columns can use all spans
        let extra_condition = if column == "trace_name" {
            " AND parent_span_id IS NULL"
        } else {
            ""
        };

        // Query from deduplicated view with approximate trace count
        let sql = format!(
            r#"
            SELECT {col}, APPROX_COUNT_DISTINCT(trace_id) as cnt
            FROM otel_spans_v
            WHERE {base_where} AND {col} IS NOT NULL{extra}
            GROUP BY {col}
            ORDER BY cnt DESC
            LIMIT {limit}
            "#,
            base_where = base_where,
            col = span_column,
            extra = extra_condition,
            limit = QUERY_MAX_FILTER_SUGGESTIONS
        );

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn duckdb::ToSql> = base_params
            .iter()
            .map(|v| v as &dyn duckdb::ToSql)
            .collect();
        let mut rows = stmt.query(param_refs.as_slice())?;

        let mut options: Vec<FilterOptionRow> = vec![];
        while let Some(row) = rows.next()? {
            let value: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            options.push(FilterOptionRow {
                value,
                count: count as u64,
            });
        }

        results.insert(column.clone(), options);
    }

    Ok(results)
}

/// Get distinct tag values with counts from trace tags array
/// Optimized: queries otel_spans directly instead of the view, uses approximate counts
pub fn get_trace_tags_options(
    conn: &Connection,
    project_id: &str,
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<Vec<FilterOptionRow>, DuckdbError> {
    // Build WHERE clause
    let mut conditions = vec!["project_id = ?".to_string()];
    let mut params: Vec<String> = vec![project_id.to_string()];

    if let Some(from) = from_timestamp {
        conditions.push("timestamp_start >= ?".to_string());
        params.push(from.to_rfc3339());
    }

    if let Some(to) = to_timestamp {
        conditions.push("timestamp_start <= ?".to_string());
        params.push(to.to_rfc3339());
    }

    let where_clause = conditions.join(" AND ");

    // Query from deduplicated view with approximate trace count per tag
    // tags is stored as JSON array string (VARCHAR), so parse with from_json first
    let sql = format!(
        r#"
        SELECT tag, APPROX_COUNT_DISTINCT(trace_id) as cnt
        FROM (
            SELECT trace_id, UNNEST(from_json(tags, '["VARCHAR"]')) as tag
            FROM otel_spans_v
            WHERE {} AND tags IS NOT NULL AND tags != '[]'
        )
        WHERE tag IS NOT NULL AND tag != ''
        GROUP BY tag
        ORDER BY cnt DESC
        LIMIT {}
        "#,
        where_clause, QUERY_MAX_FILTER_SUGGESTIONS
    );

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn duckdb::ToSql> =
        params.iter().map(|v| v as &dyn duckdb::ToSql).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;

    let mut options: Vec<FilterOptionRow> = vec![];
    while let Some(row) = rows.next()? {
        let value: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        options.push(FilterOptionRow {
            value,
            count: count as u64,
        });
    }

    Ok(options)
}

/// Columns allowed for span/observation filter options queries
/// These are low-cardinality columns suitable for dropdown filters
const SPAN_FILTER_OPTION_COLUMNS: &[&str] = &[
    "observation_type",
    "gen_ai_request_model",
    "framework",
    "status_code",
    "span_category",
    "environment",
    "gen_ai_agent_name",
    "gen_ai_system",
];

/// Get distinct values with counts for span filter options
/// Used by the Observations page to populate filter dropdowns
pub fn get_span_filter_options(
    conn: &Connection,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
    observations_only: bool,
) -> Result<std::collections::HashMap<String, Vec<FilterOptionRow>>, DuckdbError> {
    use std::collections::HashMap;

    let mut results: HashMap<String, Vec<FilterOptionRow>> = HashMap::new();

    // Build base WHERE clause
    let mut base_conditions = vec!["project_id = ?".to_string()];
    let mut base_params: Vec<String> = vec![project_id.to_string()];

    if let Some(from) = from_timestamp {
        base_conditions.push("timestamp_start >= ?".to_string());
        base_params.push(from.to_rfc3339());
    }

    if let Some(to) = to_timestamp {
        base_conditions.push("timestamp_start <= ?".to_string());
        base_params.push(to.to_rfc3339());
    }

    // Filter to observations only (GenAI spans)
    if observations_only {
        base_conditions.push("observation_type != 'span'".to_string());
    }

    let base_where = base_conditions.join(" AND ");

    for column in columns {
        // Validate column is allowed
        if !SPAN_FILTER_OPTION_COLUMNS.contains(&column.as_str()) {
            continue;
        }

        // Query distinct values with counts from deduplicated view
        let sql = format!(
            r#"
            SELECT {col}, COUNT(*) as cnt
            FROM otel_spans_v
            WHERE {base_where} AND {col} IS NOT NULL
            GROUP BY {col}
            ORDER BY cnt DESC
            LIMIT {limit}
            "#,
            base_where = base_where,
            col = column,
            limit = QUERY_MAX_FILTER_SUGGESTIONS
        );

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn duckdb::ToSql> = base_params
            .iter()
            .map(|v| v as &dyn duckdb::ToSql)
            .collect();
        let mut rows = stmt.query(param_refs.as_slice())?;

        let mut options: Vec<FilterOptionRow> = vec![];
        while let Some(row) = rows.next()? {
            let value: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            options.push(FilterOptionRow {
                value,
                count: count as u64,
            });
        }

        results.insert(column.clone(), options);
    }

    Ok(results)
}

/// Columns allowed for session filter options queries
/// These are low-cardinality columns suitable for dropdown filters
const SESSION_FILTER_OPTION_COLUMNS: &[&str] = &["environment", "user_id"];

/// Get distinct values with counts for session filter options
/// Used by the Sessions page to populate filter dropdowns
/// Counts are approximate for performance (number of sessions with that value)
pub fn get_session_filter_options(
    conn: &Connection,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<std::collections::HashMap<String, Vec<FilterOptionRow>>, DuckdbError> {
    use std::collections::HashMap;

    let mut results: HashMap<String, Vec<FilterOptionRow>> = HashMap::new();

    // Build base WHERE clause
    let mut base_conditions = vec![
        "project_id = ?".to_string(),
        "session_id IS NOT NULL".to_string(),
    ];
    let mut base_params: Vec<String> = vec![project_id.to_string()];

    if let Some(from) = from_timestamp {
        base_conditions.push("timestamp_start >= ?".to_string());
        base_params.push(from.to_rfc3339());
    }

    if let Some(to) = to_timestamp {
        base_conditions.push("timestamp_start <= ?".to_string());
        base_params.push(to.to_rfc3339());
    }

    let base_where = base_conditions.join(" AND ");

    for column in columns {
        // Validate column is allowed
        if !SESSION_FILTER_OPTION_COLUMNS.contains(&column.as_str()) {
            continue;
        }

        // Query distinct values with approximate session counts from deduplicated view
        let sql = format!(
            r#"
            SELECT {col}, APPROX_COUNT_DISTINCT(session_id) as cnt
            FROM otel_spans_v
            WHERE {base_where} AND {col} IS NOT NULL
            GROUP BY {col}
            ORDER BY cnt DESC
            LIMIT {limit}
            "#,
            base_where = base_where,
            col = column,
            limit = QUERY_MAX_FILTER_SUGGESTIONS
        );

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn duckdb::ToSql> = base_params
            .iter()
            .map(|v| v as &dyn duckdb::ToSql)
            .collect();
        let mut rows = stmt.query(param_refs.as_slice())?;

        let mut options: Vec<FilterOptionRow> = vec![];
        while let Some(row) = rows.next()? {
            let value: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            options.push(FilterOptionRow {
                value,
                count: count as u64,
            });
        }

        results.insert(column.clone(), options);
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // Integration tests for leaf generation span filtering (cost deduplication)
    // ============================================================================

    use crate::core::storage::AppStorage;
    use crate::data::duckdb::models::ObservationType;
    use crate::data::duckdb::repositories::span::insert_batch;
    use crate::data::duckdb::{DuckdbService, NormalizedSpan};
    use tempfile::TempDir;

    async fn create_test_service() -> (TempDir, DuckdbService) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let duckdb_dir = temp_dir.path().join("duckdb");
        tokio::fs::create_dir_all(&duckdb_dir)
            .await
            .expect("Failed to create duckdb dir");
        let storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = DuckdbService::init(&storage)
            .await
            .expect("Failed to init analytics service");
        (temp_dir, service)
    }

    fn make_generation_span(
        project_id: &str,
        trace_id: &str,
        span_id: &str,
        parent_span_id: Option<&str>,
        cost: f64,
        tokens: i64,
    ) -> NormalizedSpan {
        NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: parent_span_id.map(String::from),
            span_name: format!("generation-{}", span_id),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            gen_ai_cost_total: cost,
            gen_ai_usage_total_tokens: tokens,
            gen_ai_usage_input_tokens: tokens / 2,
            gen_ai_usage_output_tokens: tokens / 2,
            ..Default::default()
        }
    }

    fn make_agent_span(
        project_id: &str,
        trace_id: &str,
        span_id: &str,
        parent_span_id: Option<&str>,
    ) -> NormalizedSpan {
        NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: parent_span_id.map(String::from),
            span_name: format!("agent-{}", span_id),
            observation_type: Some(ObservationType::Agent),
            timestamp_start: Utc::now(),
            ..Default::default()
        }
    }

    /// Test nested generation spans (Strands pattern):
    /// Agent -> Generation(parent) -> Generation(child)
    /// Both generations have the same cost/tokens, but only the leaf should be counted.
    #[tokio::test]
    async fn test_get_trace_nested_generations_no_double_count() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-nested";

        // Strands pattern: agent -> parent_gen -> child_gen
        // Both generations have $0.01 cost, but we should only count $0.01 total
        let spans = vec![
            make_agent_span(project_id, trace_id, "agent-1", None),
            make_generation_span(
                project_id,
                trace_id,
                "parent-gen",
                Some("agent-1"),
                0.01,
                1000,
            ),
            make_generation_span(
                project_id,
                trace_id,
                "child-gen",
                Some("parent-gen"),
                0.01,
                1000,
            ),
        ];

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Should count only the leaf generation (child-gen), not both
        assert_eq!(
            trace.total_cost, 0.01,
            "Should not double-count nested generations"
        );
        assert_eq!(trace.total_tokens, 1000, "Should not double-count tokens");
    }

    /// Test non-nested generation spans (LangGraph/CrewAI pattern):
    /// Agent -> Generation1, Agent -> Generation2 (siblings)
    /// Both generations should be counted since neither is a parent of the other.
    #[tokio::test]
    async fn test_get_trace_sibling_generations_both_counted() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-siblings";

        // LangGraph pattern: agent -> gen1, agent -> gen2 (siblings, not nested)
        // Both should be counted: $0.01 + $0.02 = $0.03
        let spans = vec![
            make_agent_span(project_id, trace_id, "agent-1", None),
            make_generation_span(project_id, trace_id, "gen-1", Some("agent-1"), 0.01, 1000),
            make_generation_span(project_id, trace_id, "gen-2", Some("agent-1"), 0.02, 2000),
        ];

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Should count both sibling generations
        assert_eq!(
            trace.total_cost, 0.03,
            "Should count all sibling generations"
        );
        assert_eq!(trace.total_tokens, 3000, "Should count all sibling tokens");
    }

    /// Test deeply nested generations (3 levels):
    /// Agent -> Gen1 -> Gen2 -> Gen3
    /// Only Gen1 (root generation) should be counted.
    #[tokio::test]
    async fn test_get_trace_deeply_nested_only_leaf_counted() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-deep";

        // 3-level nesting: only Gen3 (leaf) should be counted
        let spans = vec![
            make_agent_span(project_id, trace_id, "agent-1", None),
            make_generation_span(project_id, trace_id, "gen-1", Some("agent-1"), 0.01, 1000),
            make_generation_span(project_id, trace_id, "gen-2", Some("gen-1"), 0.01, 1000),
            make_generation_span(project_id, trace_id, "gen-3", Some("gen-2"), 0.01, 1000),
        ];

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Only leaf generation (gen-3) should be counted
        assert_eq!(trace.total_cost, 0.01, "Should only count leaf generation");
        assert_eq!(
            trace.total_tokens, 1000,
            "Should only count leaf generation tokens"
        );
    }

    /// Test mixed pattern: some nested, some siblings
    /// Agent -> NestedParent -> NestedChild (nested)
    /// Agent -> Standalone (sibling to NestedParent)
    /// Should count NestedChild + Standalone
    #[tokio::test]
    async fn test_get_trace_mixed_nested_and_siblings() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-mixed";

        let spans = vec![
            make_agent_span(project_id, trace_id, "agent-1", None),
            // Nested pair: parent-gen -> child-gen
            make_generation_span(
                project_id,
                trace_id,
                "parent-gen",
                Some("agent-1"),
                0.01,
                1000,
            ),
            make_generation_span(
                project_id,
                trace_id,
                "child-gen",
                Some("parent-gen"),
                0.01,
                1000,
            ),
            // Standalone sibling
            make_generation_span(
                project_id,
                trace_id,
                "standalone-gen",
                Some("agent-1"),
                0.02,
                2000,
            ),
        ];

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Should count child-gen (0.01) + standalone-gen (0.02) = 0.03
        // NOT parent-gen (excluded because it's parent of child-gen)
        assert_eq!(trace.total_cost, 0.03, "Should count leaf + standalone");
        assert_eq!(
            trace.total_tokens, 3000,
            "Should count leaf + standalone tokens"
        );
    }

    /// Test session aggregation with nested generations
    #[tokio::test]
    async fn test_get_session_nested_generations_no_double_count() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let session_id = "session-1";
        let trace_id = "trace-session";

        // Create spans with session_id
        let mut spans = vec![
            make_agent_span(project_id, trace_id, "agent-1", None),
            make_generation_span(
                project_id,
                trace_id,
                "parent-gen",
                Some("agent-1"),
                0.01,
                1000,
            ),
            make_generation_span(
                project_id,
                trace_id,
                "child-gen",
                Some("parent-gen"),
                0.01,
                1000,
            ),
        ];
        for span in &mut spans {
            span.session_id = Some(session_id.to_string());
        }

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_session(&conn, project_id, session_id).expect("Query should succeed");
        let session = result.expect("Session should exist");

        // Session aggregation should also only count leaf generations
        assert_eq!(session.total_cost, 0.01, "Session should not double-count");
        assert_eq!(
            session.total_tokens, 1000,
            "Session should not double-count tokens"
        );
    }

    /// Test list_traces with nested generations
    #[tokio::test]
    async fn test_list_traces_nested_generations_no_double_count() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        // Create two traces: one with nested generations, one with siblings
        let mut spans = vec![
            // Trace 1: nested generations (should count $0.01)
            make_agent_span(project_id, "trace-1", "agent-1", None),
            make_generation_span(
                project_id,
                "trace-1",
                "parent-gen",
                Some("agent-1"),
                0.01,
                1000,
            ),
            make_generation_span(
                project_id,
                "trace-1",
                "child-gen",
                Some("parent-gen"),
                0.01,
                1000,
            ),
            // Trace 2: sibling generations (should count $0.03)
            make_agent_span(project_id, "trace-2", "agent-2", None),
            make_generation_span(project_id, "trace-2", "gen-a", Some("agent-2"), 0.01, 1000),
            make_generation_span(project_id, "trace-2", "gen-b", Some("agent-2"), 0.02, 2000),
        ];

        // Set timestamps for ordering
        let base_ts = Utc::now();
        spans[0].timestamp_start = base_ts;
        spans[1].timestamp_start = base_ts;
        spans[2].timestamp_start = base_ts;
        spans[3].timestamp_start = base_ts + chrono::Duration::seconds(1);
        spans[4].timestamp_start = base_ts + chrono::Duration::seconds(1);
        spans[5].timestamp_start = base_ts + chrono::Duration::seconds(1);

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let params = ListTracesParams {
            project_id: project_id.to_string(),
            page: 0,
            limit: 100,
            include_nongenai: true, // Test spans don't have gen_ai_system set
            ..Default::default()
        };
        let (traces, _total) = list_traces(&conn, &params).expect("Query should succeed");

        assert_eq!(traces.len(), 2, "Should have 2 traces");

        // Find traces by ID
        let trace1 = traces
            .iter()
            .find(|t| t.trace_id == "trace-1")
            .expect("Trace 1 should exist");
        let trace2 = traces
            .iter()
            .find(|t| t.trace_id == "trace-2")
            .expect("Trace 2 should exist");

        // Trace 1: nested - should count only leaf ($0.01)
        assert_eq!(trace1.total_cost, 0.01, "Trace 1 should not double-count");
        assert_eq!(
            trace1.total_tokens, 1000,
            "Trace 1 tokens should not double-count"
        );

        // Trace 2: siblings - should count both ($0.03)
        assert_eq!(
            trace2.total_cost, 0.03,
            "Trace 2 should count both siblings"
        );
        assert_eq!(
            trace2.total_tokens, 3000,
            "Trace 2 tokens should count both"
        );
    }

    /// Test cache tokens with nested generations (Strands pattern)
    /// Cache tokens only exist on parent spans, not leaf spans.
    /// Cache tokens should be summed from ALL generations (not leaf-only).
    #[tokio::test]
    async fn test_get_trace_cache_tokens_from_generation_leaf() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-cache";

        // Strands pattern:
        // - Parent "chat" span (generation) has cache tokens
        // - Child "chat us.amazon..." span is Span type (not Generation) in real data
        let parent = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "parent-chat".to_string(),
            parent_span_id: None,
            span_name: "chat".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 100,
            gen_ai_usage_output_tokens: 50,
            gen_ai_usage_total_tokens: 150,
            gen_ai_usage_cache_read_tokens: 100,
            gen_ai_usage_cache_write_tokens: 200,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        // Real Strands: child is Span type, excluded by Path 2 (trace has generation)
        let child = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "child-bedrock".to_string(),
            parent_span_id: Some("parent-chat".to_string()),
            span_name: "chat us.amazon.nova".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 100,
            gen_ai_usage_output_tokens: 50,
            gen_ai_usage_total_tokens: 0,
            gen_ai_usage_cache_read_tokens: 0,
            gen_ai_usage_cache_write_tokens: 0,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[parent, child]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Parent generation is leaf (no generation child) - all its data counted
        assert_eq!(trace.input_tokens, 100, "Input tokens from generation leaf");
        assert_eq!(
            trace.output_tokens, 50,
            "Output tokens from generation leaf"
        );
        assert_eq!(
            trace.cache_read_tokens, 100,
            "Cache read from generation leaf"
        );
        assert_eq!(
            trace.cache_write_tokens, 200,
            "Cache write from generation leaf"
        );
        assert_eq!(trace.total_cost, 0.01, "Cost from generation leaf");
    }

    /// Test list_sessions with nested generations
    #[tokio::test]
    async fn test_list_sessions_nested_generations_no_double_count() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        // Session 1: nested generations (should count $0.01)
        let mut spans1 = vec![
            make_agent_span(project_id, "trace-s1", "agent-1", None),
            make_generation_span(
                project_id,
                "trace-s1",
                "parent-gen",
                Some("agent-1"),
                0.01,
                1000,
            ),
            make_generation_span(
                project_id,
                "trace-s1",
                "child-gen",
                Some("parent-gen"),
                0.01,
                1000,
            ),
        ];
        for span in &mut spans1 {
            span.session_id = Some("session-1".to_string());
        }

        // Session 2: sibling generations (should count $0.03)
        let mut spans2 = vec![
            make_agent_span(project_id, "trace-s2", "agent-2", None),
            make_generation_span(project_id, "trace-s2", "gen-a", Some("agent-2"), 0.01, 1000),
            make_generation_span(project_id, "trace-s2", "gen-b", Some("agent-2"), 0.02, 2000),
        ];
        for span in &mut spans2 {
            span.session_id = Some("session-2".to_string());
        }

        // Set timestamps for ordering
        let base_ts = Utc::now();
        for span in &mut spans1 {
            span.timestamp_start = base_ts;
        }
        for span in &mut spans2 {
            span.timestamp_start = base_ts + chrono::Duration::seconds(1);
        }

        {
            let conn = analytics.conn();
            let all_spans: Vec<_> = spans1.into_iter().chain(spans2).collect();
            insert_batch(&conn, &all_spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let params = ListSessionsParams {
            project_id: project_id.to_string(),
            page: 0,
            limit: 100,
            ..Default::default()
        };
        let (sessions, _total) = list_sessions(&conn, &params).expect("Query should succeed");

        assert_eq!(sessions.len(), 2, "Should have 2 sessions");

        let session1 = sessions
            .iter()
            .find(|s| s.session_id == "session-1")
            .expect("Session 1 should exist");
        let session2 = sessions
            .iter()
            .find(|s| s.session_id == "session-2")
            .expect("Session 2 should exist");

        // Session 1: nested - should count only leaf ($0.01)
        assert_eq!(
            session1.total_cost, 0.01,
            "Session 1 should not double-count"
        );
        assert_eq!(
            session1.total_tokens, 1000,
            "Session 1 tokens should not double-count"
        );

        // Session 2: siblings - should count both ($0.03)
        assert_eq!(
            session2.total_cost, 0.03,
            "Session 2 should count both siblings"
        );
        assert_eq!(
            session2.total_tokens, 3000,
            "Session 2 tokens should count both"
        );
    }

    /// Test get_traces_for_session with nested generations
    #[tokio::test]
    async fn test_get_traces_for_session_nested_generations_no_double_count() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let session_id = "session-traces";

        // Create two traces in the same session with different patterns
        // Trace 1: nested generations (should count $0.01)
        let mut spans1 = vec![
            make_agent_span(project_id, "trace-1", "agent-1", None),
            make_generation_span(
                project_id,
                "trace-1",
                "parent-gen",
                Some("agent-1"),
                0.01,
                1000,
            ),
            make_generation_span(
                project_id,
                "trace-1",
                "child-gen",
                Some("parent-gen"),
                0.01,
                1000,
            ),
        ];
        // Trace 2: sibling generations (should count $0.03)
        let mut spans2 = vec![
            make_agent_span(project_id, "trace-2", "agent-2", None),
            make_generation_span(project_id, "trace-2", "gen-a", Some("agent-2"), 0.01, 1000),
            make_generation_span(project_id, "trace-2", "gen-b", Some("agent-2"), 0.02, 2000),
        ];

        // All in same session
        for span in spans1.iter_mut().chain(spans2.iter_mut()) {
            span.session_id = Some(session_id.to_string());
        }

        // Set timestamps for ordering
        let base_ts = Utc::now();
        for span in &mut spans1 {
            span.timestamp_start = base_ts;
        }
        for span in &mut spans2 {
            span.timestamp_start = base_ts + chrono::Duration::seconds(1);
        }

        {
            let conn = analytics.conn();
            let all_spans: Vec<_> = spans1.into_iter().chain(spans2).collect();
            insert_batch(&conn, &all_spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let traces =
            get_traces_for_session(&conn, project_id, session_id).expect("Query should succeed");

        assert_eq!(traces.len(), 2, "Should have 2 traces");

        let trace1 = traces
            .iter()
            .find(|t| t.trace_id == "trace-1")
            .expect("Trace 1 should exist");
        let trace2 = traces
            .iter()
            .find(|t| t.trace_id == "trace-2")
            .expect("Trace 2 should exist");

        // Trace 1: nested - should count only leaf ($0.01)
        assert_eq!(trace1.total_cost, 0.01, "Trace 1 should not double-count");
        assert_eq!(
            trace1.total_tokens, 1000,
            "Trace 1 tokens should not double-count"
        );

        // Trace 2: siblings - should count both ($0.03)
        assert_eq!(
            trace2.total_cost, 0.03,
            "Trace 2 should count both siblings"
        );
        assert_eq!(
            trace2.total_tokens, 3000,
            "Trace 2 tokens should count both"
        );
    }

    /// Test cache tokens in list_sessions
    #[tokio::test]
    async fn test_list_sessions_cache_tokens_from_generation_leaf() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let session_id = "session-cache";

        // Strands pattern: parent generation has cache tokens, child is Span type
        let parent = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: "trace-cache".to_string(),
            span_id: "parent-chat".to_string(),
            parent_span_id: None,
            span_name: "chat".to_string(),
            session_id: Some(session_id.to_string()),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 100,
            gen_ai_usage_output_tokens: 50,
            gen_ai_usage_total_tokens: 150,
            gen_ai_usage_cache_read_tokens: 100,
            gen_ai_usage_cache_write_tokens: 200,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        // Real Strands: child is Span type
        let child = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: "trace-cache".to_string(),
            span_id: "child-bedrock".to_string(),
            parent_span_id: Some("parent-chat".to_string()),
            span_name: "chat us.amazon.nova".to_string(),
            session_id: Some(session_id.to_string()),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 100,
            gen_ai_usage_output_tokens: 50,
            gen_ai_usage_total_tokens: 0,
            gen_ai_usage_cache_read_tokens: 0,
            gen_ai_usage_cache_write_tokens: 0,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[parent, child]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let params = ListSessionsParams {
            project_id: project_id.to_string(),
            page: 0,
            limit: 100,
            ..Default::default()
        };
        let (sessions, _total) = list_sessions(&conn, &params).expect("Query should succeed");

        assert_eq!(sessions.len(), 1, "Should have 1 session");
        let session = &sessions[0];

        // Parent generation is leaf (no generation child) - all its data counted
        assert_eq!(
            session.input_tokens, 100,
            "Input tokens from generation leaf"
        );
        assert_eq!(
            session.output_tokens, 50,
            "Output tokens from generation leaf"
        );
        assert_eq!(
            session.cache_read_tokens, 100,
            "Cache read from generation leaf"
        );
        assert_eq!(
            session.cache_write_tokens, 200,
            "Cache write from generation leaf"
        );
        assert_eq!(session.total_cost, 0.01, "Cost from generation leaf");
    }

    /// Test get_traces_for_session cache tokens
    #[tokio::test]
    async fn test_get_traces_for_session_cache_tokens_from_generation_leaf() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let session_id = "session-cache-traces";

        // Strands pattern in two traces: parent generation + child Span
        let parent1 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: "trace-1".to_string(),
            span_id: "parent-1".to_string(),
            parent_span_id: None,
            span_name: "chat".to_string(),
            session_id: Some(session_id.to_string()),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 100,
            gen_ai_usage_cache_read_tokens: 50,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        let child1 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: "trace-1".to_string(),
            span_id: "child-1".to_string(),
            parent_span_id: Some("parent-1".to_string()),
            span_name: "bedrock".to_string(),
            session_id: Some(session_id.to_string()),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 100,
            gen_ai_usage_cache_read_tokens: 0,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        let parent2 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: "trace-2".to_string(),
            span_id: "parent-2".to_string(),
            parent_span_id: None,
            span_name: "chat".to_string(),
            session_id: Some(session_id.to_string()),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now() + chrono::Duration::seconds(1),
            gen_ai_usage_input_tokens: 200,
            gen_ai_usage_cache_read_tokens: 75,
            gen_ai_cost_total: 0.02,
            ..Default::default()
        };

        let child2 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: "trace-2".to_string(),
            span_id: "child-2".to_string(),
            parent_span_id: Some("parent-2".to_string()),
            span_name: "bedrock".to_string(),
            session_id: Some(session_id.to_string()),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now() + chrono::Duration::seconds(1),
            gen_ai_usage_input_tokens: 200,
            gen_ai_usage_cache_read_tokens: 0,
            gen_ai_cost_total: 0.02,
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[parent1, child1, parent2, child2])
                .expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let traces =
            get_traces_for_session(&conn, project_id, session_id).expect("Query should succeed");

        assert_eq!(traces.len(), 2, "Should have 2 traces");

        let trace1 = traces
            .iter()
            .find(|t| t.trace_id == "trace-1")
            .expect("Trace 1 should exist");
        let trace2 = traces
            .iter()
            .find(|t| t.trace_id == "trace-2")
            .expect("Trace 2 should exist");

        // Each trace counts its generation leaf (parent has no generation child)
        assert_eq!(
            trace1.input_tokens, 100,
            "Trace 1 input from generation leaf"
        );
        assert_eq!(
            trace1.cache_read_tokens, 50,
            "Trace 1 cache from generation leaf"
        );
        assert_eq!(trace1.total_cost, 0.01, "Trace 1 cost from generation leaf");

        assert_eq!(
            trace2.input_tokens, 200,
            "Trace 2 input from generation leaf"
        );
        assert_eq!(
            trace2.cache_read_tokens, 75,
            "Trace 2 cache from generation leaf"
        );
        assert_eq!(trace2.total_cost, 0.02, "Trace 2 cost from generation leaf");
    }

    /// Test session with multiple traces having different nesting patterns
    #[tokio::test]
    async fn test_session_multiple_traces_aggregation() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let session_id = "session-multi";

        // Trace 1: nested - only root counted ($0.01, 1000 tokens)
        let mut spans1 = vec![
            make_generation_span(project_id, "trace-1", "parent", None, 0.01, 1000),
            make_generation_span(project_id, "trace-1", "child", Some("parent"), 0.01, 1000),
        ];

        // Trace 2: siblings - both are roots ($0.01 + $0.02 = $0.03, 3000 tokens)
        let mut spans2 = vec![
            make_generation_span(project_id, "trace-2", "gen-a", None, 0.01, 1000),
            make_generation_span(project_id, "trace-2", "gen-b", None, 0.02, 2000),
        ];

        // Trace 3: single root ($0.05, 500 tokens)
        let mut spans3 = vec![make_generation_span(
            project_id, "trace-3", "single", None, 0.05, 500,
        )];

        for span in spans1
            .iter_mut()
            .chain(spans2.iter_mut())
            .chain(spans3.iter_mut())
        {
            span.session_id = Some(session_id.to_string());
        }

        // Set distinct timestamps
        let base_ts = Utc::now();
        spans1[0].timestamp_start = base_ts;
        spans1[1].timestamp_start = base_ts;
        spans2[0].timestamp_start = base_ts + chrono::Duration::seconds(1);
        spans2[1].timestamp_start = base_ts + chrono::Duration::seconds(1);
        spans3[0].timestamp_start = base_ts + chrono::Duration::seconds(2);

        {
            let conn = analytics.conn();
            let all_spans: Vec<_> = spans1.into_iter().chain(spans2).chain(spans3).collect();
            insert_batch(&conn, &all_spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_session(&conn, project_id, session_id).expect("Query should succeed");
        let session = result.expect("Session should exist");

        // Session total: trace1($0.01) + trace2($0.03) + trace3($0.05) = $0.09
        assert_eq!(
            session.total_cost, 0.09,
            "Session should sum all root generation costs"
        );
        // Tokens: trace1(1000) + trace2(3000) + trace3(500) = 4500
        assert_eq!(
            session.total_tokens, 4500,
            "Session should sum all root generation tokens"
        );
        assert_eq!(session.trace_count, 3, "Session should have 3 traces");
    }

    /// Test deeply nested (4 levels) generations - only root counted
    #[tokio::test]
    async fn test_deeply_nested_4_levels() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-4-levels";

        // 4-level nesting: gen1 -> gen2 -> gen3 -> gen4
        // Only gen1 is a root (has no generation parent)
        let spans = vec![
            make_generation_span(project_id, trace_id, "gen-1", None, 0.10, 1000),
            make_generation_span(project_id, trace_id, "gen-2", Some("gen-1"), 0.10, 1000),
            make_generation_span(project_id, trace_id, "gen-3", Some("gen-2"), 0.10, 1000),
            make_generation_span(project_id, trace_id, "gen-4", Some("gen-3"), 0.10, 1000),
        ];

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Only gen-1 (root) counted = $0.10, 1000 tokens
        assert_eq!(trace.total_cost, 0.10, "Should count only root generation");
        assert_eq!(
            trace.total_tokens, 1000,
            "Should count only root generation tokens"
        );
    }

    /// Test trace with no generation spans (only agent/tool spans)
    #[tokio::test]
    async fn test_trace_no_generations() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-no-gen";

        let spans = vec![
            make_agent_span(project_id, trace_id, "agent-1", None),
            make_agent_span(project_id, trace_id, "agent-2", Some("agent-1")),
        ];

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // No generations = zero cost/tokens
        assert_eq!(trace.total_cost, 0.0, "No generations = zero cost");
        assert_eq!(trace.total_tokens, 0, "No generations = zero tokens");
    }

    /// Test multiple independent generation roots in same trace
    #[tokio::test]
    async fn test_multiple_independent_generation_roots() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-multi-roots";

        // gen-1 is root (parent agent-1 is not a generation)
        // gen-2 is NOT root (parent gen-1 is a generation)
        // gen-3 is root (parent agent-2 is not a generation)
        // gen-4 is NOT root (parent gen-3 is a generation)
        // Total = gen-1 ($0.01) + gen-3 ($0.02) = $0.03
        let spans = vec![
            make_agent_span(project_id, trace_id, "agent-1", None),
            make_generation_span(project_id, trace_id, "gen-1", Some("agent-1"), 0.01, 100),
            make_generation_span(project_id, trace_id, "gen-2", Some("gen-1"), 0.01, 100),
            make_agent_span(project_id, trace_id, "agent-2", None),
            make_generation_span(project_id, trace_id, "gen-3", Some("agent-2"), 0.02, 200),
            make_generation_span(project_id, trace_id, "gen-4", Some("gen-3"), 0.02, 200),
        ];

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Two roots: gen-1 + gen-3 = $0.01 + $0.02 = $0.03
        assert_eq!(
            trace.total_cost, 0.03,
            "Should sum costs from generation roots"
        );
        assert_eq!(
            trace.total_tokens, 300,
            "Should sum tokens from generation roots"
        );
    }

    /// Test nested generations where only root is counted
    #[tokio::test]
    async fn test_nested_generations_only_leaf_counted() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-nested-gen";

        // Parent generation has a generation child → excluded
        // Child generation has no generation child → included (leaf)
        let spans = vec![
            make_generation_span(project_id, trace_id, "parent", None, 0.10, 1000),
            make_generation_span(project_id, trace_id, "child", Some("parent"), 0.01, 100),
        ];

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Only leaf generation is counted
        assert_eq!(trace.total_cost, 0.01, "Only leaf generation cost");
        assert_eq!(trace.total_tokens, 100, "Only leaf generation tokens");
    }

    /// Test realistic Strands data where parent "chat" has complete data including cache.
    /// Child "bedrock" is observation_type=Span (not Generation) in real Strands data,
    /// so the parent generation (which has no generation child) is the leaf and gets counted.
    #[tokio::test]
    async fn test_chain_mixed_values_across_levels() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-mixed-values";

        // Realistic Strands: parent "chat" is generation with ALL data
        let parent = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "parent".to_string(),
            parent_span_id: None,
            span_name: "chat".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 1000,
            gen_ai_usage_output_tokens: 500,
            gen_ai_usage_cache_read_tokens: 500,
            gen_ai_usage_cache_write_tokens: 100,
            gen_ai_cost_input: 0.01,
            gen_ai_cost_output: 0.02,
            gen_ai_cost_cache_read: 0.001,
            gen_ai_cost_cache_write: 0.002,
            gen_ai_cost_total: 0.033,
            ..Default::default()
        };

        // Child is Span type (real Strands botocore pattern), excluded by Path 2
        // since trace has generation spans
        let child = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "child".to_string(),
            parent_span_id: Some("parent".to_string()),
            span_name: "bedrock".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 1000,
            gen_ai_usage_output_tokens: 500,
            gen_ai_cost_input: 0.01,
            gen_ai_cost_output: 0.02,
            gen_ai_cost_total: 0.03,
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[parent, child]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Parent generation is leaf (no generation child) - counted with complete data
        assert_eq!(trace.input_tokens, 1000, "Input from generation leaf");
        assert_eq!(trace.output_tokens, 500, "Output from generation leaf");
        assert_eq!(
            trace.cache_read_tokens, 500,
            "Cache read from generation leaf"
        );
        assert_eq!(
            trace.cache_write_tokens, 100,
            "Cache write from generation leaf"
        );
        assert_eq!(trace.input_cost, 0.01, "Input cost from generation leaf");
        assert_eq!(trace.output_cost, 0.02, "Output cost from generation leaf");
        assert_eq!(
            trace.cache_read_cost, 0.001,
            "Cache read cost from generation leaf"
        );
        assert_eq!(
            trace.cache_write_cost, 0.002,
            "Cache write cost from generation leaf"
        );
    }

    // ========================================================================
    // Regression: Strands/botocore non-generation spans with tokens
    // ========================================================================

    /// Regression: botocore RPC span (observation_type=Span) carries tokens but
    /// parent agent span has 0 tokens. Old query filtered observation_type='generation'
    /// which excluded botocore spans entirely, yielding 0 tokens at trace level.
    #[tokio::test]
    async fn test_strands_botocore_tokens_from_non_generation_span() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-strands-botocore";

        // Agent span: parent, 0 tokens (StrandsAgents pattern)
        let agent = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "agent-span".to_string(),
            parent_span_id: None,
            span_name: "Agent".to_string(),
            observation_type: Some(ObservationType::Agent),
            timestamp_start: Utc::now(),
            ..Default::default()
        };

        // Botocore RPC span: child, has tokens but observation_type=Span
        let botocore = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "botocore-span".to_string(),
            parent_span_id: Some("agent-span".to_string()),
            span_name: "Bedrock Runtime.Converse".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 940,
            gen_ai_usage_output_tokens: 160,
            gen_ai_usage_total_tokens: 1100,
            gen_ai_cost_input: 0.003,
            gen_ai_cost_output: 0.002,
            gen_ai_cost_total: 0.005,
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[agent, botocore]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        assert_eq!(trace.input_tokens, 940, "Botocore input tokens counted");
        assert_eq!(trace.output_tokens, 160, "Botocore output tokens counted");
        assert_eq!(trace.total_tokens, 1100, "Botocore total tokens counted");
        assert_eq!(trace.total_cost, 0.005, "Botocore cost counted");
    }

    /// Regression: multiple botocore calls under one agent should all be summed.
    #[tokio::test]
    async fn test_strands_multiple_botocore_spans_summed() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-strands-multi";

        let agent = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "agent".to_string(),
            parent_span_id: None,
            span_name: "Agent".to_string(),
            observation_type: Some(ObservationType::Agent),
            timestamp_start: Utc::now(),
            ..Default::default()
        };

        let botocore1 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "botocore-1".to_string(),
            parent_span_id: Some("agent".to_string()),
            span_name: "Bedrock Runtime.Converse".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 500,
            gen_ai_usage_output_tokens: 100,
            gen_ai_cost_total: 0.003,
            ..Default::default()
        };

        let botocore2 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "botocore-2".to_string(),
            parent_span_id: Some("agent".to_string()),
            span_name: "Bedrock Runtime.Converse".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 800,
            gen_ai_usage_output_tokens: 200,
            gen_ai_cost_total: 0.005,
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[agent, botocore1, botocore2]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        assert_eq!(trace.input_tokens, 1300, "Sum of both botocore input");
        assert_eq!(trace.output_tokens, 300, "Sum of both botocore output");
        assert_eq!(trace.total_cost, 0.008, "Sum of both botocore cost");
    }

    /// Regression: LangGraph pattern where botocore(tokens) -> parent generation(tokens)
    /// should NOT double-count. Parent has tokens so child is excluded.
    #[tokio::test]
    async fn test_langgraph_no_double_count_with_token_parent() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-langgraph-dedup";

        let generation = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "generation".to_string(),
            parent_span_id: None,
            span_name: "ChatOpenAI".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 1100,
            gen_ai_usage_output_tokens: 200,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        // Child has same tokens (duplicated) — should be excluded by NOT EXISTS
        let botocore = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "botocore".to_string(),
            parent_span_id: Some("generation".to_string()),
            span_name: "Bedrock Runtime.Converse".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 1100,
            gen_ai_usage_output_tokens: 200,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[generation, botocore]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        assert_eq!(trace.input_tokens, 1100, "Only parent counted");
        assert_eq!(trace.output_tokens, 200, "Only parent counted");
        assert_eq!(trace.total_cost, 0.01, "Only parent cost");
    }

    /// Regression: full Strands hierarchy where Agent has aggregated tokens.
    /// Agent(tokens=sum) → execute_event_loop_cycle(0) → Generation(tokens) → Botocore(tokens)
    /// Only Generation should be counted (Path 1). Agent is excluded (Path 2 fails:
    /// generations exist in trace). Botocore excluded (Path 2 fails: generations exist).
    /// Cycle excluded (0 tokens).
    #[tokio::test]
    async fn test_strands_full_hierarchy_agent_with_aggregated_tokens() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-strands-full";

        // Agent span: root, has aggregated tokens (sum of all generations)
        let agent = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "agent".to_string(),
            parent_span_id: None,
            span_name: "Agent".to_string(),
            observation_type: Some(ObservationType::Agent),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 2000, // sum of gen1 + gen2
            gen_ai_usage_output_tokens: 400,
            gen_ai_usage_total_tokens: 2400,
            gen_ai_cost_total: 0.02,
            ..Default::default()
        };

        // Intermediate cycle span: 0 tokens
        let cycle = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "cycle-1".to_string(),
            parent_span_id: Some("agent".to_string()),
            span_name: "execute_event_loop_cycle".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            ..Default::default()
        };

        // Generation span (chat): has tokens
        let generation = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "gen-1".to_string(),
            parent_span_id: Some("cycle-1".to_string()),
            span_name: "chat".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 1000,
            gen_ai_usage_output_tokens: 200,
            gen_ai_usage_total_tokens: 1200,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        // Botocore child of generation: has tokens but is not a generation type
        let botocore = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "botocore-1".to_string(),
            parent_span_id: Some("gen-1".to_string()),
            span_name: "Bedrock Runtime.Converse".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 1000,
            gen_ai_usage_output_tokens: 200,
            gen_ai_usage_total_tokens: 1200,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[agent, cycle, generation, botocore])
                .expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Only the Generation span should be counted (Path 1):
        // - Agent: Path 2 fails (generations exist in trace)
        // - Cycle: 0 tokens, excluded
        // - Generation: Path 1 succeeds (parent cycle is not a generation)
        // - Botocore: Path 2 fails (generations exist in trace)
        assert_eq!(
            trace.input_tokens, 1000,
            "Only generation's tokens, not agent's aggregated"
        );
        assert_eq!(trace.output_tokens, 200, "Only generation's output");
        assert_eq!(trace.total_cost, 0.01, "Only generation's cost");
    }

    /// Regression: Strands hierarchy with TWO generation cycles.
    /// Agent(sum) → cycle1(0) → Gen1(tokens) → Botocore1(tokens)
    ///            → cycle2(0) → Gen2(tokens) → Botocore2(tokens)
    /// Should count Gen1 + Gen2 only.
    #[tokio::test]
    async fn test_strands_multi_cycle_no_double_count() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-strands-multi-cycle";

        let agent = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "agent".to_string(),
            parent_span_id: None,
            span_name: "Agent".to_string(),
            observation_type: Some(ObservationType::Agent),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 3000, // aggregated sum
            gen_ai_usage_output_tokens: 600,
            gen_ai_cost_total: 0.04,
            ..Default::default()
        };

        // Cycle 1 → Gen1 → Botocore1
        let cycle1 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "cycle-1".to_string(),
            parent_span_id: Some("agent".to_string()),
            span_name: "execute_event_loop_cycle".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            ..Default::default()
        };

        let gen1 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "gen-1".to_string(),
            parent_span_id: Some("cycle-1".to_string()),
            span_name: "chat".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 1000,
            gen_ai_usage_output_tokens: 200,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        let botocore1 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "botocore-1".to_string(),
            parent_span_id: Some("gen-1".to_string()),
            span_name: "Bedrock Runtime.Converse".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 1000,
            gen_ai_usage_output_tokens: 200,
            gen_ai_cost_total: 0.01,
            ..Default::default()
        };

        // Cycle 2 → Gen2 → Botocore2
        let cycle2 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "cycle-2".to_string(),
            parent_span_id: Some("agent".to_string()),
            span_name: "execute_event_loop_cycle".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            ..Default::default()
        };

        let gen2 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "gen-2".to_string(),
            parent_span_id: Some("cycle-2".to_string()),
            span_name: "chat".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 2000,
            gen_ai_usage_output_tokens: 400,
            gen_ai_cost_total: 0.03,
            ..Default::default()
        };

        let botocore2 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "botocore-2".to_string(),
            parent_span_id: Some("gen-2".to_string()),
            span_name: "Bedrock Runtime.Converse".to_string(),
            observation_type: Some(ObservationType::Span),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 2000,
            gen_ai_usage_output_tokens: 400,
            gen_ai_cost_total: 0.03,
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(
                &conn,
                &[agent, cycle1, gen1, botocore1, cycle2, gen2, botocore2],
            )
            .expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Gen1($0.01, 1000+200) + Gen2($0.03, 2000+400) = $0.04, 3000+600
        // NOT Agent($0.04, 3000+600) which would be double-counted
        assert_eq!(trace.input_tokens, 3000, "Gen1(1000) + Gen2(2000)");
        assert_eq!(trace.output_tokens, 600, "Gen1(200) + Gen2(400)");
        assert_eq!(trace.total_cost, 0.04, "Gen1(0.01) + Gen2(0.03)");
    }

    /// Regression: Vercel AI SDK pattern where root generation has 0 tokens
    /// and child doGenerate spans carry the actual token data.
    /// ai.generateText(0 tokens) → ai.generateText.doGenerate(tokens)
    /// Should count the child, not the empty root.
    #[tokio::test]
    async fn test_vercel_root_generation_zero_tokens_child_counted() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-vercel";

        // Root generation orchestrator: 0 tokens (just orchestrates)
        let root = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "root-gen".to_string(),
            parent_span_id: None,
            span_name: "ai.generateText".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            // 0 tokens — orchestrator only
            ..Default::default()
        };

        // First doGenerate: succeeds with tokens
        let do_gen1 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "do-gen-1".to_string(),
            parent_span_id: Some("root-gen".to_string()),
            span_name: "ai.generateText.doGenerate".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            gen_ai_usage_input_tokens: 754,
            gen_ai_usage_output_tokens: 235,
            gen_ai_usage_total_tokens: 989,
            gen_ai_cost_total: 0.0019,
            ..Default::default()
        };

        // Second doGenerate: errors with 0 tokens
        let do_gen2 = NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: trace_id.to_string(),
            span_id: "do-gen-2".to_string(),
            parent_span_id: Some("root-gen".to_string()),
            span_name: "ai.generateText.doGenerate".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            // 0 tokens — errored
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[root, do_gen1, do_gen2]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
        let trace = result.expect("Trace should exist");

        // Root has 0 tokens → excluded from Path 1
        // do_gen1 has tokens, parent (root) is generation with 0 tokens → included
        // do_gen2 has 0 tokens → excluded
        assert_eq!(trace.input_tokens, 754, "Child doGenerate tokens counted");
        assert_eq!(trace.output_tokens, 235, "Child doGenerate tokens counted");
        assert_eq!(trace.total_cost, 0.0019, "Child doGenerate cost counted");
    }

    // ========================================================================
    // Feed API tests
    // ========================================================================

    #[tokio::test]
    async fn test_get_feed_spans_basic() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        // Insert test spans
        let spans = vec![
            make_generation_span(project_id, "trace-1", "span-1", None, 0.01, 100),
            make_generation_span(project_id, "trace-2", "span-2", None, 0.02, 200),
            make_agent_span(project_id, "trace-3", "span-3", None),
        ];

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let params = FeedSpansParams {
            project_id: project_id.to_string(),
            limit: 10,
            ..Default::default()
        };
        let result = get_feed_spans(&conn, &params).expect("Query should succeed");

        // Should return all 3 spans
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn test_get_feed_spans_is_observation_filter() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        // Insert mix of observation and non-observation spans
        let spans = vec![
            make_generation_span(project_id, "trace-1", "gen-1", None, 0.01, 100),
            make_agent_span(project_id, "trace-2", "agent-1", None), // agent has observation_type
            NormalizedSpan {
                project_id: Some(project_id.to_string()),
                trace_id: "trace-3".to_string(),
                span_id: "plain-1".to_string(),
                span_name: "plain-span".to_string(),
                timestamp_start: Utc::now(),
                // No observation_type, no gen_ai_request_model
                ..Default::default()
            },
        ];

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();

        // With is_observation = true
        let params = FeedSpansParams {
            project_id: project_id.to_string(),
            limit: 10,
            is_observation: Some(true),
            ..Default::default()
        };
        let result = get_feed_spans(&conn, &params).expect("Query should succeed");

        // Should return only generation and agent spans (both have observation_type)
        assert_eq!(result.len(), 2, "Should filter to observations only");
    }

    #[tokio::test]
    async fn test_get_feed_spans_limit() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        // Insert 5 spans
        let spans: Vec<_> = (0..5)
            .map(|i| {
                make_generation_span(
                    project_id,
                    &format!("trace-{}", i),
                    &format!("span-{}", i),
                    None,
                    0.01,
                    100,
                )
            })
            .collect();

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let params = FeedSpansParams {
            project_id: project_id.to_string(),
            limit: 3, // Limit to 3
            ..Default::default()
        };
        let result = get_feed_spans(&conn, &params).expect("Query should succeed");

        assert_eq!(result.len(), 3, "Should respect limit");
    }

    #[tokio::test]
    async fn test_get_feed_spans_cursor_pagination() {
        use chrono::Duration;

        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        // Create spans with different ingested_at times
        let base_time = Utc::now();
        let spans: Vec<_> = (0..5)
            .map(|i| {
                let mut span = make_generation_span(
                    project_id,
                    &format!("trace-{}", i),
                    &format!("span-{}", i),
                    None,
                    0.01,
                    100,
                );
                span.timestamp_start = base_time + Duration::seconds(i as i64);
                span.ingested_at = Some(base_time + Duration::seconds(i as i64));
                span
            })
            .collect();

        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("Insert should succeed");
        }

        let conn = analytics.conn();

        // First page (no cursor)
        let params = FeedSpansParams {
            project_id: project_id.to_string(),
            limit: 2,
            ..Default::default()
        };
        let page1 = get_feed_spans(&conn, &params).expect("Query should succeed");
        assert_eq!(page1.len(), 2, "First page should have 2 spans");

        // Second page (with cursor from last span of page1)
        let last_span = page1.last().unwrap();
        let cursor_time_us = last_span.ingested_at.timestamp_micros();
        let params = FeedSpansParams {
            project_id: project_id.to_string(),
            limit: 2,
            cursor: Some((cursor_time_us, last_span.span_id.clone())),
            ..Default::default()
        };
        let page2 = get_feed_spans(&conn, &params).expect("Query should succeed");
        assert_eq!(page2.len(), 2, "Second page should have 2 spans");

        // Verify no overlap between pages
        let page1_ids: Vec<_> = page1.iter().map(|s| &s.span_id).collect();
        let page2_ids: Vec<_> = page2.iter().map(|s| &s.span_id).collect();
        for id in &page2_ids {
            assert!(
                !page1_ids.contains(id),
                "Page 2 should not contain spans from page 1"
            );
        }
    }

    #[tokio::test]
    async fn test_get_feed_spans_empty_project() {
        let (_temp_dir, analytics) = create_test_service().await;

        let conn = analytics.conn();
        let params = FeedSpansParams {
            project_id: "nonexistent-project".to_string(),
            limit: 10,
            ..Default::default()
        };
        let result = get_feed_spans(&conn, &params).expect("Query should succeed");

        assert!(
            result.is_empty(),
            "Should return empty for nonexistent project"
        );
    }
}
