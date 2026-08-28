//! Query repository for OTEL API queries

use chrono::{DateTime, Utc};
use duckdb::{Connection, Row};

use crate::api::routes::otel::filters::{SqlParams, columns};
use crate::core::constants::{QUERY_MAX_FILTER_SUGGESTIONS, QUERY_MAX_SPANS_PER_TRACE};
use crate::data::duckdb::{DuckdbError, in_transaction};
use crate::data::types::{
    DisplayNameDialect, EventRow, FeedSpansParams, LinkRow, ListSessionsParams, ListSpansParams,
    ListTracesParams, SESSION_FILTER_OPTION_COLUMNS, SPAN_FILTER_OPTION_COLUMNS, SessionRow,
    SpanRow, TRACE_FILTER_OPTION_COLUMNS, TraceRow, genai_span_predicate, parse_tags,
    trace_display_first, trace_display_name,
};
use crate::utils::time::{micros_to_datetime, parse_iso_timestamp};

/// Inline dedup subquery replacing the old `otel_spans_v` view.
///
/// Used only where duplicates corrupt results:
/// - SUM/COUNT(*) aggregation (inflated totals)
/// - LIMIT/OFFSET or cursor pagination data queries (page misalignment)
///
/// NOT needed for (these query `otel_spans` directly):
/// - COUNT(DISTINCT) (immune to duplicates)
/// - DISTINCT subqueries (immune)
/// - GROUP BY with MIN/MAX only (immune)
/// - Point lookups by (trace_id, span_id) with DedupAnalyticsRepository
/// - Single-span UNNEST (point-lookup dedup via ORDER BY ingested_at DESC LIMIT 1)
///
/// **The latest delivery wins**, and that is not a free choice: ClickHouse stores spans in
/// `ReplacingMergeTree(ingested_at)`, so its `FINAL` keeps the newest row and no query can ask it
/// for the oldest. Keeping `MIN` here meant the same project reported different tokens depending on
/// which backend served it, whenever an exporter re-sent a span with corrected usage - invisible to
/// every test, because a retry normally carries an identical payload. A later delivery is also the
/// better record: it is the one the exporter meant to leave behind.
pub(crate) const DEDUP_SPANS: &str = "(SELECT s.* FROM otel_spans s \
     INNER JOIN (SELECT project_id, trace_id, span_id, MAX(ingested_at) AS _mi \
                 FROM otel_spans GROUP BY project_id, trace_id, span_id \
     ) _d ON s.project_id = _d.project_id AND s.trace_id = _d.trace_id \
     AND s.span_id = _d.span_id AND s.ingested_at = _d._mi)";

/// Build span conditions with optional table alias.
/// Returns (WHERE clause, bind values).
/// The alias to qualify a column with, or a table name when the query does not alias.
fn alias_or<'a>(fallback: &'a str, alias: &'a str) -> &'a str {
    if alias.is_empty() {
        if fallback.is_empty() {
            "otel_spans"
        } else {
            fallback
        }
    } else {
        alias
    }
}

/// Per-trace token and cost totals, counting each call once.
///
/// A generation span and its generation child report the same usage, so only the leaf is summed;
/// a non-generation span's usage counts only where no generation span in the trace reports any and
/// its parent reports none either. Factored out because a filter on a token or cost column has to
/// be evaluated against *this* number - the one the row displays - and reproducing the rule in a
/// second place is how the two would drift apart.
///
/// "Billed" is tokens **or** cost, and either is enough on its own: instrumentation that reports
/// `llm.cost.*` without usage exists, and requiring tokens billed those calls as free. The
/// `observation_type` test is NULL-safe for the same reason - a plain span carrying usage is what
/// transport-level instrumentation produces, and `NULL != 'generation'` is NULL, so such a span
/// contributed nothing at all.
///
/// The consequence, for a shape that mixes the two: the deepest billed span is taken as *the*
/// record of the call, so if a leaf reports a cost and no usage while its parent reported usage,
/// the trace shows that cost and no tokens. The alternative - summing the leaf's cost with the
/// parent's tokens - merges two records of one call, and there is nothing to say the parent's
/// figure belongs to it. Reaching this needs two instrumentations inside one parent/child pair.
fn gen_totals_sql(where_clause: &str) -> String {
    format!(
        r#"
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
            FROM {DEDUP_SPANS} g
            WHERE {where_clause}
              AND (
                  (g.observation_type = 'generation'
                   AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0 OR g.gen_ai_cost_total > 0)
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.trace_id = g.trace_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND ((c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens + c.gen_ai_usage_total_tokens + c.gen_ai_usage_cache_read_tokens + c.gen_ai_usage_cache_write_tokens + c.gen_ai_usage_reasoning_tokens) > 0 OR c.gen_ai_cost_total > 0)
                   ))
                  OR
                  ((g.observation_type IS NULL OR g.observation_type != 'generation')
                   AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0 OR g.gen_ai_cost_total > 0)
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND ((gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens + gen.gen_ai_usage_total_tokens + gen.gen_ai_usage_cache_read_tokens + gen.gen_ai_usage_cache_write_tokens + gen.gen_ai_usage_reasoning_tokens) > 0 OR gen.gen_ai_cost_total > 0)
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.trace_id = g.trace_id
                         AND p.project_id = g.project_id
                         AND ((p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens + p.gen_ai_usage_total_tokens + p.gen_ai_usage_cache_read_tokens + p.gen_ai_usage_cache_write_tokens + p.gen_ai_usage_reasoning_tokens) > 0 OR p.gen_ai_cost_total > 0)
                   ))
              )
            GROUP BY g.trace_id
        "#
    )
}

/// The expression a trace list row *displays* for a filterable column, when that value is an
/// aggregate over the trace's spans rather than a column of one span.
///
/// `None` means the column is a plain span attribute (a model, an environment, a session id),
/// where "the trace has a span with this value" is the honest reading.
///
/// Aliases are those of [`trace_filter_subquery`]: `n` for the span rows, `gtf` for the totals.
fn trace_aggregate_expression(view_column: &str) -> Option<String> {
    let totals = |col: &str| Some(format!("COALESCE(MAX(gtf.{col}), 0)"));
    match view_column {
        "trace_name" => Some(trace_display_name("n", DisplayNameDialect::DuckDb)),
        // Displayed values too, though they are stored per span: the row shows the earliest span
        // that carries one. See `trace_display_first`.
        "session_id" | "user_id" | "environment" => Some(trace_display_first(
            view_column,
            "n",
            DisplayNameDialect::DuckDb,
        )),
        "start_time" => Some("MIN(n.timestamp_start)".to_string()),
        "end_time" => Some("MAX(COALESCE(n.timestamp_end, n.timestamp_start))".to_string()),
        "duration_ms" => Some(
            "DATE_DIFF('millisecond', MIN(n.timestamp_start), \
             MAX(COALESCE(n.timestamp_end, n.timestamp_start)))"
                .to_string(),
        ),
        "input_tokens" | "output_tokens" | "total_tokens" | "cache_read_tokens"
        | "cache_write_tokens" | "reasoning_tokens" | "input_cost" | "output_cost"
        | "cache_read_cost" | "cache_write_cost" | "reasoning_cost" | "total_cost" => {
            totals(view_column)
        }
        _ => None,
    }
}

