use super::render::*;
use super::*;

pub(super) fn trace_conditions(
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

pub(super) fn trace_aggregate_expression(column: &str, backend: Backend) -> Option<String> {
    let display_dialect = match backend {
        Backend::Duckdb => crate::display::DisplayNameDialect::DuckDb,
        Backend::Clickhouse => crate::display::DisplayNameDialect::ClickHouse,
    };
    let totals = |column: &str| {
        let aggregate = match backend {
            Backend::Duckdb => "MAX",
            Backend::Clickhouse => "max",
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
        }),
        "end_time" => Some(match backend {
            Backend::Duckdb => "MAX(COALESCE(n.timestamp_end, n.timestamp_start))".to_string(),
            Backend::Clickhouse => "max(coalesce(n.timestamp_end, n.timestamp_start))".to_string(),
        }),
        "duration_ms" => Some(match backend {
            Backend::Duckdb => "DATE_DIFF('millisecond', MIN(n.timestamp_start), \
                 MAX(COALESCE(n.timestamp_end, n.timestamp_start)))"
                .to_string(),
            Backend::Clickhouse => "dateDiff('millisecond', min(n.timestamp_start), \
                 max(coalesce(n.timestamp_end, n.timestamp_start)))"
                .to_string(),
        }),
        "input_tokens" | "output_tokens" | "total_tokens" | "cache_read_tokens"
        | "cache_write_tokens" | "reasoning_tokens" | "input_cost" | "output_cost"
        | "cache_read_cost" | "cache_write_cost" | "reasoning_cost" | "total_cost" => {
            totals(column)
        }
        _ => None,
    }
}

pub(super) fn trace_totals_context(
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
    }
}

pub(super) fn duckdb_gen_totals_sql(where_clause: &str) -> String {
    duckdb_gen_totals_joined_sql("g.trace_id", "", where_clause)
}

pub(super) fn duckdb_gen_totals_joined_sql(key: &str, join: &str, where_clause: &str) -> String {
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

pub(super) fn clickhouse_dedup_lookup(extra_where: &str) -> String {
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

pub(super) fn clickhouse_gen_totals_cte(key: Option<&str>, scope: &str) -> String {
    clickhouse_gen_totals_joined_cte(key, "", scope)
}

pub(super) fn clickhouse_gen_totals_joined_cte(
    key: Option<&str>,
    join: &str,
    scope: &str,
) -> String {
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

pub(super) fn trace_sort(params: &ListTracesParams) -> (&'static str, &'static str) {
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

pub(super) fn duckdb_trace_projection() -> String {
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

pub(super) fn clickhouse_trace_projection() -> String {
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

pub(super) fn duckdb_trace_page(params: &ListTracesParams) -> PageQuery {
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

pub(super) fn clickhouse_time_scoped_dedup(params: &ListTracesParams) -> (String, Vec<QueryValue>) {
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

pub(super) fn clickhouse_trace_page(params: &ListTracesParams) -> PageQuery {
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
