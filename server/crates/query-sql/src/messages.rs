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
    gen_ai_cost_output::DOUBLE AS cost_output,
    request_thread"#
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
    toFloat64(gen_ai_cost_output) AS cost_output,
    request_thread"#
        }
    }
}

fn winner_source(backend: Backend, watermark: Option<i64>) -> (String, Vec<QueryValue>) {
    match (backend, watermark) {
        (Backend::Duckdb, watermark) => crate::winners::duckdb_winning_spans(watermark),
        (Backend::Clickhouse, Some(watermark)) => (
            "(SELECT * FROM otel_spans \
              WHERE toInt64(toUnixTimestamp64Micro(ingested_at)) < ? \
              ORDER BY ingested_at DESC LIMIT 1 BY project_id, trace_id, span_id)"
                .to_string(),
            vec![QueryValue::Int64(watermark)],
        ),
        (Backend::Clickhouse, None) => ("(SELECT * FROM otel_spans FINAL)".to_string(), Vec::new()),
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
            let (winner, winner_values) = crate::winners::duckdb_winner_condition(watermark);
            let mut values = vec![QueryValue::String(project_id.to_string())];
            values.extend(winner_values);
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
                         WHERE project_id = ? AND {winner} \
                           AND trace_id IN (\
                             SELECT trace_id FROM otel_spans \
                             WHERE project_id = ? AND session_id = ?\
                           )\
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

/// The columns every message read returns, in the order both row parsers read them.
///
/// The span projection's aliases, then `log_messages`. Listed once because the outer select names each
/// column rather than `s.*`: a join's star expansion is where the two dialects disagree about naming.
const MESSAGE_COLUMNS: &[&str] = &[
    "trace_id",
    "span_id",
    "parent_span_id",
    "span_timestamp_us",
    "span_end_timestamp_us",
    "messages",
    "model",
    "provider",
    "status_code",
    "exception_type",
    "exception_message",
    "exception_stacktrace",
    "input_tokens",
    "output_tokens",
    "total_tokens",
    "cost_total",
    "tool_definitions",
    "tool_names",
    "observation_type",
    "session_id",
    "ingested_at_us",
    "scope_name",
    "scope_version",
    "span_name",
    "framework",
    "response_model",
    "response_id",
    "temperature",
    "top_p",
    "max_tokens",
    "finish_reasons",
    "cache_read_tokens",
    "cache_write_tokens",
    "reasoning_tokens",
    "cost_input",
    "cost_output",
    "request_thread",
];

/// Every non-empty `otel_logs.messages` array for the selected spans, one row per `(trace, span)`.
///
/// The log store's winner is read - one row per `(log_digest, ordinal)`, since a re-sent record is the same
/// record - and the watermark is applied to that winner, on both backends alike. DuckDB keeps only the
/// latest delivery of an identity, so "the newest delivery is before the watermark" is the only question it
/// can answer; ClickHouse is asked the same one, which is what makes the answers equivalent. The arrays are
/// concatenated in `(timestamp, log_digest, ordinal)` order, a total order over identities, so the result is
/// the same bytes on both.
///
/// `scope` narrows the log rows to the span selector's traces, so a trace read aggregates that trace's logs
/// rather than the project's.
fn log_messages_source(
    backend: Backend,
    project_id: &str,
    watermark: Option<i64>,
    scope: Option<(String, Vec<QueryValue>)>,
) -> (String, Vec<QueryValue>) {
    let mut values = vec![QueryValue::String(project_id.to_string())];
    let mut conditions = vec![
        "project_id = ?".to_string(),
        "trace_id IS NOT NULL".to_string(),
        "span_id IS NOT NULL".to_string(),
        "messages != '[]'".to_string(),
    ];
    if let Some(watermark) = watermark {
        conditions.push(match backend {
            Backend::Duckdb => "EPOCH_US(ingested_at) < ?::BIGINT".to_string(),
            Backend::Clickhouse => "toInt64(toUnixTimestamp64Micro(ingested_at)) < ?".to_string(),
        });
        values.push(QueryValue::Int64(watermark));
    }
    if let Some((condition, scope_values)) = scope {
        conditions.push(condition);
        values.extend(scope_values);
    }
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT trace_id AS log_trace_id, span_id AS log_span_id, \
                    '[' || string_agg(substr(messages, 2, length(messages) - 2), ',' \
                                      ORDER BY timestamp, log_digest, ordinal) || ']' AS aggregated \
             FROM otel_logs \
             WHERE {} \
             GROUP BY trace_id, span_id",
            conditions.join(" AND ")
        ),
        // `FINAL` before the filter: the watermark and the non-empty test apply to the winning delivery,
        // which is what DuckDB's single row per identity is.
        Backend::Clickhouse => format!(
            "SELECT assumeNotNull(trace_id) AS log_trace_id, assumeNotNull(span_id) AS log_span_id, \
                    concat('[', arrayStringConcat(arrayMap(entry -> entry.4, arraySort(groupArray(\
                        (timestamp, log_digest, ordinal, substring(messages, 2, length(messages) - 2))\
                    ))), ','), ']') AS aggregated \
             FROM otel_logs FINAL \
             WHERE {} \
             GROUP BY log_trace_id, log_span_id",
            conditions.join(" AND ")
        ),
    };
    (sql, values)
}

