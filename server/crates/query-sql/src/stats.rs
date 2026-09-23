//! Typed statements for the project statistics dashboard.
//!
//! The plan owns statement structure, winner selection, token de-duplication, bind order and
//! timezone-aware bucket boundaries. Analytical adapters only execute and decode these statements.

use chrono::{DateTime, Duration, Utc};
use sideseat_core::constants::QUERY_MAX_TOP_STATS;
use sideseat_ports::types::StatsParams;

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};

/// Every statement needed to assemble one project-statistics response.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectStatsQueryPlan {
    pub main: ParameterizedQuery,
    pub canonical_sessions: ParameterizedQuery,
    pub previous_traces: ParameterizedQuery,
    pub average_trace_duration: ParameterizedQuery,
    pub frameworks: ParameterizedQuery,
    pub models: ParameterizedQuery,
    pub token_trend: Option<ParameterizedQuery>,
    pub latency_trend: Option<ParameterizedQuery>,
    pub recent_activity: ParameterizedQuery,
}

/// Build the complete statistics plan.
pub fn project_stats(
    params: &StatsParams,
    now: DateTime<Utc>,
    buckets: &[BucketWindow],
    backend: Backend,
) -> ProjectStatsQueryPlan {
    assert_analytics_backend(backend);
    let period = params.to_timestamp - params.from_timestamp;
    let previous_from = params.from_timestamp - period;

    ProjectStatsQueryPlan {
        main: main_aggregation(params, backend),
        canonical_sessions: canonical_session_count(params, backend),
        previous_traces: trace_count(
            params.project_id.as_str(),
            previous_from,
            params.from_timestamp,
            backend,
        ),
        average_trace_duration: average_trace_duration(params, backend),
        frameworks: framework_breakdown(params, backend),
        models: model_breakdown(params, backend),
        token_trend: (!buckets.is_empty()).then(|| token_trend(params, buckets, backend)),
        latency_trend: (!buckets.is_empty()).then(|| latency_trend(params, buckets, backend)),
        recent_activity: trace_count(
            params.project_id.as_str(),
            now - Duration::minutes(5),
            now,
            backend,
        ),
    }
}

fn main_aggregation(params: &StatsParams, backend: Backend) -> ParameterizedQuery {
    let source = winning_spans(backend);
    let (lookup, mut values) = token_lookup(params, backend);
    let token_condition = token_dedup_condition(backend, &source);
    values.extend(window_values(params, backend));
    values.extend(window_values(params, backend));

    let count_distinct = match backend {
        Backend::Duckdb => "COUNT(DISTINCT s.trace_id)",
        Backend::Clickhouse => "count(DISTINCT s.trace_id)",
    };
    let count_spans = match backend {
        Backend::Duckdb => "COUNT(*)",
        Backend::Clickhouse => "count()",
    };
    let unique_users = match backend {
        Backend::Duckdb => "COUNT(DISTINCT s.user_id) FILTER (WHERE s.user_id IS NOT NULL)",
        Backend::Clickhouse => "countIf(DISTINCT s.user_id, s.user_id IS NOT NULL)",
    };
    let sums = token_sums(backend);
    let costs = cost_projection(backend, "ga");
    let lookup_prefix = optional_with_prefix(&lookup);

    ParameterizedQuery::new(
        format!(
            r#"WITH {lookup_prefix}gen_agg AS (
    SELECT
        {sums}
    FROM {source} g
    WHERE g.project_id = ?
      AND {from_predicate}
      AND {to_predicate}
      AND {token_condition}
)
SELECT
    {count_distinct} AS traces,
    {count_spans} AS spans,
    {unique_users} AS unique_users,
    COALESCE(MAX(ga.input_tokens), 0) AS input_tokens,
    COALESCE(MAX(ga.output_tokens), 0) AS output_tokens,
    COALESCE(MAX(ga.total_tokens), 0) AS total_tokens,
    COALESCE(MAX(ga.cache_read_tokens), 0) AS cache_read_tokens,
    COALESCE(MAX(ga.cache_write_tokens), 0) AS cache_write_tokens,
    COALESCE(MAX(ga.reasoning_tokens), 0) AS reasoning_tokens,
    {costs}
FROM {source} s
CROSS JOIN gen_agg ga
WHERE s.project_id = ?
  AND {main_from_predicate}
  AND {main_to_predicate}"#,
            from_predicate = timestamp_predicate("g.timestamp_start", ">=", backend),
            to_predicate = timestamp_predicate("g.timestamp_start", "<=", backend),
            main_from_predicate = timestamp_predicate("s.timestamp_start", ">=", backend),
            main_to_predicate = timestamp_predicate("s.timestamp_start", "<=", backend),
        ),
        values,
    )
}

