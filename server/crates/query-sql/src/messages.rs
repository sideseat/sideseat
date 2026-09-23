//! Typed statements for message-bearing span reads.

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};
use crate::display::MESSAGE_CONTENT_FILTER;
use sideseat_ports::types::{FeedMessagesParams, MessageQueryParams};

fn message_projection(backend: Backend) -> &'static str {
    match backend {
        Backend::Duckdb => {
            r#"trace_id,
    span_id,
    parent_span_id,
    EPOCH_US(timestamp_start) AS span_timestamp_us,
    EPOCH_US(timestamp_end) AS span_end_timestamp_us,
    messages,
    gen_ai_request_model AS model,
    gen_ai_system AS provider,
    status_code,
    exception_type,
    exception_message,
    exception_stacktrace,
    gen_ai_usage_input_tokens AS input_tokens,
    gen_ai_usage_output_tokens AS output_tokens,
    gen_ai_usage_total_tokens AS total_tokens,
    gen_ai_cost_total::DOUBLE AS cost_total,
    tool_definitions,
    tool_names,
    observation_type,
    session_id,
    EPOCH_US(ingested_at) AS ingested_at_us,
    scope_name,
    scope_version,
    span_name,
    framework,
    gen_ai_response_model AS response_model,
    gen_ai_response_id AS response_id,
    gen_ai_temperature AS temperature,
    gen_ai_top_p AS top_p,
    gen_ai_max_tokens AS max_tokens,
    gen_ai_finish_reasons AS finish_reasons,
    gen_ai_usage_cache_read_tokens AS cache_read_tokens,
    gen_ai_usage_cache_write_tokens AS cache_write_tokens,
    gen_ai_usage_reasoning_tokens AS reasoning_tokens,
    gen_ai_cost_input::DOUBLE AS cost_input,
    gen_ai_cost_output::DOUBLE AS cost_output"#
        }
        Backend::Clickhouse => {
            r#"trace_id,
    span_id,
    parent_span_id,
    toInt64(toUnixTimestamp64Micro(timestamp_start)) AS span_timestamp_us,
    if(timestamp_end IS NULL, NULL,
       toInt64(toUnixTimestamp64Micro(timestamp_end))) AS span_end_timestamp_us,
    messages,
    gen_ai_request_model AS model,
    gen_ai_system AS provider,
    status_code,
    exception_type,
    exception_message,
    exception_stacktrace,
    gen_ai_usage_input_tokens AS input_tokens,
    gen_ai_usage_output_tokens AS output_tokens,
    gen_ai_usage_total_tokens AS total_tokens,
    toFloat64(gen_ai_cost_total) AS cost_total,
    tool_definitions,
    tool_names,
    observation_type,
    session_id,
    toInt64(toUnixTimestamp64Micro(ingested_at)) AS ingested_at_us,
    scope_name,
    scope_version,
    span_name,
    framework,
    gen_ai_response_model AS response_model,
    gen_ai_response_id AS response_id,
    gen_ai_temperature AS temperature,
    gen_ai_top_p AS top_p,
    gen_ai_max_tokens AS max_tokens,
    gen_ai_finish_reasons AS finish_reasons,
    gen_ai_usage_cache_read_tokens AS cache_read_tokens,
    gen_ai_usage_cache_write_tokens AS cache_write_tokens,
    gen_ai_usage_reasoning_tokens AS reasoning_tokens,
    toFloat64(gen_ai_cost_input) AS cost_input,
    toFloat64(gen_ai_cost_output) AS cost_output"#
        }
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    }
}

fn winner_source(backend: Backend, watermark: Option<i64>) -> (String, Vec<QueryValue>) {
    match (backend, watermark) {
        (Backend::Duckdb, Some(watermark)) => (
            "(SELECT * FROM otel_spans WHERE EPOCH_US(ingested_at) < ?::BIGINT \
             QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                        ORDER BY ingested_at DESC, rowid DESC) = 1)"
                .to_string(),
            vec![QueryValue::Int64(watermark)],
        ),
        (Backend::Duckdb, None) => (
            "(SELECT * FROM otel_spans \
             QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                        ORDER BY ingested_at DESC, rowid DESC) = 1)"
                .to_string(),
            Vec::new(),
        ),
        (Backend::Clickhouse, Some(watermark)) => (
            "(SELECT * FROM otel_spans \
              WHERE toInt64(toUnixTimestamp64Micro(ingested_at)) < ? \
              ORDER BY ingested_at DESC LIMIT 1 BY project_id, trace_id, span_id)"
                .to_string(),
            vec![QueryValue::Int64(watermark)],
        ),
        (Backend::Clickhouse, None) => ("(SELECT * FROM otel_spans FINAL)".to_string(), Vec::new()),
        (Backend::Sqlite | Backend::Postgres, _) => {
            panic!("{} is not an analytics query backend", backend.name())
        }
    }
}