/// True when the column's displayed value comes from [`gen_totals_sql`], so the subquery evaluating
/// it has to join those totals in.
fn needs_gen_totals(view_column: &str) -> bool {
    trace_aggregate_expression(view_column).is_some_and(|e| e.contains("gtf."))
}

fn build_trace_span_conditions(params: &ListTracesParams, alias: &str) -> (String, Vec<String>) {
    let mut conditions = Vec::new();
    let mut bind_values = Vec::new();

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

    // A trace-level subquery, like the session above and for the same reason: a user id sits on the
    // spans that carry one, so a row predicate selected the same traces but summed only those spans,
    // reporting a trace's tokens as whatever the matching spans held rather than the trace's own.
    if let Some(ref uid) = params.user_id {
        conditions.push(format!(
            "{} IN (SELECT DISTINCT trace_id FROM otel_spans WHERE project_id = ? AND user_id = ?)",
            col("trace_id")
        ));
        bind_values.push(params.project_id.clone());
        bind_values.push(uid.clone());
    }

    if let Some(ref envs) = params.environment
        && !envs.is_empty()
    {
        let placeholders: Vec<&str> = envs.iter().map(|_| "?").collect();
        conditions.push(format!(
            "{} IN (SELECT DISTINCT trace_id FROM otel_spans WHERE project_id = ? \
             AND environment IN ({}))",
            col("trace_id"),
            placeholders.join(", ")
        ));
        bind_values.push(params.project_id.clone());
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

    // Advanced filters select *traces*, never individual spans.
    //
    // Applied to span rows, a trace filter means something the list cannot show. Two ways:
    //
    // - a trace's tokens, cost and duration are aggregates over its spans, so comparing one span
    //   against the value hid a trace displaying 3000 tokens from `> 2500` because no single span
    //   reached it, and returned that same trace for `< 1500` because one span was under. It also
    //   fed the filter into the totals CTE, so the number displayed was a sum over the spans that
    //   had passed the filter rather than the trace's own.
    // - ANDed on one row, two filters asked for a span carrying both values. A session id lives on
    //   the root span and tokens on its generation child - the ordinary shape for anything that
    //   opens a root agent span - so session + tokens matched nothing while either alone matched.
    //
    // Both go away once each filter is a per-trace predicate: a condition on the aggregate for a
    // displayed value, "the trace has a span where..." for a span attribute. Keeping them as
    // conditions on this WHERE (rather than a HAVING somewhere later) is what lets the count query,
    // the totals CTE and the page query share one definition of which traces match - and a
    // trace-level condition keeps or drops all of a trace's spans, so it cannot distort an
    // aggregate the way a row-level one did.
    let mut aggregate_conditions: Vec<String> = Vec::new();
    let mut aggregate_binds: Vec<String> = Vec::new();
    let mut join_totals = false;

    for filter in &params.filters {
        match trace_aggregate_expression(filter.column()) {
            Some(expression) => {
                let mut inner = SqlParams::default();
                aggregate_conditions.push(filter.to_sql_against(&mut inner, &expression));
                aggregate_binds.extend(inner.values);
                join_totals |= needs_gen_totals(filter.column());
            }
            None => {
                // A column no row displays on its own - a model, a provider, a framework, a tag.
                // "Some span of this trace" is the honest reading of the positive form; the
                // negative one is "no span", which is the complement of the positive rather than
                // the negated predicate. Asked as written, "none of claude-haiku" returned a trace
                // that used claude-haiku in one call and something else in the next.
                let twin = filter.positive_twin();
                let rendered = twin.as_ref().unwrap_or(filter);
                let quantifier = if twin.is_some() { "NOT IN" } else { "IN" };
                let mut inner = SqlParams::default();
                let condition =
                    rendered.to_sql_aliased(&mut inner, columns::map_trace_column_to_spans, "n");
                conditions.push(format!(
                    "{}.trace_id {quantifier} (SELECT n.trace_id FROM otel_spans n \
                     WHERE n.project_id = ? AND {condition})",
                    alias_or("otel_spans", alias)
                ));
                bind_values.push(params.project_id.clone());
                bind_values.extend(inner.values);
            }
        }
    }

    // One subquery for every aggregate filter, so a trace is grouped once however many were asked
    // for. The totals join is added only when a token or cost column is involved: the other
    // aggregates come from the span rows directly and do not need it.
    if !aggregate_conditions.is_empty() {
        let mut totals_join = String::new();
        if join_totals {
            // Scoped over the same rows the projection sums, time window included. Every other
            // condition on this query is trace-level, so it keeps or drops a trace whole and cannot
            // change a total; the window is the one that still selects spans, and a filter comparing
            // against an all-time total would have selected traces by a number the row does not
            // show - the defect this whole path exists to remove.
            let mut scope = "g.project_id = ?".to_string();
            let mut scope_binds = vec![params.project_id.clone()];
            if let Some(ref from) = params.from_timestamp {
                scope.push_str(" AND g.timestamp_start >= ?");
                scope_binds.push(from.to_rfc3339());
            }
            if let Some(ref to) = params.to_timestamp {
                scope.push_str(" AND g.timestamp_start <= ?");
                scope_binds.push(to.to_rfc3339());
            }
            totals_join = format!(
                "LEFT JOIN ({}) gtf ON gtf.trace_id = n.trace_id",
                gen_totals_sql(&scope)
            );
            // Bound before the outer project_id: the join is rendered ahead of the WHERE.
            bind_values.extend(scope_binds);
        }
        conditions.push(format!(
            "{}.trace_id IN (SELECT n.trace_id FROM otel_spans n {totals_join} \
             WHERE n.project_id = ? GROUP BY n.project_id, n.trace_id HAVING {})",
            alias_or("otel_spans", alias),
            aggregate_conditions.join(" AND ")
        ));
        bind_values.push(params.project_id.clone());
        bind_values.extend(aggregate_binds);
    }

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

    // Count query — raw otel_spans avoids the DEDUP_SPANS self-join. Safe because duplicates share
    // identical data for all filterable columns, and the HAVING checks are immune to row
    // duplication.
    //
    // Both HAVING conditions belong here as well as in the page query: a count that ignored them
    // reported more traces than the pages contain.
    let mut count_having: Vec<String> = Vec::new();
    if !params.include_nongenai {
        count_having.push(format!(
            "COUNT(*) FILTER (WHERE {}) > 0",
            genai_span_predicate("")
        ));
    }
    let count_sql = if count_having.is_empty() {
        format!(
            "SELECT COUNT(DISTINCT trace_id) FROM otel_spans WHERE {}",
            span_where
        )
    } else {
        format!(
            // project_id is in the GROUP BY so the name subquery in the HAVING can correlate on
            // it. The WHERE already fixes it to one value, so the grouping is unchanged.
            r#"SELECT COUNT(*) FROM (
                SELECT trace_id FROM otel_spans WHERE {}
                GROUP BY project_id, trace_id
                HAVING {}
            ) t"#,
            span_where,
            count_having.join(" AND ")
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
    // Every column TRACE_SORTABLE accepts is mapped. One that is accepted and not mapped falls
    // through to min_ts, so the list comes back ordered by time while the UI shows the column the
    // user picked as the active sort.
    let span_sort_field = match sort_field {
        "start_time" => "min_ts",
        "end_time" => "max_ts",
        "duration_ms" => "duration_ms",
        "total_cost" => "total_cost",
        "total_tokens" => "total_tokens",
        "observation_count" => "observation_count",
        _ => "min_ts",
    };

    let offset = (params.page.saturating_sub(1)) * params.limit;

    // Build aliased WHERE clauses for CTEs
    // gen_totals CTE uses alias "g", filtered_traces CTE uses alias "sp"
    let (span_where_g, bind_values_g) = build_trace_span_conditions(params, "g");
    let (span_where_sp, bind_values_sp) = build_trace_span_conditions(params, "sp");

    // HAVING clause to filter traces with GenAI spans when not including non-GenAI traces
    let mut having_parts: Vec<String> = Vec::new();
    if !params.include_nongenai {
        // The second term keeps the documented "without the SDK" path visible: a transport-tagged
        // Bedrock span is classified as a plain span on purpose, so such a trace has no
        // observations even though its GenAI data is present and aggregated.
        having_parts.push("observation_count > 0 OR genai_span_count > 0".to_string());
    }
    let having_clause = if having_parts.is_empty() {
        String::new()
    } else {
        format!("HAVING {}", having_parts.join(" AND "))
    };

    let data_sql = format!(
        r#"
        WITH gen_totals AS ({gen_totals}),
        filtered_traces AS (
            SELECT
                sp.project_id,
                sp.trace_id,
                MIN(sp.timestamp_start) as min_ts,
                MAX(COALESCE(sp.timestamp_end, sp.timestamp_start)) as max_ts,
                DATE_DIFF('millisecond', MIN(sp.timestamp_start), MAX(COALESCE(sp.timestamp_end, sp.timestamp_start))) as duration_ms,
                COALESCE(MAX(gt.total_cost), 0)::DOUBLE as total_cost,
                -- Sortable, so it has to be computed here: `?order_by=total_tokens` is accepted by
                -- the API and fell through to min_ts, silently sorting by time instead.
                COALESCE(MAX(gt.total_tokens), 0) as total_tokens,
                COUNT(*) FILTER (WHERE sp.observation_type != 'span') as observation_count,
                COUNT(*) FILTER (WHERE {genai_sp}) as genai_span_count
            FROM {DEDUP_SPANS} sp
            LEFT JOIN gen_totals gt ON sp.trace_id = gt.trace_id
            WHERE {span_where_sp}
            GROUP BY sp.project_id, sp.trace_id
            {having_clause}
            -- The same total key the outer query orders by: requested field, min_ts, trace_id.
            -- Two traces with the same sort value have no defined order between them otherwise, so
            -- a row can appear on two pages or on none; and if the inner key differed from the
            -- outer one, page membership and display order would disagree at every boundary.
            ORDER BY {span_sort_field} {sort_dir}, min_ts {sort_dir}, sp.trace_id ASC
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
        JOIN {DEDUP_SPANS} s ON t.project_id = s.project_id AND t.trace_id = s.trace_id
        LEFT JOIN gen_totals gt2 ON t.trace_id = gt2.trace_id
        GROUP BY t.trace_id, t.min_ts, t.{span_sort_field}
        -- Ordered by the column that was asked for. The inner CTE already picked the page by it,
        -- but the outer query re-sorted by min_ts, so ?order_by=total_cost returned the most
        -- expensive traces arranged by time - the requested order was silently discarded, and only
        -- its direction survived. min_ts is kept as the tiebreak so a page is deterministic.
        ORDER BY t.{span_sort_field} {sort_dir}, t.min_ts {sort_dir}, t.trace_id ASC
        "#,
        gen_totals = gen_totals_sql(&span_where_g),
        span_where_sp = span_where_sp,
        // The same definition the count query uses, qualified for this CTE's alias.
        genai_sp = genai_span_predicate("sp"),
        span_sort_field = span_sort_field,
        sort_dir = sort_dir,
        limit = params.limit,
        offset = offset,
        DEDUP_SPANS = DEDUP_SPANS
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
    let sql = format!(
        r#"
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
            FROM {DEDUP_SPANS} g
            WHERE g.project_id = ? AND g.trace_id = ?
              AND (
                  (g.observation_type = 'generation'
                   AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0 OR g.gen_ai_cost_total > 0)
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.trace_id = g.trace_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND ((c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens + c.gen_ai_usage_total_tokens + c.gen_ai_usage_cache_read_tokens + c.gen_ai_usage_cache_write_tokens + c.gen_ai_usage_reasoning_tokens) > 0 OR c.gen_ai_cost_total > 0)
                   ))
                  OR
                  ((g.observation_type IS NULL OR g.observation_type != 'generation')
                   AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0 OR g.gen_ai_cost_total > 0)
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND ((gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens + gen.gen_ai_usage_total_tokens + gen.gen_ai_usage_cache_read_tokens + gen.gen_ai_usage_cache_write_tokens + gen.gen_ai_usage_reasoning_tokens) > 0 OR gen.gen_ai_cost_total > 0)
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.trace_id = g.trace_id
                         AND p.project_id = g.project_id
                         AND ((p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens + p.gen_ai_usage_total_tokens + p.gen_ai_usage_cache_read_tokens + p.gen_ai_usage_cache_write_tokens + p.gen_ai_usage_reasoning_tokens) > 0 OR p.gen_ai_cost_total > 0)
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
        FROM {DEDUP_SPANS} s
        CROSS JOIN gen_totals gt
        WHERE s.project_id = ? AND s.trace_id = ?
        GROUP BY s.trace_id, gt.input_tokens, gt.output_tokens, gt.total_tokens,
                 gt.cache_read_tokens, gt.cache_write_tokens, gt.reasoning_tokens,
                 gt.input_cost, gt.output_cost, gt.cache_read_cost, gt.cache_write_cost,
                 gt.reasoning_cost, gt.total_cost
    "#,
        DEDUP_SPANS = DEDUP_SPANS
    );

    let mut stmt = conn.prepare(&sql)?;
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

    // "GenAI only", the same definition the trace and session lists use. Testing observation_type
    // alone hid every span that carries GenAI data without one - transport-level instrumentation
    // records gen_ai.* on a plain span, and some instrumentation reports only token usage. The UI
    // has this filter on by default, so those spans were missing from the list while their traces
    // were present in the trace list, which does use the full predicate.
    if params.is_observation == Some(true) {
        conditions.push(format!("({})", genai_span_predicate("")));
    }

    // Apply advanced filters - map API column names to DB columns
    for filter in &params.filters {
        conditions.push(filter.to_sql_aliased(&mut sql_params, columns::map_span_column, ""));
    }
    bind_values.extend(sql_params.values);

    let where_clause = conditions.join(" AND ");

    // Count query — COUNT(DISTINCT) on raw table avoids the DEDUP_SPANS self-join
    let count_sql = format!(
        "SELECT COUNT(*) FROM (SELECT DISTINCT trace_id, span_id FROM otel_spans WHERE {}) _c",
        where_clause
    );
    let total = execute_count(conn, &count_sql, &bind_values)?;

    // Data query - map API column names (start_time/end_time) to DB columns (timestamp_start/timestamp_end)
    // (trace_id, span_id) breaks ties so LIMIT/OFFSET paginate a total order: spans sharing a
    // timestamp have no defined order between them otherwise, and a row can then appear on two
    // pages or on none.
    let order = params
        .order_by
        .as_ref()
        .map(|o| o.to_sql_mapped(columns::map_span_column))
        .unwrap_or_else(|| "timestamp_start DESC".to_string())
        + ", trace_id ASC, span_id ASC";

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
         FROM {DEDUP_SPANS} WHERE {} ORDER BY {} LIMIT {} OFFSET {}",
        where_clause, order, params.limit, offset,
        DEDUP_SPANS = DEDUP_SPANS
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

    // Cursor condition, on the same total key the ORDER BY uses; the trace id is part of it
    // because a span id is unique only within a trace.
    if let Some((cursor_time_us, cursor_span_id, cursor_trace_id)) = &params.cursor {
        conditions
            .push("(EPOCH_US(ingested_at), span_id, trace_id) < (?::BIGINT, ?, ?)".to_string());
        bind_values.push(cursor_time_us.to_string());
        bind_values.push(cursor_span_id.clone());
        bind_values.push(cursor_trace_id.clone());
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

    // "GenAI only", the same definition the trace and session lists use. Testing observation_type
    // alone hid every span that carries GenAI data without one - transport-level instrumentation
    // records gen_ai.* on a plain span, and some instrumentation reports only token usage. The UI
    // has this filter on by default, so those spans were missing from the list while their traces
    // were present in the trace list, which does use the full predicate.
    if params.is_observation == Some(true) {
        conditions.push(format!("({})", genai_span_predicate("")));
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
         FROM {DEDUP_SPANS} WHERE {} ORDER BY ingested_at DESC, span_id DESC, trace_id DESC LIMIT {}",
        where_clause, params.limit,
        DEDUP_SPANS = DEDUP_SPANS
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
               FROM otel_spans WHERE project_id = ? AND trace_id = ? ORDER BY timestamp_start LIMIT {}",
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
               FROM otel_spans WHERE project_id = ? AND trace_id = ? AND span_id = ?";

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
    // Point-lookup dedup: fetch the single canonical row, then UNNEST its events.
    // Much faster than DEDUP_SPANS which GROUP BY + JOINs all project spans.
    let sql = format!(
        "SELECT _s.span_id, (ordinality - 1)::INTEGER as event_index,
                      event->>'timestamp' as event_timestamp,
                      event->>'name' as event_name,
                      (event->'attributes')::VARCHAR as attributes
               FROM (SELECT span_id, raw_span FROM otel_spans
                     WHERE project_id = ? AND trace_id = ? AND span_id = ?
                     ORDER BY ingested_at DESC LIMIT 1) _s,
                    UNNEST(CAST(_s.raw_span->'events' AS JSON[])) WITH ORDINALITY AS t(event, ordinality)
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
    // Point-lookup dedup: fetch the single canonical row, then UNNEST its links.
    let sql = format!(
        "SELECT _s.span_id, link->>'trace_id' as linked_trace_id, link->>'span_id' as linked_span_id,
                (link->'attributes')::VARCHAR as attributes
               FROM (SELECT span_id, raw_span FROM otel_spans
                     WHERE project_id = ? AND trace_id = ? AND span_id = ?
                     ORDER BY ingested_at DESC LIMIT 1) _s,
                    UNNEST(CAST(_s.raw_span->'links' AS JSON[])) WITH ORDINALITY AS t(link, ordinality)
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

    // Count query — raw otel_spans avoids the DEDUP_SPANS self-join
    let count_sql = format!(
        "SELECT COUNT(DISTINCT session_id) FROM otel_spans WHERE {}",
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
    // Every column SESSION_SORTABLE accepts is mapped; see the trace list for why that matters.
    let span_sort_field = match sort_field {
        "start_time" => "min_ts",
        "end_time" => "max_ts",
        "total_cost" => "total_cost",
        "trace_count" => "trace_count",
        "span_count" => "span_count",
        "observation_count" => "observation_count",
        _ => "min_ts",
    };

    let offset = (params.page.saturating_sub(1)) * params.limit;

    // Build aliased WHERE clauses for CTEs
    // Only the session_traces CTE carries the request's conditions now; the aggregates read the
    // traces it selected, so there is one bind set instead of two.
    let (span_where_sp, bind_values_sp) = build_session_span_conditions(params, "sp");

    let data_sql = format!(
        r#"
        WITH matching_sessions AS (
            -- Which sessions the request selects. Rows that name a session, exactly as the count
            -- query counts them, so the two agree.
            SELECT DISTINCT sp.project_id, sp.session_id
            FROM {DEDUP_SPANS} sp
            WHERE {span_where_sp}
        ),
        session_traces AS (
            -- Which traces belong to those sessions - every trace of each, not only the ones whose
            -- naming rows passed the filter.
            --
            -- Selection and membership are separate questions, and answering both with one
            -- predicate returned a partial session: a session of two traces in different
            -- environments, filtered to one, was listed with that trace's times, counts and cost
            -- while opening it showed both. Aggregating the naming rows alone had the same shape,
            -- and under-reported every session whose id is recorded on the root span only, which is
            -- how several frameworks record it.
            SELECT DISTINCT sp.project_id, sp.session_id, sp.trace_id
            FROM {DEDUP_SPANS} sp
            JOIN matching_sessions ms
              ON ms.project_id = sp.project_id AND ms.session_id = sp.session_id
        ),
        gen_totals AS (
            SELECT
                st.session_id,
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
            FROM {DEDUP_SPANS} g
            JOIN session_traces st
              ON st.project_id = g.project_id AND st.trace_id = g.trace_id
            WHERE 1 = 1
              AND (
                  (g.observation_type = 'generation'
                   AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0 OR g.gen_ai_cost_total > 0)
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.trace_id = g.trace_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND ((c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens + c.gen_ai_usage_total_tokens + c.gen_ai_usage_cache_read_tokens + c.gen_ai_usage_cache_write_tokens + c.gen_ai_usage_reasoning_tokens) > 0 OR c.gen_ai_cost_total > 0)
                   ))
                  OR
                  ((g.observation_type IS NULL OR g.observation_type != 'generation')
                   AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0 OR g.gen_ai_cost_total > 0)
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND ((gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens + gen.gen_ai_usage_total_tokens + gen.gen_ai_usage_cache_read_tokens + gen.gen_ai_usage_cache_write_tokens + gen.gen_ai_usage_reasoning_tokens) > 0 OR gen.gen_ai_cost_total > 0)
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.trace_id = g.trace_id
                         AND p.project_id = g.project_id
                         AND ((p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens + p.gen_ai_usage_total_tokens + p.gen_ai_usage_cache_read_tokens + p.gen_ai_usage_cache_write_tokens + p.gen_ai_usage_reasoning_tokens) > 0 OR p.gen_ai_cost_total > 0)
                   ))
              )
            GROUP BY st.session_id
        ),
        filtered_sessions AS (
            SELECT
                st.project_id,
                st.session_id,
                MIN(sp.timestamp_start) as min_ts,
                -- max_ts and observation_count are sortable, so they are computed here rather than
                -- letting `?order_by=end_time` fall through to min_ts.
                MAX(COALESCE(sp.timestamp_end, sp.timestamp_start)) as max_ts,
                COALESCE(MAX(gt.total_cost), 0)::DOUBLE as total_cost,
                COUNT(DISTINCT sp.trace_id) as trace_count,
                COUNT(*) as span_count,
                COUNT(*) FILTER (WHERE sp.observation_type != 'span') as observation_count
            FROM session_traces st
            JOIN {DEDUP_SPANS} sp
              ON sp.project_id = st.project_id AND sp.trace_id = st.trace_id
            LEFT JOIN gen_totals gt ON gt.session_id = st.session_id
            GROUP BY st.project_id, st.session_id
            -- The same total key the outer query orders by: requested field, min_ts, session_id.
            ORDER BY {span_sort_field} {sort_dir}, min_ts {sort_dir}, st.session_id ASC
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
        JOIN session_traces st ON st.project_id = f.project_id AND st.session_id = f.session_id
        JOIN {DEDUP_SPANS} s ON s.project_id = st.project_id AND s.trace_id = st.trace_id
        LEFT JOIN gen_totals gt2 ON gt2.session_id = f.session_id
        GROUP BY f.session_id, f.min_ts, f.{span_sort_field}
        -- See the trace list: the requested column, not just its direction.
        ORDER BY f.{span_sort_field} {sort_dir}, f.min_ts {sort_dir}, f.session_id ASC
        "#,
        span_where_sp = span_where_sp,
        span_sort_field = span_sort_field,
        sort_dir = sort_dir,
        limit = params.limit,
        offset = offset,
        DEDUP_SPANS = DEDUP_SPANS
    );

    // Combine bind values: gen_totals CTE first, then filtered_sessions CTE
    let rows = execute_session_query(conn, &data_sql, &bind_values_sp)?;

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
    let sql = format!(
        r#"
        WITH session_traces AS (
            SELECT DISTINCT trace_id FROM otel_spans
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
            FROM {DEDUP_SPANS} g
            WHERE g.project_id = ?
              AND g.trace_id IN (SELECT trace_id FROM session_traces)
              AND (
                  (g.observation_type = 'generation'
                   AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0 OR g.gen_ai_cost_total > 0)
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.trace_id = g.trace_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND ((c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens + c.gen_ai_usage_total_tokens + c.gen_ai_usage_cache_read_tokens + c.gen_ai_usage_cache_write_tokens + c.gen_ai_usage_reasoning_tokens) > 0 OR c.gen_ai_cost_total > 0)
                   ))
                  OR
                  ((g.observation_type IS NULL OR g.observation_type != 'generation')
                   AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0 OR g.gen_ai_cost_total > 0)
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND ((gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens + gen.gen_ai_usage_total_tokens + gen.gen_ai_usage_cache_read_tokens + gen.gen_ai_usage_cache_write_tokens + gen.gen_ai_usage_reasoning_tokens) > 0 OR gen.gen_ai_cost_total > 0)
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.trace_id = g.trace_id
                         AND p.project_id = g.project_id
                         AND ((p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens + p.gen_ai_usage_total_tokens + p.gen_ai_usage_cache_read_tokens + p.gen_ai_usage_cache_write_tokens + p.gen_ai_usage_reasoning_tokens) > 0 OR p.gen_ai_cost_total > 0)
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
        FROM {DEDUP_SPANS} s
        CROSS JOIN gen_totals gt
        WHERE s.project_id = ?
          AND s.trace_id IN (SELECT trace_id FROM session_traces)
        GROUP BY gt.input_tokens, gt.output_tokens, gt.total_tokens,
                 gt.cache_read_tokens, gt.cache_write_tokens, gt.reasoning_tokens,
                 gt.input_cost, gt.output_cost, gt.cache_read_cost, gt.cache_write_cost,
                 gt.reasoning_cost, gt.total_cost
    "#,
        DEDUP_SPANS = DEDUP_SPANS
    );

    let mut stmt = conn.prepare(&sql)?;
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
    let sql = format!(
        r#"
        WITH session_traces AS (
            SELECT DISTINCT trace_id FROM otel_spans
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
            FROM {DEDUP_SPANS} g
            WHERE g.project_id = ?
              AND g.trace_id IN (SELECT trace_id FROM session_traces)
              AND (
                  (g.observation_type = 'generation'
                   AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0 OR g.gen_ai_cost_total > 0)
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.trace_id = g.trace_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND ((c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens + c.gen_ai_usage_total_tokens + c.gen_ai_usage_cache_read_tokens + c.gen_ai_usage_cache_write_tokens + c.gen_ai_usage_reasoning_tokens) > 0 OR c.gen_ai_cost_total > 0)
                   ))
                  OR
                  ((g.observation_type IS NULL OR g.observation_type != 'generation')
                   AND ((g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens + g.gen_ai_usage_total_tokens + g.gen_ai_usage_cache_read_tokens + g.gen_ai_usage_cache_write_tokens + g.gen_ai_usage_reasoning_tokens) > 0 OR g.gen_ai_cost_total > 0)
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND ((gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens + gen.gen_ai_usage_total_tokens + gen.gen_ai_usage_cache_read_tokens + gen.gen_ai_usage_cache_write_tokens + gen.gen_ai_usage_reasoning_tokens) > 0 OR gen.gen_ai_cost_total > 0)
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.trace_id = g.trace_id
                         AND p.project_id = g.project_id
                         AND ((p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens + p.gen_ai_usage_total_tokens + p.gen_ai_usage_cache_read_tokens + p.gen_ai_usage_cache_write_tokens + p.gen_ai_usage_reasoning_tokens) > 0 OR p.gen_ai_cost_total > 0)
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
        FROM {DEDUP_SPANS} s
        LEFT JOIN gen_totals gt ON s.trace_id = gt.trace_id
        WHERE s.project_id = ?
          AND s.trace_id IN (SELECT trace_id FROM session_traces)
        GROUP BY s.trace_id, gt.input_tokens, gt.output_tokens, gt.total_tokens,
                 gt.cache_read_tokens, gt.cache_write_tokens, gt.reasoning_tokens,
                 gt.input_cost, gt.output_cost, gt.cache_read_cost, gt.cache_write_cost,
                 gt.reasoning_cost, gt.total_cost
        ORDER BY MIN(s.timestamp_start) DESC
    "#,
        DEDUP_SPANS = DEDUP_SPANS
    );

    // Bind order: session_traces(project_id, session_id), gen_totals(project_id), main(project_id)
    execute_trace_query(
        conn,
        &sql,
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
         FROM otel_spans
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

    // Count unique spans per project without the DEDUP_SPANS self-join
    let placeholders: Vec<&str> = project_ids.iter().map(|_| "?").collect();
    let sql = format!(
        "SELECT project_id, COUNT(*) FROM (SELECT DISTINCT project_id, trace_id, span_id FROM otel_spans WHERE project_id IN ({})) GROUP BY project_id",
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

/// Get distinct values with counts for trace filter options.
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

        // COUNT(DISTINCT trace_id) is immune to row duplication. Exact, not
        // APPROX_COUNT_DISTINCT: these counts sit next to each option in the filter UI, and the
        // ClickHouse backend counts them exactly - the two disagreeing by HyperLogLog's error
        // means one project reports different numbers depending on which backend serves it.
        //
        // trace_name is computed per trace with the same root-span-then-earliest-named fallback the
        // trace list displays. Listing root span names only offered names that did not match the
        // list and omitted the ones a trace with no root span shows - so filtering by a name the UI
        // had just displayed returned nothing.
        let sql = if column == "trace_name" {
            format!(
                r#"
                SELECT name, COUNT(DISTINCT trace_id) as cnt
                FROM (
                    SELECT trace_id,
                        COALESCE(
                            FIRST(span_name ORDER BY timestamp_start)
                                FILTER (WHERE parent_span_id IS NULL AND span_name IS NOT NULL),
                            FIRST(span_name ORDER BY timestamp_start)
                                FILTER (WHERE span_name IS NOT NULL)
                        ) AS name
                    FROM otel_spans
                    WHERE {base_where}
                    GROUP BY trace_id
                )
                WHERE name IS NOT NULL
                GROUP BY name
                ORDER BY cnt DESC
                LIMIT {limit}
                "#,
                base_where = base_where,
                limit = QUERY_MAX_FILTER_SUGGESTIONS
            )
        } else {
            format!(
                r#"
                SELECT {col}, COUNT(DISTINCT trace_id) as cnt
                FROM otel_spans
                WHERE {base_where} AND {col} IS NOT NULL
                GROUP BY {col}
                ORDER BY cnt DESC
                LIMIT {limit}
                "#,
                base_where = base_where,
                col = span_column,
                limit = QUERY_MAX_FILTER_SUGGESTIONS
            )
        };

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

/// Get distinct tag values with counts from trace tags array.
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

    // COUNT(DISTINCT trace_id) is immune to row duplication, and exact so both analytics
    // backends report the same number (see get_trace_filter_options).
    // tags is stored as JSON array string (VARCHAR), so parse with from_json first
    let sql = format!(
        r#"
        SELECT tag, COUNT(DISTINCT trace_id) as cnt
        FROM (
            SELECT trace_id, UNNEST(from_json(tags, '["VARCHAR"]')) as tag
            FROM otel_spans
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

    // "GenAI only", as in the span list above.
    if observations_only {
        base_conditions.push(format!("({})", genai_span_predicate("")));
    }

    let base_where = base_conditions.join(" AND ");

    for column in columns {
        // Validate column is allowed
        if !SPAN_FILTER_OPTION_COLUMNS.contains(&column.as_str()) {
            continue;
        }

        // DISTINCT (trace_id, span_id) is immune to row duplication
        let sql = format!(
            r#"
            SELECT {col}, COUNT(*) as cnt
            FROM (SELECT DISTINCT trace_id, span_id, {col} FROM otel_spans
                  WHERE {base_where} AND {col} IS NOT NULL) _d
            GROUP BY {col}
            ORDER BY cnt DESC
            LIMIT {limit}
            "#,
            base_where = base_where,
            col = column,
            limit = QUERY_MAX_FILTER_SUGGESTIONS,
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

/// Get distinct values with counts for session filter options
/// Used by the Sessions page to populate filter dropdowns
/// Each count is the number of sessions carrying that value
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

        // COUNT(DISTINCT session_id) is immune to row duplication, and exact so both analytics
        // backends report the same number (see get_trace_filter_options).
        let sql = format!(
            r#"
            SELECT {col}, COUNT(DISTINCT session_id) as cnt
            FROM otel_spans
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

    /// A trace whose only span is transport-tagged (the documented "without the SDK"
    /// path: BotocoreInstrumentor sets rpc.system, so the span is deliberately classified
    /// as a plain span) must still appear in the default GenAI-only listing. It used to be
    /// filtered out entirely, so those users saw nothing even though tokens and costs were
    /// aggregated correctly.
    ///
    /// This exercises the `include_nongenai: false` branch, which no other test covered -
    /// the reason the original break slipped past the suite.
    #[tokio::test]
    async fn test_list_traces_includes_genai_trace_without_observation_span() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        let mut transport_only =
            make_generation_span(project_id, "trace-nosdk", "sp-1", None, 0.02, 40);
        // What the transport instrumentation produces: plain span, but real GenAI data.
        transport_only.observation_type = Some(ObservationType::Span);
        transport_only.gen_ai_system = Some("aws.bedrock".to_string());
        transport_only.gen_ai_request_model = Some("global.anthropic.claude-haiku-4-5".to_string());

        // A trace with neither observations nor GenAI attributes must stay hidden.
        let mut plain = make_generation_span(project_id, "trace-plain", "sp-2", None, 0.0, 0);
        plain.observation_type = Some(ObservationType::Span);
        plain.gen_ai_usage_total_tokens = 0;
        plain.gen_ai_usage_input_tokens = 0;
        plain.gen_ai_usage_output_tokens = 0;
        plain.gen_ai_cost_total = 0.0;

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[transport_only, plain]).expect("Insert should succeed");
        }

        let params = ListTracesParams {
            project_id: project_id.to_string(),
            page: 0,
            limit: 100,
            include_nongenai: false,
            ..Default::default()
        };
        let conn = analytics.conn();
        let (traces, _total) = list_traces(&conn, &params).expect("Query should succeed");

        let ids: Vec<&str> = traces.iter().map(|t| t.trace_id.as_str()).collect();
        assert!(
            ids.contains(&"trace-nosdk"),
            "a GenAI trace without an observation span must be listed, got {ids:?}"
        );
        assert!(
            !ids.contains(&"trace-plain"),
            "a trace with no GenAI evidence must stay hidden, got {ids:?}"
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
            cursor: Some((
                cursor_time_us,
                last_span.span_id.clone(),
                last_span.trace_id.clone(),
            )),
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

    // ========================================================================
    // A trace filter means what the trace list displays
    // ========================================================================

    use crate::data::duckdb::filters::{Filter, NullOp, NumberOp, OptionsOp, StringOp};

    fn trace_filter_params(project_id: &str, filters: Vec<Filter>) -> ListTracesParams {
        ListTracesParams {
            project_id: project_id.to_string(),
            page: 1,
            limit: 50,
            filters,
            ..Default::default()
        }
    }

    /// A token filter compares against the total the row shows, not against one span of it.
    ///
    /// The list displays a trace's tokens as a sum over its spans, and the filter was applied to
    /// each span row separately, so "more than 2500 tokens" hid a trace displaying 3000 because no
    /// single span reached 2500. The same mismatch let "fewer than 1500" return that trace, since
    /// one span qualifies. Both directions are wrong against what the user can see.
    #[tokio::test]
    async fn a_token_filter_matches_the_total_the_trace_displays() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-aggregate-tokens";

        let spans = vec![
            make_agent_span(project_id, trace_id, "agent-1", None),
            make_generation_span(project_id, trace_id, "gen-1", Some("agent-1"), 0.01, 1000),
            make_generation_span(project_id, trace_id, "gen-2", Some("agent-1"), 0.02, 2000),
        ];
        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("insert");
        }

        let conn = analytics.conn();
        let displayed = get_trace(&conn, project_id, trace_id)
            .expect("query")
            .expect("trace exists");
        assert_eq!(displayed.total_tokens, 3000, "premise of the test");

        let (rows, total) = list_traces(
            &conn,
            &trace_filter_params(
                project_id,
                vec![Filter::Number {
                    column: "total_tokens".to_string(),
                    operator: NumberOp::Gt,
                    value: 2500.0,
                }],
            ),
        )
        .expect("query");
        assert_eq!(
            rows.len(),
            1,
            "a trace displaying 3000 tokens must match `> 2500`; no single span reaches it"
        );
        assert_eq!(total, 1, "the count must agree with the page");

        let (rows, _) = list_traces(
            &conn,
            &trace_filter_params(
                project_id,
                vec![Filter::Number {
                    column: "total_tokens".to_string(),
                    operator: NumberOp::Lt,
                    value: 1500.0,
                }],
            ),
        )
        .expect("query");
        assert!(
            rows.is_empty(),
            "a trace displaying 3000 tokens must not match `< 1500` because one span does"
        );
    }

    /// A span id is unique only within a trace, so token dedup must not look outside one.
    ///
    /// The parent/child checks that stop a nested generation from being counted twice matched on span
    /// id alone. Two traces from the same framework routinely reuse an id shape, and a generation in
    /// trace B whose parent id equalled a span id in trace A suppressed trace A's tokens - the trace
    /// reported zero while its span carried usage.
    #[tokio::test]
    async fn token_dedup_does_not_reach_into_another_trace() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        // trace-1: a lone generation whose tokens must count.
        let lone = make_generation_span(project_id, "trace-1", "shared-id", None, 0.01, 500);
        // trace-2: a generation whose parent is "shared-id" - the same span id, another trace.
        let child =
            make_generation_span(project_id, "trace-2", "child", Some("shared-id"), 0.02, 700);
        let root = make_agent_span(project_id, "trace-2", "shared-id", None);
        {
            let conn = analytics.conn();
            insert_batch(&conn, &[lone, child, root]).expect("insert");
        }

        let conn = analytics.conn();
        let first = get_trace(&conn, project_id, "trace-1")
            .expect("query")
            .expect("trace exists");
        assert_eq!(
            first.total_tokens, 500,
            "trace-1's generation was suppressed by a same-id span in another trace"
        );
        let second = get_trace(&conn, project_id, "trace-2")
            .expect("query")
            .expect("trace exists");
        assert_eq!(
            second.total_tokens, 700,
            "trace-2 should count its own leaf generation once"
        );
    }

    /// A span with no observation type still reports what it was billed.
    ///
    /// Transport-level instrumentation records usage and cost on a plain span, leaving
    /// observation_type NULL. The billing branch for a non-generation span tested
    /// `observation_type != 'generation'`, which is NULL for such a span - so the row never
    /// qualified and the trace reported zero tokens and zero cost while its span carried both.
    #[tokio::test]
    async fn a_span_with_no_observation_type_is_still_billed() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        let mut plain = make_generation_span(project_id, "trace-1", "root", None, 0.0, 0);
        plain.observation_type = None;
        plain.gen_ai_usage_input_tokens = 11;
        plain.gen_ai_usage_output_tokens = 2;
        plain.gen_ai_usage_total_tokens = 13;
        plain.gen_ai_cost_total = 0.004;
        {
            let conn = analytics.conn();
            insert_batch(&conn, &[plain]).expect("insert");
        }

        let conn = analytics.conn();
        let trace = get_trace(&conn, project_id, "trace-1")
            .expect("query")
            .expect("trace exists");
        assert_eq!(
            trace.total_tokens, 13,
            "a plain span's token usage was not counted"
        );
        assert!(
            (trace.total_cost - 0.004).abs() < 1e-9,
            "a plain span's cost was not counted: {}",
            trace.total_cost
        );
    }

    /// A span reporting only a total token count is still billed.
    ///
    /// `gen_ai.usage.total_tokens` and OpenInference's `llm.token_count.total` are extracted on their
    /// own, with no input/output breakdown - and the predicate admitting rows to aggregation summed
    /// the input and output columns, which are zero for such a span. The trace was visible and
    /// reported zero tokens.
    #[tokio::test]
    async fn a_total_only_token_count_is_still_billed() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        let mut total_only = make_generation_span(project_id, "trace-1", "root", None, 0.0, 0);
        total_only.gen_ai_usage_input_tokens = 0;
        total_only.gen_ai_usage_output_tokens = 0;
        total_only.gen_ai_usage_total_tokens = 42;
        {
            let conn = analytics.conn();
            insert_batch(&conn, &[total_only]).expect("insert");
        }

        let conn = analytics.conn();
        let trace = get_trace(&conn, project_id, "trace-1")
            .expect("query")
            .expect("trace exists");
        assert_eq!(
            trace.total_tokens, 42,
            "a span reporting only a total was aggregated as zero"
        );
    }

    /// Cache and reasoning usage count on their own.
    ///
    /// Extraction accepts each of them independently of the input/output breakdown, so a span can
    /// report cache reads or reasoning tokens and nothing else - a prompt-cache hit that ran no new
    /// completion looks exactly like that. Admission summed input, output and total, all zero here,
    /// so the trace reported nothing.
    #[tokio::test]
    async fn cache_and_reasoning_usage_count_on_their_own() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        let mut cache_only = make_generation_span(project_id, "trace-1", "root", None, 0.0, 0);
        cache_only.gen_ai_usage_input_tokens = 0;
        cache_only.gen_ai_usage_output_tokens = 0;
        cache_only.gen_ai_usage_total_tokens = 0;
        cache_only.gen_ai_usage_cache_read_tokens = 700;

        let mut reasoning_only = make_generation_span(project_id, "trace-2", "root", None, 0.0, 0);
        reasoning_only.gen_ai_usage_input_tokens = 0;
        reasoning_only.gen_ai_usage_output_tokens = 0;
        reasoning_only.gen_ai_usage_total_tokens = 0;
        reasoning_only.gen_ai_usage_reasoning_tokens = 64;
        {
            let conn = analytics.conn();
            insert_batch(&conn, &[cache_only, reasoning_only]).expect("insert");
        }

        let conn = analytics.conn();
        let cached = get_trace(&conn, project_id, "trace-1")
            .expect("query")
            .expect("trace exists");
        assert_eq!(
            cached.cache_read_tokens, 700,
            "a cache-only span was aggregated as zero"
        );
        let reasoned = get_trace(&conn, project_id, "trace-2")
            .expect("query")
            .expect("trace exists");
        assert_eq!(
            reasoned.reasoning_tokens, 64,
            "a reasoning-only span was aggregated as zero"
        );
    }

    /// An empty "none of" is not a filter, and must not exclude everything.
    ///
    /// The negated form is rendered by excluding the matches of its positive twin, and an empty
    /// option list renders as `1 = 1` - so negating the subquery around it excluded every trace.
    /// "None of nothing" has to mean "everything".
    #[tokio::test]
    async fn an_empty_exclusion_list_filters_nothing() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let spans = vec![make_generation_span(
            project_id, "trace-1", "gen-1", None, 0.01, 100,
        )];
        {
            let conn = analytics.conn();
            insert_batch(&conn, &spans).expect("insert");
        }

        let conn = analytics.conn();
        let (rows, total) = list_traces(
            &conn,
            &trace_filter_params(
                project_id,
                vec![Filter::StringOptions {
                    column: "gen_ai_request_model".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec![],
                }],
            ),
        )
        .expect("query");
        assert_eq!(
            rows.len(),
            1,
            "an empty exclusion list excluded the trace instead of filtering nothing"
        );
        assert_eq!(total, 1, "the count must agree with the page");
    }

    /// A filtered session list shows the whole session, not the part that matched.
    ///
    /// Selection and membership are separate questions. Answering both with one predicate returned a
    /// session of two traces, filtered to one environment, carrying only that trace's times, counts
    /// and cost - while opening the session showed both traces.
    #[tokio::test]
    async fn a_filtered_session_is_still_the_whole_session() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        let mut first = make_generation_span(project_id, "trace-1", "gen-1", None, 0.01, 1000);
        first.session_id = Some("session-5".to_string());
        first.environment = Some("prod".to_string());
        let mut second = make_generation_span(project_id, "trace-2", "gen-2", None, 0.02, 2000);
        second.session_id = Some("session-5".to_string());
        second.environment = Some("dev".to_string());
        second.timestamp_start = first.timestamp_start + chrono::Duration::seconds(30);
        {
            let conn = analytics.conn();
            insert_batch(&conn, &[first, second]).expect("insert");
        }

        let conn = analytics.conn();
        let params = ListSessionsParams {
            project_id: project_id.to_string(),
            page: 1,
            limit: 50,
            environment: Some(vec!["prod".to_string()]),
            ..Default::default()
        };
        let (rows, total) = list_sessions(&conn, &params).expect("query");
        assert_eq!(rows.len(), 1, "the session matches through its prod trace");
        assert_eq!(total, 1, "the count must agree with the page");
        assert_eq!(
            rows[0].trace_count, 2,
            "the session has two traces; the filter selected it, it does not shrink it"
        );
        assert_eq!(
            rows[0].total_tokens, 3000,
            "both traces called the model: {} reported",
            rows[0].total_tokens
        );
    }

    /// A filter on a value the row displays matches that value, not any span's copy of it.
    ///
    /// A trace records its session on the root span; its children carry none. Asked of span rows,
    /// "session is null" was true of every such trace - the children satisfy it - so the filter
    /// returned traces displayed under a session, and no filter excluded them.
    #[tokio::test]
    async fn a_filter_on_a_displayed_attribute_matches_what_the_row_shows() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        let mut with_session = make_agent_span(project_id, "trace-has-session", "agent-1", None);
        with_session.session_id = Some("session-9".to_string());
        // The child carries no session id, which is what made the row-level check wrong.
        let child = make_generation_span(
            project_id,
            "trace-has-session",
            "gen-1",
            Some("agent-1"),
            0.01,
            100,
        );
        let lonely = make_generation_span(project_id, "trace-no-session", "gen-2", None, 0.01, 100);
        {
            let conn = analytics.conn();
            insert_batch(&conn, &[with_session, child, lonely]).expect("insert");
        }

        let conn = analytics.conn();
        let (rows, total) = list_traces(
            &conn,
            &trace_filter_params(
                project_id,
                vec![Filter::Null {
                    column: "session_id".to_string(),
                    operator: NullOp::IsNull,
                }],
            ),
        )
        .expect("query");
        let ids: Vec<&String> = rows.iter().map(|t| &t.trace_id).collect();
        assert_eq!(
            ids,
            vec![&"trace-no-session".to_string()],
            "only the trace displaying no session has none: {ids:?}"
        );
        assert_eq!(total, 1, "the count must agree with the page");
    }

    /// "None of" means no span used it, not that some span used something else.
    ///
    /// A trace that called claude-haiku once and another model next came back from the filter that
    /// excluded claude-haiku: the second span satisfied "not claude-haiku" on its own.
    #[tokio::test]
    async fn excluding_a_value_excludes_every_trace_that_used_it() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";

        let mut mixed = make_generation_span(project_id, "trace-mixed", "gen-1", None, 0.01, 100);
        mixed.gen_ai_request_model = Some("claude-haiku".to_string());
        let mut mixed_second =
            make_generation_span(project_id, "trace-mixed", "gen-2", Some("gen-1"), 0.01, 100);
        mixed_second.gen_ai_request_model = Some("claude-sonnet".to_string());
        let mut other = make_generation_span(project_id, "trace-other", "gen-3", None, 0.01, 100);
        other.gen_ai_request_model = Some("claude-sonnet".to_string());
        {
            let conn = analytics.conn();
            insert_batch(&conn, &[mixed, mixed_second, other]).expect("insert");
        }

        let conn = analytics.conn();
        let (rows, total) = list_traces(
            &conn,
            &trace_filter_params(
                project_id,
                vec![Filter::StringOptions {
                    column: "gen_ai_request_model".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["claude-haiku".to_string()],
                }],
            ),
        )
        .expect("query");
        let ids: Vec<&String> = rows.iter().map(|t| &t.trace_id).collect();
        assert_eq!(
            ids,
            vec![&"trace-other".to_string()],
            "the trace that used claude-haiku in one of its two calls must be excluded: {ids:?}"
        );
        assert_eq!(total, 1, "the count must agree with the page");
    }

    /// Two filters describe the trace, not one span of it.
    ///
    /// The session id lives on the root span and the tokens on its generation child - the ordinary
    /// shape for every framework that opens a root agent span. ANDing both conditions on a single
    /// span row asks for a span that carries the session *and* the tokens, which no span does, so
    /// the trace vanished from a list that showed it under either filter alone.
    #[tokio::test]
    async fn two_trace_filters_describe_the_trace_not_one_span() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-split-attributes";

        let mut root = make_agent_span(project_id, trace_id, "agent-1", None);
        root.session_id = Some("session-7".to_string());
        let child =
            make_generation_span(project_id, trace_id, "gen-1", Some("agent-1"), 0.02, 2000);
        {
            let conn = analytics.conn();
            insert_batch(&conn, &[root, child]).expect("insert");
        }

        let conn = analytics.conn();
        let session_only = trace_filter_params(
            project_id,
            vec![Filter::String {
                column: "session_id".to_string(),
                operator: StringOp::Eq,
                value: "session-7".to_string(),
            }],
        );
        let (rows, _) = list_traces(&conn, &session_only).expect("query");
        assert_eq!(rows.len(), 1, "premise: the session filter alone matches");

        let mut both = session_only.clone();
        both.filters.push(Filter::Number {
            column: "total_tokens".to_string(),
            operator: NumberOp::Gt,
            value: 100.0,
        });
        let (rows, total) = list_traces(&conn, &both).expect("query");
        assert_eq!(
            rows.len(),
            1,
            "the session is on the root span and the tokens on its child; both describe the trace"
        );
        assert_eq!(total, 1, "the count must agree with the page");
    }
}