fn canonical_session_count(params: &StatsParams, backend: Backend) -> ParameterizedQuery {
    let source = winning_spans(backend);
    let aggregate = match backend {
        Backend::Duckdb => "arg_min(raw.session_id, (raw.timestamp_start, raw.span_id))",
        Backend::Clickhouse => {
            "argMin(assumeNotNull(raw.session_id), (raw.timestamp_start, raw.span_id))"
        }
    };
    let count = match backend {
        Backend::Duckdb => "COUNT(DISTINCT c.session_id)",
        Backend::Clickhouse => "count(DISTINCT c.session_id)",
    };
    let mut values = vec![QueryValue::String(params.project_id.to_string())];
    values.extend(window_values(params, backend));

    ParameterizedQuery::new(
        format!(
            r#"WITH canonical AS (
    SELECT raw.project_id, raw.trace_id, {aggregate} AS session_id
    FROM {source} raw
    WHERE raw.project_id = ?
      AND raw.session_id IS NOT NULL
      AND raw.session_id != ''
    GROUP BY raw.project_id, raw.trace_id
),
active_traces AS (
    SELECT DISTINCT active.project_id, active.trace_id
    FROM {source} active
    WHERE active.project_id = ?
      AND {from_predicate}
      AND {to_predicate}
)
SELECT {count} AS count
FROM canonical c
INNER JOIN active_traces a
    ON a.project_id = c.project_id AND a.trace_id = c.trace_id"#,
            from_predicate = timestamp_predicate("active.timestamp_start", ">=", backend),
            to_predicate = timestamp_predicate("active.timestamp_start", "<=", backend),
        ),
        values,
    )
}

fn trace_count(
    project_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    backend: Backend,
) -> ParameterizedQuery {
    let source = winning_spans(backend);
    let count = match backend {
        Backend::Duckdb => "COUNT(DISTINCT trace_id)",
        Backend::Clickhouse => "count(DISTINCT trace_id)",
    };
    ParameterizedQuery::new(
        format!(
            r#"SELECT {count} AS count
FROM {source}
WHERE project_id = ?
  AND {from_predicate}
  AND {to_predicate}"#,
            from_predicate = timestamp_predicate("timestamp_start", ">=", backend),
            to_predicate = timestamp_predicate("timestamp_start", "<=", backend),
        ),
        vec![
            QueryValue::String(project_id.to_string()),
            timestamp_value(from, backend),
            timestamp_value(to, backend),
        ],
    )
}

fn average_trace_duration(params: &StatsParams, backend: Backend) -> ParameterizedQuery {
    let source = winning_spans(backend);
    let min = match backend {
        Backend::Duckdb => "MIN(timestamp_start)",
        Backend::Clickhouse => "min(timestamp_start)",
    };
    let max = match backend {
        Backend::Duckdb => "MAX(COALESCE(timestamp_end, timestamp_start))",
        Backend::Clickhouse => "max(coalesce(timestamp_end, timestamp_start))",
    };
    let average = match backend {
        Backend::Duckdb => "AVG(DATE_DIFF('millisecond', min_ts, max_ts))::DOUBLE",
        Backend::Clickhouse => "avg(dateDiff('millisecond', min_ts, max_ts))",
    };

    ParameterizedQuery::new(
        format!(
            r#"WITH traces AS (
    SELECT trace_id, {min} AS min_ts, {max} AS max_ts
    FROM {source}
    WHERE project_id = ?
      AND {from_predicate}
      AND {to_predicate}
    GROUP BY trace_id
)
SELECT {average} AS avg_duration_ms
FROM traces"#,
            from_predicate = timestamp_predicate("timestamp_start", ">=", backend),
            to_predicate = timestamp_predicate("timestamp_start", "<=", backend),
        ),
        window_values(params, backend),
    )
}

