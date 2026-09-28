use super::*;
use super::{render::*, traces::*};

/// List canonical sessions while keeping selection separate from membership.
///
/// A filter selects a session when any span of any trace canonically assigned to that session
/// satisfies it. Once selected, the row aggregates every trace in the session, not only the traces
/// or spans that matched the filter.
pub fn list_sessions(params: &ListSessionsParams, backend: Backend) -> PageQuery {
    match backend {
        Backend::Duckdb => duckdb_session_page(params),
        Backend::Clickhouse => clickhouse_session_page(params),
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