/// Join each selected span row to the messages its log records carry, in one statement.
///
/// The span rows are selected exactly as before; the log aggregate is a `LEFT JOIN`, so a span with no log
/// records is unchanged and a log record whose span is absent attaches to nothing. `filter`, `order` and
/// `limit` apply to the joined rows, so the content filter sees `log_messages` and a feed page is cut after
/// it. Values bind in text order: the span source, the span conditions, then the log aggregate.
fn join_log_messages(
    backend: Backend,
    spans: (String, Vec<QueryValue>),
    logs: (String, Vec<QueryValue>),
    filter: Option<&str>,
    order: &str,
    limit: Option<u32>,
) -> ParameterizedQuery {
    let (spans_sql, mut values) = spans;
    let (logs_sql, log_values) = logs;
    values.extend(log_values);
    let columns = MESSAGE_COLUMNS
        .iter()
        .map(|column| format!("s.{column} AS {column}"))
        .collect::<Vec<_>>()
        .join(", ");
    // An unmatched `LEFT JOIN` row is NULL on DuckDB and the type's default, '', on ClickHouse.
    let log_messages = match backend {
        Backend::Duckdb => "COALESCE(l.aggregated, '[]')",
        Backend::Clickhouse => {
            "if(ifNull(l.aggregated, '') = '', '[]', ifNull(l.aggregated, '[]'))"
        }
    };
    let filter = filter.map(|f| format!(" WHERE {f}")).unwrap_or_default();
    let limit = limit.map(|n| format!("\nLIMIT {n}")).unwrap_or_default();
    ParameterizedQuery::new(
        format!(
            "SELECT * FROM (\n\
             SELECT {columns}, {log_messages} AS log_messages\n\
             FROM ({spans_sql}) s\n\
             LEFT JOIN ({logs_sql}) l ON l.log_trace_id = s.trace_id AND l.log_span_id = s.span_id\n\
             ){filter}\n\
             ORDER BY {order}{limit}"
        ),
        values,
    )
}