fn framework_breakdown(params: &StatsParams, backend: Backend) -> ParameterizedQuery {
    let source = winning_spans(backend);
    let count = match backend {
        Backend::Duckdb => "COUNT(DISTINCT trace_id)",
        Backend::Clickhouse => "count(DISTINCT trace_id)",
    };
    let round = match backend {
        Backend::Duckdb => "ROUND",
        Backend::Clickhouse => "round",
    };
    let mut values = window_values(params, backend);
    values.extend(window_values(params, backend));

    ParameterizedQuery::new(
        format!(
            r#"WITH genai_traces AS (
    SELECT DISTINCT trace_id
    FROM {source}
    WHERE project_id = ?
      AND {first_from}
      AND {first_to}
      AND observation_type != 'span'
),
framework_counts AS (
    SELECT framework, {count} AS count
    FROM {source}
    WHERE project_id = ?
      AND {second_from}
      AND {second_to}
      AND trace_id IN (SELECT trace_id FROM genai_traces)
    GROUP BY framework
),
total AS (
    SELECT COALESCE(SUM(count), 1) AS total FROM framework_counts
)
SELECT
    fc.framework,
    fc.count,
    {round}(100.0 * fc.count / t.total, 1) AS percentage
FROM framework_counts fc, total t
ORDER BY fc.count DESC
LIMIT {QUERY_MAX_TOP_STATS}"#,
            first_from = timestamp_predicate("timestamp_start", ">=", backend),
            first_to = timestamp_predicate("timestamp_start", "<=", backend),
            second_from = timestamp_predicate("timestamp_start", ">=", backend),
            second_to = timestamp_predicate("timestamp_start", "<=", backend),
        ),
        values,
    )
}

fn model_breakdown(params: &StatsParams, backend: Backend) -> ParameterizedQuery {
    let source = winning_spans(backend);
    let (lookup, mut values) = token_lookup(params, backend);
    values.extend(window_values(params, backend));
    let token_condition = token_dedup_condition(backend, &source);
    let token_sum = match backend {
        Backend::Duckdb => "COALESCE(SUM(gen_ai_usage_total_tokens), 0)",
        Backend::Clickhouse => "sum(gen_ai_usage_total_tokens)",
    };
    let cost_sum = match backend {
        Backend::Duckdb => "ROUND(COALESCE(SUM(gen_ai_cost_total), 0)::DOUBLE, 4)",
        Backend::Clickhouse => "round(sum(toFloat64(gen_ai_cost_total)), 4)",
    };
    let round = match backend {
        Backend::Duckdb => "ROUND",
        Backend::Clickhouse => "round",
    };
    let lookup_prefix = optional_with_prefix(&lookup);

    ParameterizedQuery::new(
        format!(
            r#"WITH {lookup_prefix}gen_roots AS (
    SELECT
        g.gen_ai_request_model,
        g.gen_ai_usage_total_tokens,
        g.gen_ai_cost_total
    FROM {source} g
    WHERE g.project_id = ?
      AND {from_predicate}
      AND {to_predicate}
      AND {token_condition}
),
model_stats AS (
    SELECT
        gen_ai_request_model AS model,
        {token_sum} AS tokens,
        {cost_sum} AS cost
    FROM gen_roots
    GROUP BY gen_ai_request_model
),
total AS (
    SELECT COALESCE(SUM(tokens), 1) AS total FROM model_stats
)
SELECT
    ms.model,
    ms.tokens,
    ms.cost,
    {round}(100.0 * ms.tokens / t.total, 1) AS percentage
FROM model_stats ms, total t
ORDER BY ms.tokens DESC
LIMIT {QUERY_MAX_TOP_STATS}"#,
            from_predicate = timestamp_predicate("g.timestamp_start", ">=", backend),
            to_predicate = timestamp_predicate("g.timestamp_start", "<=", backend),
        ),
        values,
    )
}