fn timestamp_condition(
    column: &str,
    operator: &str,
    value: &chrono::DateTime<chrono::Utc>,
    backend: Backend,
) -> (String, QueryValue) {
    match backend {
        Backend::Duckdb => (
            format!("{column} {operator} ?"),
            QueryValue::String(value.to_rfc3339()),
        ),
        Backend::Clickhouse => (
            format!("{column} {operator} fromUnixTimestamp64Micro(?)"),
            QueryValue::Int64(value.timestamp_micros()),
        ),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

fn traces_of_session(
    project_id: &str,
    session_id: &str,
    watermark: Option<i64>,
    backend: Backend,
) -> (String, Vec<QueryValue>) {
    match backend {
        Backend::Duckdb => {
            let watermark_sql = if watermark.is_some() {
                "AND EPOCH_US(ingested_at) < ?::BIGINT "
            } else {
                ""
            };
            let mut values = vec![QueryValue::String(project_id.to_string())];
            values.extend(watermark.map(QueryValue::Int64));
            values.push(QueryValue::String(project_id.to_string()));
            values.push(QueryValue::String(session_id.to_string()));
            values.push(QueryValue::String(session_id.to_string()));
            (
                format!(
                    "SELECT trace_id FROM (\
                       SELECT trace_id, \
                              arg_min(session_id, (timestamp_start, span_id)) \
                              AS canonical_session \
                       FROM (\
                         SELECT * FROM otel_spans \
                         WHERE project_id = ? {watermark_sql}\
                           AND trace_id IN (\
                             SELECT trace_id FROM otel_spans \
                             WHERE project_id = ? AND session_id = ?\
                           ) \
                         QUALIFY ROW_NUMBER() OVER (\
                           PARTITION BY project_id, trace_id, span_id \
                           ORDER BY ingested_at DESC, rowid DESC\
                         ) = 1\
                       ) candidates \
                       WHERE session_id IS NOT NULL AND session_id != '' \
                       GROUP BY trace_id\
                     ) canonical \
                     WHERE canonical_session = ?"
                ),
                values,
            )
        }
        Backend::Clickhouse => {
            let (source, mut values) = winner_source(backend, watermark);
            values.push(QueryValue::String(project_id.to_string()));
            values.push(QueryValue::String(project_id.to_string()));
            values.push(QueryValue::String(session_id.to_string()));
            values.push(QueryValue::String(session_id.to_string()));
            (
                format!(
                    "SELECT trace_id FROM (\
                       SELECT trace_id, \
                              argMin(assumeNotNull(session_id), \
                                     (timestamp_start, span_id)) AS canonical_session \
                       FROM {source} \
                       WHERE project_id = ? \
                         AND trace_id IN (\
                           SELECT trace_id FROM otel_spans \
                           WHERE project_id = ? AND session_id = ?\
                         ) \
                         AND session_id IS NOT NULL AND session_id != '' \
                       GROUP BY trace_id\
                     ) canonical \
                     WHERE canonical_session = ?"
                ),
                values,
            )
        }
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

/// Resolve one session to its canonical trace ids at the requested traversal instant.
pub fn session_trace_ids(
    project_id: &str,
    session_id: &str,
    watermark: Option<i64>,
    backend: Backend,
) -> ParameterizedQuery {
    let (sql, params) = traces_of_session(project_id, session_id, watermark, backend);
    ParameterizedQuery::new(sql, params)
}

/// Read raw message-bearing span context for the highest-priority selector in `params`.
pub fn get_messages(params: &MessageQueryParams, backend: Backend) -> ParameterizedQuery {
    let (source, mut values) = winner_source(backend, params.ingested_before_us);
    let mut conditions = vec!["project_id = ?".to_string()];
    values.push(QueryValue::String(params.project_id.to_string()));

    if let Some(span_id) = &params.span_id {
        conditions.push("span_id = ?".to_string());
        values.push(QueryValue::String(span_id.clone()));
        if let Some(trace_id) = &params.trace_id {
            conditions.push("trace_id = ?".to_string());
            values.push(QueryValue::String(trace_id.clone()));
        }
    } else if let Some(session_id) = &params.session_id {
        let session_traces = session_trace_ids(
            params.project_id.as_str(),
            session_id,
            params.ingested_before_us,
            backend,
        );
        conditions.push(format!("trace_id IN ({})", session_traces.sql()));
        values.extend_from_slice(session_traces.params());
        conditions.push(MESSAGE_CONTENT_FILTER.to_string());
    } else if let Some(trace_id) = &params.trace_id {
        conditions.push("trace_id = ?".to_string());
        values.push(QueryValue::String(trace_id.clone()));
        conditions.push(MESSAGE_CONTENT_FILTER.to_string());
    } else if let Some(trace_ids) = &params.trace_ids {
        if trace_ids.is_empty() {
            conditions.push("1 = 0".to_string());
        } else {
            conditions.push(format!(
                "trace_id IN ({})",
                std::iter::repeat_n("?", trace_ids.len())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            values.extend(trace_ids.iter().cloned().map(QueryValue::String));
        }
        conditions.push(MESSAGE_CONTENT_FILTER.to_string());
    }

    if let Some(from) = &params.from_timestamp {
        let (condition, value) = timestamp_condition("timestamp_start", ">=", from, backend);
        conditions.push(condition);
        values.push(value);
    }
    if let Some(to) = &params.to_timestamp {
        let (condition, value) = timestamp_condition("timestamp_start", "<", to, backend);
        conditions.push(condition);
        values.push(value);
    }

    ParameterizedQuery::new(
        format!(
            "SELECT\n{}\nFROM {source}\nWHERE {}\n\
             ORDER BY timestamp_start ASC, trace_id ASC, span_id ASC",
            message_projection(backend),
            conditions.join(" AND "),
        ),
        values,
    )
}

/// Read one stable project-wide page of message-bearing spans.
pub fn get_project_messages(params: &FeedMessagesParams, backend: Backend) -> ParameterizedQuery {
    let (source, mut values) = winner_source(backend, params.ingested_before_us);
    let mut conditions = vec![
        "project_id = ?".to_string(),
        MESSAGE_CONTENT_FILTER.to_string(),
    ];
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
        let (condition, value) = timestamp_condition(
            "COALESCE(timestamp_end, timestamp_start)",
            ">=",
            start,
            backend,
        );
        conditions.push(condition);
        values.push(value);
    }
    if let Some(end) = &params.end_time {
        let (condition, value) = timestamp_condition("timestamp_start", "<", end, backend);
        conditions.push(condition);
        values.push(value);
    }

    ParameterizedQuery::new(
        format!(
            "SELECT\n{}\nFROM {source}\nWHERE {}\n\
             ORDER BY ingested_at DESC, span_id DESC, trace_id DESC\nLIMIT {}",
            message_projection(backend),
            conditions.join(" AND "),
            params.limit,
        ),
        values,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use sideseat_ports::types::ProjectId;

    #[test]
    fn message_selector_priority_and_watermark_bind_order_are_explicit() {
        let params = MessageQueryParams {
            project_id: ProjectId::from("tenant-'quoted"),
            span_id: Some("span-'quoted".to_string()),
            trace_id: Some("trace-'quoted".to_string()),
            session_id: Some("ignored-session".to_string()),
            trace_ids: Some(vec!["ignored-trace".to_string()]),
            ingested_before_us: Some(99),
            ..Default::default()
        };
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = get_messages(&params, backend);
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert_eq!(query.params().first(), Some(&QueryValue::Int64(99)));
            assert_eq!(
                query.params().get(1),
                Some(&QueryValue::String("tenant-'quoted".to_string()))
            );
            assert!(query.sql().contains("span_id = ? AND trace_id = ?"));
            assert!(!query.sql().contains("ignored-session"));
            assert!(!query.sql().contains("ignored-trace"));
        }
    }

    #[test]
    fn empty_trace_batch_is_fail_closed_and_project_feed_uses_total_cursor() {
        let empty = MessageQueryParams {
            project_id: ProjectId::from("p"),
            trace_ids: Some(Vec::new()),
            ..Default::default()
        };
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = get_messages(&empty, backend);
            assert!(query.sql().contains("1 = 0"));
            assert!(query.sql().contains(MESSAGE_CONTENT_FILTER));

            let feed = get_project_messages(
                &FeedMessagesParams {
                    project_id: ProjectId::from("tenant-'quoted"),
                    limit: 17,
                    cursor: Some((42, "span-'quoted".to_string(), "trace-'quoted".to_string())),
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
                    ingested_before_us: Some(100),
                },
                backend,
            );
            assert_eq!(feed.sql().matches('?').count(), feed.params().len());
            assert_eq!(feed.params().first(), Some(&QueryValue::Int64(100)));
            assert_eq!(feed.params().get(2), Some(&QueryValue::Int64(42)));
            assert!(
                feed.sql()
                    .contains("ORDER BY ingested_at DESC, span_id DESC, trace_id DESC")
            );
            assert!(feed.sql().ends_with("LIMIT 17"));
            assert!(!feed.sql().contains("tenant-'quoted"));
        }
    }

    #[test]
    fn session_membership_uses_the_same_watermark_as_message_rows() {
        let params = MessageQueryParams {
            project_id: ProjectId::from("p"),
            session_id: Some("session-a".to_string()),
            ingested_before_us: Some(123),
            ..Default::default()
        };
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let query = get_messages(&params, backend);
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert_eq!(
                query
                    .params()
                    .iter()
                    .filter(|value| matches!(value, QueryValue::Int64(123)))
                    .count(),
                2,
                "outer rows and session membership must use one traversal instant"
            );
            assert!(query.sql().contains("canonical_session = ?"));
        }
    }
}
