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
    request_thread,
    span_marks,
    request_frame"#
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
    request_thread,
    span_marks,
    request_frame"#
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
                    concat!(
                        "SELECT trace_id FROM (\
                       SELECT trace_id, ",
                        duckdb_canonical_session!(),
                        " AS canonical_session \
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
                    winner = winner
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
                    concat!(
                        "SELECT trace_id FROM (\
                       SELECT trace_id, ",
                        clickhouse_canonical_session!(),
                        " AS canonical_session \
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
                    source = source
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
    "span_marks",
    "request_frame",
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
        "messages_bytes > 0".to_string(),
    ];
    if let Some(watermark) = watermark {
        conditions.push(match backend {
            Backend::Duckdb => "EPOCH_US(ingested_at) < ?::BIGINT".to_string(),
            Backend::Clickhouse => "toInt64(toUnixTimestamp64Micro(ingested_at)) < ?".to_string(),
        });
        values.push(QueryValue::Int64(watermark));
    }
    // ClickHouse reads the scope as `PREWHERE`, which it does not choose itself under `FINAL`: the record's trace and
    // span are part of its identity, the same in every delivery, so filtering them before the deliveries are merged
    // keeps the same winners - and the raw records of the rows it leaves out are never read.
    let mut prewhere = String::new();
    if let Some((condition, scope_values)) = scope {
        match backend {
            Backend::Duckdb => {
                conditions.push(condition);
                values.extend(scope_values);
            }
            Backend::Clickhouse => {
                prewhere = format!("PREWHERE project_id = ? AND {condition} ");
                let mut scoped = vec![QueryValue::String(project_id.to_string())];
                scoped.extend(scope_values);
                scoped.extend(values);
                values = scoped;
            }
        }
    }
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT trace_id AS log_trace_id, span_id AS log_span_id, \
                    '[' || string_agg({source}, ',' ORDER BY timestamp, log_digest, ordinal) || ']' AS aggregated \
             FROM otel_logs \
             WHERE {} \
             GROUP BY trace_id, span_id",
            conditions.join(" AND "),
            source = log_message_source(Backend::Duckdb),
        ),
        // `FINAL` before the filter: the watermark and the non-empty test apply to the winning delivery,
        // which is what DuckDB's single row per identity is.
        Backend::Clickhouse => format!(
            "SELECT assumeNotNull(trace_id) AS log_trace_id, assumeNotNull(span_id) AS log_span_id, \
                    {sources} AS aggregated, {token} \
             FROM otel_logs FINAL {prewhere}\
             WHERE {} \
             GROUP BY log_trace_id, log_span_id",
            conditions.join(" AND "),
            sources = CLICKHOUSE_LOG_MESSAGE_SOURCES,
            token = CLICKHOUSE_CARRIER_TOKEN,
        ),
    };
    (sql, values)
}

/// One log record's messages as the store answers them (`LogMessageSource`), a JSON object: the stored array, or the
/// raw record and what the derivation reads beside it where the messages are not stored, which the domain then
/// rebuilds (`domain::log_messages`) - never both, so a read carries the stored array or the inputs ingest bounded
/// it by (`ingest::settle_views`). A record carries messages exactly where `messages_bytes` is above zero.
pub(crate) fn log_message_source(backend: Backend) -> String {
    log_message_source_of(
        backend,
        ["messages", "raw_log", "scope_name", "noted_references"],
    )
}

/// [`log_message_source`] over these expressions for the record's messages, raw record, scope and noted references
/// - a read that carries them out of a scan under other names.
pub(crate) fn log_message_source_of(
    backend: Backend,
    [messages, raw_log, scope, noted]: [&str; 4],
) -> String {
    match backend {
        Backend::Duckdb => format!(
            "CAST(json_object('m', {messages}, \
                              'r', CASE WHEN {messages} IS NULL THEN CAST({raw_log} AS VARCHAR) END, \
                              's', CASE WHEN {messages} IS NULL THEN {scope} END, \
                              'n', CASE WHEN {messages} IS NULL THEN {noted} END) AS VARCHAR)"
        ),
        Backend::Clickhouse => format!(
            "concat('{{\"m\":', if(isNull({messages}), 'null', toJSONString(assumeNotNull({messages}))), \
                    ',\"r\":', if(isNull({messages}), toJSONString(ifNull({raw_log}, '')), 'null'), \
                    ',\"s\":', if(isNull({messages}) AND isNotNull({scope}), \
                                   toJSONString(assumeNotNull({scope})), 'null'), \
                    ',\"n\":', if(isNull({messages}) AND isNotNull({noted}), \
                                   toJSONString(assumeNotNull({noted})), 'null'), '}}')"
        ),
    }
}

/// A span's carrier revisions summed into a fixed-size token: their count, and the sum of each revision's hash - its
/// identity, the instant it orders by, its receipt, and a hash of its messages (`messages_hash`), so the token moves
/// with any change to what the span joins, a same-instant redelivery's included. Two reads of one span that agree on
/// both read the same messages in the same order, so what one admitted the other answers; all of it is read from
/// narrow columns, never a record's messages or raw form.
const CLICKHOUSE_CARRIER_TOKEN: &str = "toUInt64(count()) AS carriers, \
     sum(cityHash64(log_digest, ordinal, toInt64(toUnixTimestamp64Micro(timestamp)), \
                    toInt64(toUnixTimestamp64Micro(ingested_at)), messages_hash)) AS carrier_token";