fn token_trend(
    params: &StatsParams,
    buckets: &[BucketWindow],
    backend: Backend,
) -> ParameterizedQuery {
    let source = winning_spans(backend);
    let (bucket_relation, mut values) = bucket_relation(buckets, backend);
    values.push(QueryValue::String(params.project_id.to_string()));
    let (lookup, lookup_values) = token_lookup(params, backend);
    values.extend(lookup_values);
    values.extend(window_values(params, backend));
    let lookup_clause = optional_comma_clause(&lookup);
    let token_condition = token_dedup_condition(backend, &source);
    let sum = match backend {
        Backend::Duckdb => "COALESCE(SUM(gr.total_tokens), 0)::BIGINT",
        Backend::Clickhouse => "sum(gr.total_tokens)",
    };
    let bucket_value = bucket_micros("ab.bucket", backend);

    ParameterizedQuery::new(
        format!(
            r#"WITH bucket_values AS (
    {bucket_relation}
),
all_buckets AS (
    SELECT bucket, bucket_end, ? AS project_id
    FROM bucket_values
){lookup_clause},
gen_roots AS (
    SELECT
        g.project_id,
        g.timestamp_start,
        COALESCE(g.gen_ai_usage_total_tokens, 0) AS total_tokens
    FROM {source} g
    WHERE g.project_id = ?
      AND {from_predicate}
      AND {to_predicate}
      AND {token_condition}
),
bucketed_data AS (
    SELECT br.bucket, {sum} AS tokens
    FROM all_buckets br
    LEFT JOIN gen_roots gr
      ON gr.project_id = br.project_id
     AND gr.timestamp_start >= br.bucket
     AND gr.timestamp_start < br.bucket_end
    GROUP BY br.bucket
)
SELECT {bucket_value} AS bucket, COALESCE(bd.tokens, 0) AS tokens
FROM all_buckets ab
LEFT JOIN bucketed_data bd ON bd.bucket = ab.bucket
ORDER BY bucket ASC"#,
            from_predicate = timestamp_predicate("g.timestamp_start", ">=", backend),
            to_predicate = timestamp_predicate("g.timestamp_start", "<=", backend),
        ),
        values,
    )
}

fn latency_trend(
    params: &StatsParams,
    buckets: &[BucketWindow],
    backend: Backend,
) -> ParameterizedQuery {
    let source = winning_spans(backend);
    let (bucket_relation, mut values) = bucket_relation(buckets, backend);
    values.push(QueryValue::String(params.project_id.to_string()));
    values.extend(window_values(params, backend));
    let min = match backend {
        Backend::Duckdb => "MIN(timestamp_start)",
        Backend::Clickhouse => "min(timestamp_start)",
    };
    let max = match backend {
        Backend::Duckdb => "MAX(COALESCE(timestamp_end, timestamp_start))",
        Backend::Clickhouse => "max(coalesce(timestamp_end, timestamp_start))",
    };
    let average = match backend {
        Backend::Duckdb => "AVG(DATE_DIFF('millisecond', t.min_ts, t.max_ts))::DOUBLE",
        Backend::Clickhouse => "avg(dateDiff('millisecond', t.min_ts, t.max_ts))",
    };
    let bucket_value = bucket_micros("ab.bucket", backend);

    ParameterizedQuery::new(
        format!(
            r#"WITH bucket_values AS (
    {bucket_relation}
),
all_buckets AS (
    SELECT bucket, bucket_end, ? AS project_id
    FROM bucket_values
),
traces AS (
    SELECT project_id, trace_id, {min} AS min_ts, {max} AS max_ts
    FROM {source}
    WHERE project_id = ?
      AND {from_predicate}
      AND {to_predicate}
    GROUP BY project_id, trace_id
),
bucketed_data AS (
    SELECT br.bucket, {average} AS avg_duration_ms
    FROM all_buckets br
    LEFT JOIN traces t
      ON t.project_id = br.project_id
     AND t.min_ts >= br.bucket
     AND t.min_ts < br.bucket_end
    GROUP BY br.bucket
)
SELECT
    {bucket_value} AS bucket,
    COALESCE(bd.avg_duration_ms, 0.0) AS avg_duration_ms
FROM all_buckets ab
LEFT JOIN bucketed_data bd ON bd.bucket = ab.bucket
ORDER BY bucket ASC"#,
            from_predicate = timestamp_predicate("timestamp_start", ">=", backend),
            to_predicate = timestamp_predicate("timestamp_start", "<=", backend),
        ),
        values,
    )
}

fn winning_spans(backend: Backend) -> String {
    match backend {
        Backend::Duckdb => "(SELECT * FROM otel_spans \
            QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
            ORDER BY ingested_at DESC, rowid DESC) = 1)"
            .to_string(),
        Backend::Clickhouse => "(SELECT * FROM otel_spans FINAL)".to_string(),
    }
}