/// Read raw message-bearing span context for the highest-priority selector in `params`.
pub fn get_messages(params: &MessageQueryParams, backend: Backend) -> ParameterizedQuery {
    let (source, mut values) = winner_source(backend, params.ingested_before_us);
    let mut conditions = vec!["project_id = ?".to_string()];
    values.push(QueryValue::String(params.project_id.to_string()));
    let mut filter = None;
    // The same selector, narrowing the log rows. A session is the only selector that is a subquery, and on
    // ClickHouse it must be `GLOBAL IN`: the log aggregate reads the `Distributed` table directly, and a plain
    // `IN` over another distributed table is refused by `distributed_product_mode`.
    let mut log_scope: Option<(String, Vec<QueryValue>)> = None;

    if let Some(span_id) = &params.span_id {
        conditions.push("span_id = ?".to_string());
        values.push(QueryValue::String(span_id.clone()));
        let mut scope = (
            "span_id = ?".to_string(),
            vec![QueryValue::String(span_id.clone())],
        );
        if let Some(trace_id) = &params.trace_id {
            conditions.push("trace_id = ?".to_string());
            values.push(QueryValue::String(trace_id.clone()));
            scope.0.push_str(" AND trace_id = ?");
            scope.1.push(QueryValue::String(trace_id.clone()));
        }
        log_scope = Some(scope);
    } else if let Some(session_id) = &params.session_id {
        let session_traces = session_trace_ids(
            params.project_id.as_str(),
            session_id,
            params.ingested_before_us,
            backend,
        );
        conditions.push(format!("trace_id IN ({})", session_traces.sql()));
        values.extend_from_slice(session_traces.params());
        let membership = match backend {
            Backend::Duckdb => "IN",
            Backend::Clickhouse => "GLOBAL IN",
        };
        log_scope = Some((
            format!("trace_id {membership} ({})", session_traces.sql()),
            session_traces.params().to_vec(),
        ));
        filter = Some(MESSAGE_CONTENT_FILTER);
    } else if let Some(trace_id) = &params.trace_id {
        conditions.push("trace_id = ?".to_string());
        values.push(QueryValue::String(trace_id.clone()));
        log_scope = Some((
            "trace_id = ?".to_string(),
            vec![QueryValue::String(trace_id.clone())],
        ));
        filter = Some(MESSAGE_CONTENT_FILTER);
    } else if let Some(trace_ids) = &params.trace_ids {
        if trace_ids.is_empty() {
            conditions.push("1 = 0".to_string());
            log_scope = Some(("1 = 0".to_string(), Vec::new()));
        } else {
            let placeholders = std::iter::repeat_n("?", trace_ids.len())
                .collect::<Vec<_>>()
                .join(", ");
            conditions.push(format!("trace_id IN ({placeholders})"));
            values.extend(trace_ids.iter().cloned().map(QueryValue::String));
            log_scope = Some((
                format!("trace_id IN ({placeholders})"),
                trace_ids.iter().cloned().map(QueryValue::String).collect(),
            ));
        }
        filter = Some(MESSAGE_CONTENT_FILTER);
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

    join_log_messages(
        backend,
        (
            format!(
                "SELECT\n{}\nFROM {source}\nWHERE {}",
                message_projection(backend),
                conditions.join(" AND "),
            ),
            values,
        ),
        log_messages_source(
            backend,
            params.project_id.as_str(),
            params.ingested_before_us,
            log_scope,
        ),
        filter,
        "span_timestamp_us ASC, trace_id ASC, span_id ASC",
        None,
    )
}

/// Read one stable project-wide page of message-bearing spans.
///
/// The page is ordered and cut by the span's own `ingested_at`, so a log record arriving after its span was
/// paged does not bring the span back: the next traversal sees it, this one does not.
pub fn get_project_messages(params: &FeedMessagesParams, backend: Backend) -> ParameterizedQuery {
    let (source, mut values) = winner_source(backend, params.ingested_before_us);
    let mut conditions = vec!["project_id = ?".to_string()];
    values.push(QueryValue::String(params.project_id.to_string()));

    if let Some((cursor_time_us, cursor_span_id, cursor_trace_id)) = &params.cursor {
        let ingested = match backend {
            Backend::Duckdb => "EPOCH_US(ingested_at)",
            Backend::Clickhouse => "toInt64(toUnixTimestamp64Micro(ingested_at))",
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

    if backend == Backend::Duckdb {
        return duckdb_project_messages(params, &conditions.join(" AND "), values);
    }
    join_log_messages(
        backend,
        (
            format!(
                "SELECT\n{}\nFROM {source}\nWHERE {}",
                message_projection(backend),
                conditions.join(" AND "),
            ),
            values,
        ),
        log_messages_source(
            backend,
            params.project_id.as_str(),
            params.ingested_before_us,
            None,
        ),
        Some(MESSAGE_CONTENT_FILTER),
        "ingested_at_us DESC, span_id DESC, trace_id DESC",
        Some(params.limit),
    )
}

/// DuckDB's project feed picks its page before it reads what the page shows. Joining every span's columns to
/// every span's aggregated log messages and filtering the result held the project's log messages in one
/// aggregate - a state DuckDB cannot spill - and at a million spans ran out of the memory limit. The page is
/// chosen on the spans' own columns and on which spans have log messages at all, then only its rows are read whole,
/// by row id, and only their log messages aggregated, and the content filter applies to the joined rows as before:
/// the same rows, in the same order.
fn duckdb_project_messages(
    params: &FeedMessagesParams,
    conditions: &str,
    span_values: Vec<QueryValue>,
) -> ParameterizedQuery {
    // The winner condition binds first, as the winner relation's would: `span_values` is in that order.
    let (winner, _) = crate::winners::duckdb_winner_condition(params.ingested_before_us);
    let (carriers_scope, carrier_values) = log_message_carriers(params);
    let (_, log_values) = log_messages_source(
        Backend::Duckdb,
        params.project_id.as_str(),
        params.ingested_before_us,
        None,
    );
    let (aggregate, _) = log_messages_source(
        Backend::Duckdb,
        params.project_id.as_str(),
        params.ingested_before_us,
        Some((
            "(trace_id, span_id) IN (SELECT trace_id, span_id FROM picked)".to_string(),
            Vec::new(),
        )),
    );
    let columns = MESSAGE_COLUMNS
        .iter()
        .map(|column| format!("s.{column} AS {column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let limit = params.limit;
    let sql = format!(
        "WITH carriers AS ({carriers_scope}), \
         picked AS (SELECT rowid AS row_id, trace_id, span_id FROM otel_spans WHERE {winner} AND {conditions} \
           AND (messages != '[]' OR tool_definitions != '[]' OR tool_names != '[]' OR status_code = 'ERROR' \
                OR (trace_id, span_id) IN (SELECT trace_id, span_id FROM carriers)) \
           ORDER BY ingested_at DESC, span_id DESC, trace_id DESC LIMIT {limit})\n\
         SELECT * FROM (\n\
         SELECT {columns}, COALESCE(l.aggregated, '[]') AS log_messages\n\
         FROM (SELECT\n{projection}\nFROM otel_spans WHERE rowid IN (SELECT row_id FROM picked)) s\n\
         LEFT JOIN ({aggregate}) l ON l.log_trace_id = s.trace_id AND l.log_span_id = s.span_id\n\
         ) WHERE {MESSAGE_CONTENT_FILTER}\n\
         ORDER BY ingested_at_us DESC, span_id DESC, trace_id DESC\nLIMIT {limit}",
        projection = message_projection(Backend::Duckdb),
    );
    let mut values = carrier_values;
    values.extend(span_values);
    values.extend(log_values);
    ParameterizedQuery::new(sql, values)
}

/// The spans with log messages, as DuckDB's feed aggregates them: the same conditions, without the aggregate.
fn log_message_carriers(params: &FeedMessagesParams) -> (String, Vec<QueryValue>) {
    let mut values = vec![QueryValue::String(params.project_id.to_string())];
    let mut conditions = vec![
        "project_id = ?",
        "trace_id IS NOT NULL",
        "span_id IS NOT NULL",
        "messages != '[]'",
    ];
    if let Some(watermark) = params.ingested_before_us {
        conditions.push("EPOCH_US(ingested_at) < ?::BIGINT");
        values.push(QueryValue::Int64(watermark));
    }
    (
        format!(
            "SELECT DISTINCT trace_id, span_id FROM otel_logs WHERE {}",
            conditions.join(" AND ")
        ),
        values,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use sideseat_ports::types::ProjectId;

    /// How many times a span winner as of a watermark binds it: DuckDB's condition names it twice
    /// (`crate::winners`), ClickHouse's ranking once.
    fn span_watermark_binds(backend: Backend) -> usize {
        match backend {
            Backend::Duckdb => 2,
            Backend::Clickhouse => 1,
        }
    }

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
            let watermarks = span_watermark_binds(backend);
            assert!(
                query.params()[..watermarks]
                    .iter()
                    .all(|value| value == &QueryValue::Int64(99))
            );
            assert_eq!(
                query.params().get(watermarks),
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
            // DuckDB finds the spans with log messages, picks the page and then joins their messages to it
            // (`duckdb_project_messages`): the log bound is bound twice.
            let (span_reads, log_reads) = match backend {
                Backend::Duckdb => (1, 2),
                Backend::Clickhouse => (1, 1),
            };
            let count = |wanted: &QueryValue| {
                feed.params()
                    .iter()
                    .filter(|value| *value == wanted)
                    .count()
            };
            assert_eq!(
                count(&QueryValue::Int64(100)),
                span_reads * span_watermark_binds(backend) + log_reads
            );
            assert_eq!(count(&QueryValue::Int64(42)), span_reads);
            assert!(
                feed.sql()
                    .contains("ORDER BY ingested_at_us DESC, span_id DESC, trace_id DESC")
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
                // Span rows, their session membership and the log aggregate's session scope are span
                // winner conditions; the log aggregate's own bound is one.
                3 * span_watermark_binds(backend) + 1,
                "span rows, their session membership, the log aggregate and its session scope must all \
                 use one traversal instant"
            );
            assert!(query.sql().contains("canonical_session = ?"));
        }
    }

    fn every_selector() -> Vec<MessageQueryParams> {
        let project_id = ProjectId::from("p");
        vec![
            MessageQueryParams {
                project_id: project_id.clone(),
                span_id: Some("s".to_string()),
                trace_id: Some("t".to_string()),
                ingested_before_us: Some(7),
                ..Default::default()
            },
            MessageQueryParams {
                project_id: project_id.clone(),
                span_id: Some("s".to_string()),
                ..Default::default()
            },
            MessageQueryParams {
                project_id: project_id.clone(),
                session_id: Some("session".to_string()),
                ingested_before_us: Some(7),
                ..Default::default()
            },
            MessageQueryParams {
                project_id: project_id.clone(),
                trace_id: Some("t".to_string()),
                ..Default::default()
            },
            MessageQueryParams {
                project_id: project_id.clone(),
                trace_ids: Some(vec!["t1".to_string(), "t2".to_string()]),
                ingested_before_us: Some(7),
                ..Default::default()
            },
            MessageQueryParams {
                project_id,
                trace_ids: Some(Vec::new()),
                ..Default::default()
            },
        ]
    }

    /// One statement per read, every placeholder bound, and the log aggregate narrowed by the same
    /// selector and bounded by the same watermark as the span rows.
    #[test]
    fn log_messages_join_in_the_same_statement_with_matching_binds() {
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            for params in every_selector() {
                let query = get_messages(&params, backend);
                assert_eq!(
                    query.sql().matches('?').count(),
                    query.params().len(),
                    "{backend:?} {params:?}"
                );
                assert_eq!(query.sql().matches("LEFT JOIN").count(), 1);
                assert!(query.sql().contains("FROM otel_logs"));
                let watermarks = query
                    .params()
                    .iter()
                    .filter(|value| matches!(value, QueryValue::Int64(7)))
                    .count();
                let span = span_watermark_binds(backend);
                match params.ingested_before_us {
                    // The span source and the log aggregate, plus both copies of session membership.
                    Some(_) if params.session_id.is_some() && params.span_id.is_none() => {
                        assert_eq!(watermarks, 3 * span + 1)
                    }
                    Some(_) => assert_eq!(watermarks, span + 1, "{backend:?} {params:?}"),
                    None => assert_eq!(watermarks, 0),
                }
                let filtered = query.sql().contains(MESSAGE_CONTENT_FILTER);
                assert_eq!(
                    filtered,
                    params.span_id.is_none(),
                    "the span view alone applies no content filter"
                );
            }
            let feed = get_project_messages(
                &FeedMessagesParams {
                    project_id: ProjectId::from("p"),
                    limit: 5,
                    ingested_before_us: Some(7),
                    ..Default::default()
                },
                backend,
            );
            assert_eq!(feed.sql().matches('?').count(), feed.params().len());
            assert_eq!(
                feed.params()
                    .iter()
                    .filter(|value| matches!(value, QueryValue::Int64(7)))
                    .count(),
                match backend {
                    // The log messages are found, to pick the page, and then joined to it.
                    Backend::Duckdb => span_watermark_binds(backend) + 2,
                    Backend::Clickhouse => span_watermark_binds(backend) + 1,
                },
                "a feed page and the log messages joined to it describe one instant"
            );
            let filter_at = feed.sql().find(MESSAGE_CONTENT_FILTER).expect("filtered");
            let limit_at = feed.sql().rfind("LIMIT 5").expect("limited");
            assert!(
                filter_at < limit_at && feed.sql().ends_with("LIMIT 5"),
                "the page is cut after the joined rows are filtered"
            );
        }
    }

    #[test]
    fn the_content_filter_admits_a_span_whose_only_messages_are_in_logs() {
        assert!(MESSAGE_CONTENT_FILTER.contains("log_messages != '[]'"));
    }

    #[test]
    fn a_session_scopes_clickhouse_logs_with_a_global_subquery() {
        let params = MessageQueryParams {
            project_id: ProjectId::from("p"),
            session_id: Some("session".to_string()),
            ..Default::default()
        };
        assert!(
            get_messages(&params, Backend::Clickhouse)
                .sql()
                .contains("trace_id GLOBAL IN (")
        );
        assert!(
            !get_messages(&params, Backend::Duckdb)
                .sql()
                .contains("GLOBAL")
        );
    }
}