/// ClickHouse's aggregate of a span's log records' messages as the store answers them (`LogMessageSource`), typed
/// rather than JSON text, so no record's raw form is escaped to be read: an array of (messages, raw record, scope,
/// noted references) in `(timestamp, log_digest, ordinal)` order, the last three only where the messages are not
/// stored, the first only where they are.
const CLICKHOUSE_LOG_MESSAGE_SOURCES: &str = "arrayMap(entry -> (entry.4, entry.5, entry.6, entry.7), \
     arraySort(entry -> (entry.1, entry.2, entry.3), groupArray((timestamp, log_digest, ordinal, messages, \
         if(isNull(messages), raw_log, NULL), if(isNull(messages), scope_name, NULL), \
         if(isNull(messages), noted_references, NULL)))))";

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
    // An unmatched `LEFT JOIN` row is NULL on DuckDB, and on ClickHouse the type's default or NULL as
    // `join_use_nulls` says - so each value is normalised to the default, and an array cannot be NULL there.
    let log_messages = match backend {
        Backend::Duckdb => {
            "COALESCE(l.aggregated, '[]') AS log_messages, l.aggregated IS NOT NULL AS has_log_messages"
        }
        // The carrier token a feed page checks its messages by (`CLICKHOUSE_CARRIER_TOKEN`).
        Backend::Clickhouse => {
            "l.aggregated AS log_messages, notEmpty(l.aggregated) AS has_log_messages, \
             toUInt64(ifNull(l.carriers, 0)) AS log_carriers, \
             toUInt64(ifNull(l.carrier_token, 0)) AS log_carrier_token"
        }
    };
    let filter = filter.map(|f| format!(" WHERE {f}")).unwrap_or_default();
    let limit = limit.map(|n| format!("\nLIMIT {n}")).unwrap_or_default();
    ParameterizedQuery::new(
        format!(
            "SELECT * FROM (\n\
             SELECT {columns}, {log_messages}\n\
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

/// A feed page's own conditions on the spans, after the winner source's, with their values in that order: the
/// project, the cursor and the time bounds.
fn feed_conditions(
    params: &FeedMessagesParams,
    backend: Backend,
) -> (Vec<String>, Vec<QueryValue>) {
    let mut values = Vec::new();
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

    (conditions, values)
}

/// Read one stable project-wide page of message-bearing spans.
///
/// The page is ordered and cut by the span's own `ingested_at`, so a log record arriving after its span was
/// paged does not bring the span back: the next traversal sees it, this one does not.
pub fn get_project_messages(params: &FeedMessagesParams, backend: Backend) -> ParameterizedQuery {
    let (source, mut values) = winner_source(backend, params.ingested_before_us);
    let (conditions, condition_values) = feed_conditions(params, backend);
    values.extend(condition_values);

    if backend == Backend::Duckdb {
        return duckdb_project_messages(params, &conditions.join(" AND "), values);
    }
    clickhouse_project_messages_page(params, &source, &conditions.join(" AND "), values)
}

/// ClickHouse's project feed page, cut before any log record's messages are read: the spans, each with the token of
/// the winning revisions of the log records that carry messages for it - read from the carriers' keys alone, under
/// the watermark, a fixed size per span - filtered, ordered and limited. The page's messages are then read for its
/// spans alone ([`log_message_sources_of_spans`]), with the same token, so the read follows the page rather than
/// the project, and a caller accepts it only where the tokens agree. Its rows carry an empty `log_messages`, which
/// that second read fills.
fn clickhouse_project_messages_page(
    params: &FeedMessagesParams,
    source: &str,
    conditions: &str,
    span_values: Vec<QueryValue>,
) -> ParameterizedQuery {
    let (carriers, carrier_values) = log_message_carriers_on(Backend::Clickhouse, params);
    let columns = MESSAGE_COLUMNS
        .iter()
        .map(|column| format!("s.{column} AS {column}"))
        .collect::<Vec<_>>()
        .join(", ");
    // In text order: the spans' source, then the carriers'.
    let mut values = span_values;
    values.extend(carrier_values);
    ParameterizedQuery::new(
        format!(
            "SELECT * FROM (\n\
             SELECT {columns}, {empty} AS log_messages, ifNull(c.carriers, 0) > 0 AS has_log_messages, \
                    toUInt64(ifNull(c.carriers, 0)) AS log_carriers, \
                    toUInt64(ifNull(c.carrier_token, 0)) AS log_carrier_token\n\
             FROM (SELECT\n{projection}\nFROM {source}\nWHERE {conditions}) s\n\
             LEFT JOIN ({carriers}) c ON c.log_trace_id = s.trace_id AND c.log_span_id = s.span_id\n\
             ) WHERE {MESSAGE_CONTENT_FILTER}\n\
             ORDER BY ingested_at_us DESC, span_id DESC, trace_id DESC\nLIMIT {limit}",
            empty = CLICKHOUSE_NO_LOG_MESSAGE_SOURCES,
            projection = message_projection(Backend::Clickhouse),
            limit = params.limit,
        ),
        values,
    )
}

/// The ClickHouse project feed in one statement: every candidate span joined to its log records' messages, then
/// filtered and cut. Its read follows the project, so it is the fallback a feed read takes only where log records
/// arriving between the page and its messages keep the two from agreeing ([`clickhouse_project_messages_page`]).
pub fn clickhouse_project_messages_joined(params: &FeedMessagesParams) -> ParameterizedQuery {
    let (source, mut values) = winner_source(Backend::Clickhouse, params.ingested_before_us);
    let (conditions, condition_values) = feed_conditions(params, Backend::Clickhouse);
    values.extend(condition_values);
    join_log_messages(
        Backend::Clickhouse,
        (
            format!(
                "SELECT\n{}\nFROM {source}\nWHERE {}",
                message_projection(Backend::Clickhouse),
                conditions.join(" AND "),
            ),
            values,
        ),
        log_messages_source(
            Backend::Clickhouse,
            params.project_id.as_str(),
            params.ingested_before_us,
            None,
        ),
        Some(MESSAGE_CONTENT_FILTER),
        "ingested_at_us DESC, span_id DESC, trace_id DESC",
        Some(params.limit),
    )
}

/// An empty aggregate of [`CLICKHOUSE_LOG_MESSAGE_SOURCES`]'s type.
const CLICKHOUSE_NO_LOG_MESSAGE_SOURCES: &str = "CAST([], 'Array(Tuple(Nullable(String), Nullable(String), Nullable(String), Nullable(String)))')";

/// The log records' messages of these spans, one row per span with its carrier token: what a ClickHouse feed page
/// joins after it is cut ([`clickhouse_project_messages_page`]). Read by the trace ids' bloom filter, then the spans
/// exactly, each identity's winning delivery under the watermark, as the page read them.
pub fn log_message_sources_of_spans(
    project_id: &str,
    watermark: Option<i64>,
    spans: &[(String, String)],
) -> Option<ParameterizedQuery> {
    if spans.is_empty() {
        return None;
    }
    let mut traces: Vec<&str> = spans.iter().map(|(trace, _)| trace.as_str()).collect();
    traces.sort_unstable();
    traces.dedup();
    let mut values: Vec<QueryValue> = traces
        .iter()
        .map(|trace| QueryValue::String((*trace).to_string()))
        .collect();
    for (trace, span) in spans {
        values.push(QueryValue::String(trace.clone()));
        values.push(QueryValue::String(span.clone()));
    }
    let scope = format!(
        "trace_id IN ({}) AND (trace_id, span_id) IN ({})",
        std::iter::repeat_n("?", traces.len())
            .collect::<Vec<_>>()
            .join(", "),
        std::iter::repeat_n("(?, ?)", spans.len())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let (sql, aggregate_values) = log_messages_source(
        Backend::Clickhouse,
        project_id,
        watermark,
        Some((scope, values)),
    );
    Some(ParameterizedQuery::new(sql, aggregate_values))
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
         SELECT {columns}, COALESCE(l.aggregated, '[]') AS log_messages, \
                l.aggregated IS NOT NULL AS has_log_messages\n\
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
    log_message_carriers_on(Backend::Duckdb, params)
}

/// The spans log records carry messages for, by their keys alone: no record's messages or raw form is read. Each
/// identity's winning delivery decides, under the watermark, as the log aggregate decides
/// ([`log_messages_source`]).
fn log_message_carriers_on(
    backend: Backend,
    params: &FeedMessagesParams,
) -> (String, Vec<QueryValue>) {
    let mut values = vec![QueryValue::String(params.project_id.to_string())];
    let mut conditions = vec![
        "project_id = ?",
        "trace_id IS NOT NULL",
        "span_id IS NOT NULL",
        "messages_bytes > 0",
    ];
    if let Some(watermark) = params.ingested_before_us {
        conditions.push(match backend {
            Backend::Duckdb => "EPOCH_US(ingested_at) < ?::BIGINT",
            Backend::Clickhouse => "toInt64(toUnixTimestamp64Micro(ingested_at)) < ?",
        });
        values.push(QueryValue::Int64(watermark));
    }
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT DISTINCT trace_id, span_id FROM otel_logs WHERE {}",
            conditions.join(" AND ")
        ),
        // Each span's carrier token, by the carriers' keys alone, which the page's messages are checked by.
        Backend::Clickhouse => format!(
            "SELECT assumeNotNull(trace_id) AS log_trace_id, assumeNotNull(span_id) AS log_span_id, \
                    {CLICKHOUSE_CARRIER_TOKEN} \
             FROM otel_logs FINAL WHERE {} GROUP BY log_trace_id, log_span_id",
            conditions.join(" AND ")
        ),
    };
    (sql, values)
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