fn timestamp_predicate(column: &str, operator: &str, backend: Backend) -> String {
    match backend {
        Backend::Duckdb => format!("{column} {operator} ?"),
        Backend::Clickhouse => format!("{column} {operator} fromUnixTimestamp64Micro(?)"),
    }
}

fn timestamp_value(value: DateTime<Utc>, backend: Backend) -> QueryValue {
    match backend {
        Backend::Duckdb => QueryValue::String(value.to_rfc3339()),
        Backend::Clickhouse => QueryValue::Int64(value.timestamp_micros()),
    }
}

fn window_values(params: &StatsParams, backend: Backend) -> Vec<QueryValue> {
    vec![
        QueryValue::String(params.project_id.to_string()),
        timestamp_value(params.from_timestamp, backend),
        timestamp_value(params.to_timestamp, backend),
    ]
}

fn token_lookup(params: &StatsParams, backend: Backend) -> (String, Vec<QueryValue>) {
    match backend {
        Backend::Duckdb => (String::new(), Vec::new()),
        Backend::Clickhouse => (
            format!(
                r#"dedup_lookup AS (
    SELECT span_id, parent_span_id, trace_id, observation_type
    FROM otel_spans FINAL
    WHERE project_id = ?
      AND trace_id IN (
          SELECT DISTINCT trace_id
          FROM otel_spans FINAL
          WHERE project_id = ?
            AND {}
            AND {}
      )
      AND ((gen_ai_usage_input_tokens + gen_ai_usage_output_tokens
            + gen_ai_usage_total_tokens + gen_ai_usage_cache_read_tokens
            + gen_ai_usage_cache_write_tokens + gen_ai_usage_reasoning_tokens) > 0
           OR gen_ai_cost_total > 0)
)"#,
                timestamp_predicate("timestamp_start", ">=", backend),
                timestamp_predicate("timestamp_start", "<=", backend),
            ),
            vec![
                QueryValue::String(params.project_id.to_string()),
                QueryValue::String(params.project_id.to_string()),
                timestamp_value(params.from_timestamp, backend),
                timestamp_value(params.to_timestamp, backend),
            ],
        ),
    }
}

fn token_dedup_condition(backend: Backend, duck_source: &str) -> String {
    match backend {
        Backend::Duckdb => format!(
            r#"(
    (g.observation_type = 'generation'
     AND {tokenful}
     AND NOT EXISTS (
         SELECT 1 FROM {duck_source} c
         WHERE c.parent_span_id = g.span_id
           AND c.trace_id = g.trace_id
           AND c.project_id = g.project_id
           AND c.observation_type = 'generation'
           AND {child_tokenful}
     ))
    OR
    ((g.observation_type IS NULL OR g.observation_type != 'generation')
     AND {tokenful}
     AND NOT EXISTS (
         SELECT 1 FROM {duck_source} gen
         WHERE gen.trace_id = g.trace_id
           AND gen.project_id = g.project_id
           AND gen.observation_type = 'generation'
           AND {generation_tokenful}
     )
     AND NOT EXISTS (
         SELECT 1 FROM {duck_source} p
         WHERE p.span_id = g.parent_span_id
           AND p.trace_id = g.trace_id
           AND p.project_id = g.project_id
           AND {parent_tokenful}
     ))
)"#,
            tokenful = tokenful("g"),
            child_tokenful = tokenful("c"),
            generation_tokenful = tokenful("gen"),
            parent_tokenful = tokenful("p"),
        ),
        Backend::Clickhouse => format!(
            r#"(
    (g.observation_type = 'generation'
     AND {tokenful}
     AND (g.trace_id, g.span_id) NOT IN (
         SELECT trace_id, parent_span_id
         FROM dedup_lookup
         WHERE observation_type = 'generation' AND parent_span_id IS NOT NULL
     ))
    OR
    ((g.observation_type IS NULL OR g.observation_type != 'generation')
     AND {tokenful}
     AND g.trace_id NOT IN (
         SELECT DISTINCT trace_id
         FROM dedup_lookup
         WHERE observation_type = 'generation'
     )
     AND (g.parent_span_id IS NULL OR (g.trace_id, g.parent_span_id) NOT IN (
         SELECT trace_id, span_id FROM dedup_lookup
     )))
)"#,
            tokenful = tokenful("g"),
        ),
    }
}

fn tokenful(alias: &str) -> String {
    format!(
        "(({alias}.gen_ai_usage_input_tokens + {alias}.gen_ai_usage_output_tokens \
         + {alias}.gen_ai_usage_total_tokens + {alias}.gen_ai_usage_cache_read_tokens \
         + {alias}.gen_ai_usage_cache_write_tokens + {alias}.gen_ai_usage_reasoning_tokens) > 0 \
         OR {alias}.gen_ai_cost_total > 0)"
    )
}

fn token_sums(backend: Backend) -> String {
    let cost = |column: &str| match backend {
        Backend::Duckdb => format!("COALESCE(SUM({column}), 0)"),
        Backend::Clickhouse => format!("COALESCE(SUM(toFloat64({column})), 0)"),
    };
    format!(
        r#"COALESCE(SUM(gen_ai_usage_input_tokens), 0) AS input_tokens,
        COALESCE(SUM(gen_ai_usage_output_tokens), 0) AS output_tokens,
        COALESCE(SUM(gen_ai_usage_total_tokens), 0) AS total_tokens,
        COALESCE(SUM(gen_ai_usage_cache_read_tokens), 0) AS cache_read_tokens,
        COALESCE(SUM(gen_ai_usage_cache_write_tokens), 0) AS cache_write_tokens,
        COALESCE(SUM(gen_ai_usage_reasoning_tokens), 0) AS reasoning_tokens,
        {} AS input_cost,
        {} AS output_cost,
        {} AS cache_read_cost,
        {} AS cache_write_cost,
        {} AS reasoning_cost,
        {} AS total_cost"#,
        cost("gen_ai_cost_input"),
        cost("gen_ai_cost_output"),
        cost("gen_ai_cost_cache_read"),
        cost("gen_ai_cost_cache_write"),
        cost("gen_ai_cost_reasoning"),
        cost("gen_ai_cost_total"),
    )
}

fn cost_projection(backend: Backend, alias: &str) -> String {
    let expression = |column: &str| match backend {
        Backend::Duckdb => format!("ROUND(COALESCE(MAX({alias}.{column}), 0)::DOUBLE, 4)"),
        Backend::Clickhouse => format!("round(COALESCE(MAX({alias}.{column}), 0), 4)"),
    };
    format!(
        r#"{} AS input_cost,
    {} AS output_cost,
    {} AS cache_read_cost,
    {} AS cache_write_cost,
    {} AS reasoning_cost,
    {} AS total_cost"#,
        expression("input_cost"),
        expression("output_cost"),
        expression("cache_read_cost"),
        expression("cache_write_cost"),
        expression("reasoning_cost"),
        expression("total_cost"),
    )
}

fn optional_with_prefix(cte: &str) -> String {
    if cte.is_empty() {
        String::new()
    } else {
        format!("{cte},\n")
    }
}

fn optional_comma_clause(cte: &str) -> String {
    if cte.is_empty() {
        String::new()
    } else {
        format!(",\n{cte}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

fn bucket_relation(buckets: &[BucketWindow], backend: Backend) -> (String, Vec<QueryValue>) {
    let row = match backend {
        Backend::Duckdb => {
            "SELECT CAST(? AS TIMESTAMP) AS bucket, CAST(? AS TIMESTAMP) AS bucket_end"
        }
        Backend::Clickhouse => {
            "SELECT fromUnixTimestamp64Micro(?) AS bucket, \
             fromUnixTimestamp64Micro(?) AS bucket_end"
        }
    };
    let mut values = Vec::with_capacity(buckets.len() * 2);
    for bucket in buckets {
        values.push(timestamp_value(bucket.start, backend));
        values.push(timestamp_value(bucket.end, backend));
    }
    (
        vec![row; buckets.len()].join("\n    UNION ALL\n    "),
        values,
    )
}

fn bucket_micros(column: &str, backend: Backend) -> String {
    match backend {
        Backend::Duckdb => format!("EPOCH_US({column})"),
        Backend::Clickhouse => format!("toInt64(toUnixTimestamp64Micro({column}))"),
    }
}

fn assert_analytics_backend(backend: Backend) {
    assert!(
        matches!(backend, Backend::Duckdb | Backend::Clickhouse),
        "{} is not an analytics backend",
        backend.name()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideseat_ports::types::ProjectId;

    fn params() -> StatsParams {
        StatsParams {
            project_id: ProjectId::from("project-secret"),
            from_timestamp: DateTime::parse_from_rfc3339("2024-01-17T10:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
            to_timestamp: DateTime::parse_from_rfc3339("2024-01-17T13:45:00Z")
                .unwrap()
                .with_timezone(&Utc),
            timezone: "Europe/London".parse().unwrap(),
        }
    }

    fn buckets() -> Vec<BucketWindow> {
        let starts = [
            "2024-01-17T10:00:00Z",
            "2024-01-17T11:00:00Z",
            "2024-01-17T12:00:00Z",
            "2024-01-17T13:00:00Z",
        ]
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .unwrap()
                .with_timezone(&Utc)
        });
        starts
            .iter()
            .enumerate()
            .map(|(index, start)| BucketWindow {
                start: *start,
                end: starts
                    .get(index + 1)
                    .copied()
                    .unwrap_or(*start + Duration::hours(1)),
            })
            .collect()
    }

    #[test]
    fn every_stats_value_is_bound_and_every_read_uses_winning_rows() {
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let plan = project_stats(
                &params(),
                DateTime::parse_from_rfc3339("2024-01-17T14:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                &buckets(),
                backend,
            );
            let statements = [
                &plan.main,
                &plan.canonical_sessions,
                &plan.previous_traces,
                &plan.average_trace_duration,
                &plan.frameworks,
                &plan.models,
                plan.token_trend.as_ref().unwrap(),
                plan.latency_trend.as_ref().unwrap(),
                &plan.recent_activity,
            ];
            for statement in statements {
                assert!(!statement.sql().contains("project-secret"));
                assert!(!statement.sql().contains("2024-01-17T10:30:00"));
                match backend {
                    Backend::Duckdb => assert!(statement.sql().contains("ROW_NUMBER()")),
                    Backend::Clickhouse => {
                        assert!(statement.sql().contains("FINAL"));
                        assert!(
                            !statement.sql().contains("FINAL s")
                                && !statement.sql().contains("FINAL g")
                                && !statement.sql().contains("FINAL raw")
                                && !statement.sql().contains("FINAL active"),
                            "ClickHouse requires aliases before FINAL; winner relations use subqueries"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn clickhouse_token_queries_use_materialized_anti_joins() {
        let plan = project_stats(
            &params(),
            DateTime::parse_from_rfc3339("2024-01-17T14:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            &buckets(),
            Backend::Clickhouse,
        );
        for statement in [&plan.main, &plan.models, plan.token_trend.as_ref().unwrap()] {
            assert!(statement.sql().contains("dedup_lookup AS"));
            assert!(statement.sql().contains("NOT IN"));
            assert!(!statement.sql().contains("NOT EXISTS"));
        }
    }

    #[test]
    fn canonical_sessions_are_tenant_keyed_and_buckets_are_typed_values() {
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let plan = project_stats(
                &params(),
                DateTime::parse_from_rfc3339("2024-01-17T14:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                &buckets(),
                backend,
            );
            assert!(
                plan.canonical_sessions
                    .sql()
                    .contains("a.project_id = c.project_id AND a.trace_id = c.trace_id")
            );
            assert!(plan.canonical_sessions.sql().contains("raw.session_id"));
            assert!(plan.canonical_sessions.sql().contains(" AS count"));
            assert!(
                !plan
                    .canonical_sessions
                    .sql()
                    .contains("AND session_id IS NOT NULL"),
                "the ClickHouse aggregate alias must not shadow the source session_id in WHERE"
            );
        }
        let plan = project_stats(
            &params(),
            DateTime::parse_from_rfc3339("2024-01-17T14:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            &buckets(),
            Backend::Duckdb,
        );
        let trend = plan.token_trend.unwrap();
        assert_eq!(
            trend
                .params()
                .iter()
                .filter(|value| matches!(value, QueryValue::String(value) if value.contains('T')))
                .count(),
            10,
            "four hourly windows contribute start/end values before the two query-scope timestamps"
        );
        assert!(!trend.sql().contains("2024-01-17"));
    }
}
