//! Query repository for OTEL API queries.

use chrono::{DateTime, Utc};
use duckdb::{Connection, Row};

use crate::{DuckdbError, in_transaction};
use sideseat_core::utils::time::{micros_to_datetime, parse_iso_timestamp};
use sideseat_ports::types::{
    EventRow, FeedSpansParams, LinkRow, ListSessionsParams, ListSpansParams, ListTracesParams,
    SessionRow, SpanRow, TraceRow, parse_tags,
};
use sideseat_query_sql::confirmations;
use sideseat_query_sql::{Backend, analytics, dml};

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
///
/// `QUALIFY ROW_NUMBER()`, not a join on `MAX(ingested_at)`: the join returned **every** row tied at the
/// maximum, so two deliveries of one span landing in the same stored microsecond both survived and the span
/// appeared twice - a duplicate, which is the one thing the feed must never produce. `ROW_NUMBER() … = 1`
/// keeps exactly one row per span.
///
/// The tiebreak is `rowid DESC`, so on an equal `ingested_at` the **later insert wins** - which is
/// "latest delivery wins" for a same-microsecond re-delivery. `ingested_at` alone left the choice to the
/// engine, and it could keep the older row, so a re-delivery correcting a span's tokens or messages was
/// silently ignored. `rowid` is DuckDB's physical insert order on an append-only table, so a later delivery
/// always has the higher one. (`ingested_at` is assigned per backend at write time, so a cross-backend tie
/// does not arise in practice; ClickHouse breaks a same-microsecond tie by `ReplacingMergeTree` insert order,
/// which is the same "later insert wins" intent - neither distinguishes sub-microsecond recency, an inherent
/// limit of using a microsecond timestamp as the version.)
#[cfg(test)]
pub(crate) const DEDUP_SPANS: &str = "(SELECT * FROM otel_spans \
     QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
     ORDER BY ingested_at DESC, rowid DESC) = 1)";

/// The expression a trace list row *displays* for a filterable column, when that value is an
/// aggregate over the trace's spans rather than a column of one span.
///
/// `None` means the column is a plain span attribute (a model, an environment, a session id),
/// where "the trace has a span with this value" is the honest reading.
///
/// Aliases are those of [`trace_filter_subquery`]: `n` for the span rows, `gtf` for the totals.
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
    let page = analytics::list_traces(params, Backend::Duckdb);
    let total = execute_count_values(conn, &page.count)?;
    let rows = execute_trace_query_values(conn, &page.rows)?;
    Ok((rows, total))
}

/// Get a single trace by ID
pub fn get_trace(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
) -> Result<Option<TraceRow>, DuckdbError> {
    let query = analytics::trace_by_id(project_id, trace_id, Backend::Duckdb);
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;
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
    let page = analytics::list_spans(params, Backend::Duckdb);
    let total = execute_count_values(conn, &page.count)?;
    let rows = execute_span_query_values(conn, &page.rows)?;

    Ok((rows, total))
}

/// Get spans for feed with cursor-based pagination.
pub fn get_feed_spans(
    conn: &Connection,
    params: &FeedSpansParams,
) -> Result<Vec<SpanRow>, DuckdbError> {
    let query = analytics::feed_spans(params, Backend::Duckdb);
    execute_span_query_values(conn, &query)
}

/// Get spans for a trace (for trace detail view)
pub fn get_spans_for_trace(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
) -> Result<Vec<SpanRow>, DuckdbError> {
    let query = analytics::spans_for_trace(project_id, trace_id, Backend::Duckdb);
    execute_span_query_values(conn, &query)
}

/// Get a single span by trace_id and span_id
pub fn get_span(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Option<SpanRow>, DuckdbError> {
    let query = analytics::span_by_id().render(Backend::Duckdb);
    let mut stmt = conn.prepare(query.sql())?;
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
    let query = analytics::events_for_span(project_id, trace_id, span_id, Backend::Duckdb);
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut query_rows = stmt.query(values.as_slice())?;
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
    let query = analytics::links_for_span(project_id, trace_id, span_id, Backend::Duckdb);
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut query_rows = stmt.query(values.as_slice())?;
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

/// List sessions with pagination and filters.
pub fn list_sessions(
    conn: &Connection,
    params: &ListSessionsParams,
) -> Result<(Vec<SessionRow>, u64), DuckdbError> {
    let page = analytics::list_sessions(params, Backend::Duckdb);
    let total = execute_count_values(conn, &page.count)?;
    let rows = execute_session_query_values(conn, &page.rows)?;
    Ok((rows, total))
}

/// Get a single session by ID
pub fn get_session(
    conn: &Connection,
    project_id: &str,
    session_id: &str,
) -> Result<Option<SessionRow>, DuckdbError> {
    let query = analytics::session_by_id(project_id, session_id, Backend::Duckdb);
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;
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
    let query = analytics::traces_for_session(project_id, session_id, Backend::Duckdb);
    execute_trace_query_values(conn, &query)
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

    let Some(query) = analytics::span_counts_bulk(project_id, spans, Backend::Duckdb) else {
        return Ok(HashMap::new());
    };

    let mut counts: HashMap<(String, String), SpanCounts> = HashMap::with_capacity(spans.len());
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;

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

fn duckdb_values(values: &[analytics::QueryValue]) -> Vec<&dyn duckdb::ToSql> {
    values
        .iter()
        .map(|value| match value {
            analytics::QueryValue::String(value) => value as &dyn duckdb::ToSql,
            analytics::QueryValue::Int64(value) => value as &dyn duckdb::ToSql,
            analytics::QueryValue::Float64(value) => value as &dyn duckdb::ToSql,
        })
        .collect()
}

fn execute_count_values(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<u64, DuckdbError> {
    let values = duckdb_values(query.params());
    let count: i64 = conn.query_row(query.sql(), values.as_slice(), |row| row.get(0))?;
    Ok(count as u64)
}

pub fn spans_match_content(
    conn: &Connection,
    project_id: &str,
    records: &[(String, String, String)],
) -> Result<bool, DuckdbError> {
    let Some(plan) = confirmations::spans(project_id, records, Backend::Duckdb) else {
        return Ok(true);
    };
    let values = duckdb_values(plan.query.params());
    let found: i64 = conn.query_row(plan.query.sql(), values.as_slice(), |row| row.get(0))?;
    Ok(found as u64 == plan.expected)
}

fn execute_trace_query_values(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<TraceRow>, DuckdbError> {
    let mut stmt = conn.prepare(query.sql())?;
    let values = duckdb_values(query.params());
    let mut query_rows = stmt.query(values.as_slice())?;
    let mut rows = Vec::new();
    while let Some(row) = query_rows.next()? {
        rows.push(row_to_trace(row)?);
    }
    Ok(rows)
}

fn execute_span_query_values(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<SpanRow>, DuckdbError> {
    let mut stmt = conn.prepare(query.sql())?;
    let values = duckdb_values(query.params());
    let mut query_rows = stmt.query(values.as_slice())?;
    let mut rows = Vec::new();
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
        scope_name: row.get(39)?,
        scope_version: row.get(40)?,
    })
}

fn execute_session_query_values(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<SessionRow>, DuckdbError> {
    let mut stmt = conn.prepare(query.sql())?;
    let values = duckdb_values(query.params());
    let mut query_rows = stmt.query(values.as_slice())?;
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
/// Which of these traces have no winning spans left.
///
/// `DEDUP_SPANS`, so an obsolete revision does not make a deleted trace look alive.
pub fn traces_without_spans(
    conn: &Connection,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<String>, DuckdbError> {
    let Some(query) = analytics::surviving_trace_ids(project_id, trace_ids, Backend::Duckdb) else {
        return Ok(Vec::new());
    };
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;
    let mut alive: Vec<String> = Vec::new();
    while let Some(row) = rows.next()? {
        alive.push(row.get(0)?);
    }
    Ok(trace_ids
        .iter()
        .filter(|t| !alive.contains(t))
        .cloned()
        .collect())
}

/// The text of every field that can hold a `#!B64!#` reference, for the surviving winning spans of these
/// traces.
///
/// `DEDUP_SPANS`, not the raw table: an expired revision's text is not evidence that a *live* span still
/// references a file, and `otel_spans` is append-only so the obsolete rows are still there. Reading raw would
/// keep an association alive on the strength of a superseded revision - the mirror of the defect that made
/// retention delete a current span because an old revision of it had expired.
pub fn file_reference_fields_for_traces(
    conn: &Connection,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<String>, DuckdbError> {
    let Some(query) = analytics::file_reference_fields(project_id, trace_ids, Backend::Duckdb)
    else {
        return Ok(Vec::new());
    };
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;

    let mut fields = Vec::new();
    while let Some(row) = rows.next()? {
        for index in 0..4 {
            if let Ok(Some(text)) = row.get::<_, Option<String>>(index) {
                fields.push(text);
            }
        }
    }
    Ok(fields)
}

pub fn span_body_fields_for_traces(
    conn: &Connection,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<sideseat_ports::types::SpanBodySource>, DuckdbError> {
    let Some(query) = analytics::span_body_fields(project_id, trace_ids, Backend::Duckdb) else {
        return Ok(Vec::new());
    };
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;
    let mut sources = Vec::new();
    while let Some(row) = rows.next()? {
        sources.push(sideseat_ports::types::SpanBodySource {
            trace_id: row.get(0)?,
            span_id: row.get(1)?,
            messages: row.get(2)?,
            tool_definitions: row.get(3)?,
            tool_names: row.get(4)?,
            raw_span: row.get(5)?,
        });
    }
    Ok(sources)
}

pub fn span_body_backfill_page(
    conn: &Connection,
    project_id: &str,
    after: Option<(String, String)>,
    limit: usize,
) -> Result<Vec<sideseat_ports::types::SpanBodySource>, DuckdbError> {
    let query = analytics::span_body_backfill_page(
        project_id,
        after
            .as_ref()
            .map(|(trace, span)| (trace.as_str(), span.as_str())),
        limit,
        Backend::Duckdb,
    );
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;
    let mut sources = Vec::new();
    while let Some(row) = rows.next()? {
        sources.push(sideseat_ports::types::SpanBodySource {
            trace_id: row.get(0)?,
            span_id: row.get(1)?,
            messages: row.get(2)?,
            tool_definitions: row.get(3)?,
            tool_names: row.get(4)?,
            raw_span: row.get(5)?,
        });
    }
    Ok(sources)
}

pub fn delete_traces(
    conn: &Connection,
    project_id: &str,
    trace_ids: &[String],
) -> Result<u64, DuckdbError> {
    let Some(query) = dml::delete_traces(
        dml::MutationTarget::duckdb("otel_spans"),
        project_id,
        trace_ids,
    ) else {
        return Ok(0);
    };

    in_transaction(conn, |conn| {
        super::search::delete_for_traces(conn, project_id, trace_ids)?;
        let values = duckdb_values(query.params());
        let deleted = conn.execute(query.sql(), values.as_slice())?;
        let logs = dml::delete_logs_for_traces(
            dml::MutationTarget::duckdb("otel_logs"),
            project_id,
            trace_ids,
        )
        .expect("non-empty trace set produces a log delete");
        let values = duckdb_values(logs.params());
        conn.execute(logs.sql(), values.as_slice())?;
        Ok(deleted as u64)
    })
}

/// Get trace_ids for given session_ids
/// The distinct sessions the given traces belong to.
/// Which session each of the given traces belongs to; traces with none are absent.
///
/// `DEDUP_SPANS` for the same reason as [`get_session_ids_for_traces`]: the append-only table keeps both
/// versions of a span whose session changed, so a raw read reports a trace in two sessions at once.
/// The session is the one on the trace's **earliest** span, which is exactly what the trace and session
/// views display (`FIRST(session_id ORDER BY timestamp_start)`). `MIN(session_id)` was deterministic but
/// picked the lexicographically smallest instead, so a trace that started in `z-session` and had a later
/// span report `a-session` was displayed under one and *grouped* under the other - splitting a conversation
/// and replaying its shared history as duplicates. `span_id` breaks a timestamp tie, so the answer cannot
/// depend on row order.
pub fn get_trace_session_pairs(
    conn: &Connection,
    project_id: &str,
    trace_ids: &[String],
    as_of_us: Option<i64>,
) -> Result<Vec<(String, String)>, DuckdbError> {
    let Some(query) =
        analytics::trace_session_pairs(project_id, trace_ids, as_of_us, Backend::Duckdb)
    else {
        return Ok(vec![]);
    };
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;

    let mut pairs: Vec<(String, String)> = vec![];
    while let Some(row) = rows.next()? {
        pairs.push((row.get(0)?, row.get(1)?));
    }
    pairs.sort();
    Ok(pairs)
}

pub fn get_session_ids_for_traces(
    conn: &Connection,
    project_id: &str,
    trace_ids: &[String],
    as_of_us: Option<i64>,
) -> Result<Vec<String>, DuckdbError> {
    let Some(query) =
        analytics::session_ids_for_traces(project_id, trace_ids, as_of_us, Backend::Duckdb)
    else {
        return Ok(vec![]);
    };
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;

    let mut session_ids: Vec<String> = vec![];
    while let Some(row) = rows.next()? {
        session_ids.push(row.get(0)?);
    }
    session_ids.sort();
    session_ids.dedup();
    Ok(session_ids)
}

pub fn get_trace_ids_for_sessions(
    conn: &Connection,
    project_id: &str,
    session_ids: &[String],
    as_of_us: Option<i64>,
) -> Result<Vec<String>, DuckdbError> {
    let Some(query) =
        analytics::trace_ids_for_sessions(project_id, session_ids, as_of_us, Backend::Duckdb)
    else {
        return Ok(vec![]);
    };
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;

    let mut trace_ids: Vec<String> = vec![];
    while let Some(row) = rows.next()? {
        trace_ids.push(row.get(0)?);
    }

    Ok(trace_ids)
}

/// Delete multiple sessions by deleting all traces with those session_ids
/// Delete every trace of these sessions, returning the trace ids removed.
///
/// The ids, not a row count: the caller tombstones and reclaims files for exactly this set, and it is a
/// superset of whatever the caller resolved before calling - see the trait.
pub fn delete_sessions(
    conn: &Connection,
    project_id: &str,
    session_ids: &[String],
) -> Result<Vec<String>, DuckdbError> {
    // `None`: a deletion acts on what exists *now*. Bounding it to some past instant would leave a trace
    // that joined the session since, which the caller was told 204 for.
    let trace_ids = get_trace_ids_for_sessions(conn, project_id, session_ids, None)?;
    if trace_ids.is_empty() {
        return Ok(vec![]);
    }
    let statement = dml::delete_session_traces(
        dml::MutationTarget::duckdb("otel_spans"),
        project_id,
        &trace_ids,
    )
    .expect("non-empty trace set produces a delete");
    in_transaction(conn, |conn| {
        let values = duckdb_values(statement.params());
        conn.execute(statement.sql(), values.as_slice())?;
        let logs = dml::delete_logs_for_traces(
            dml::MutationTarget::duckdb("otel_logs"),
            project_id,
            &trace_ids,
        )
        .expect("non-empty trace set produces a log delete");
        let values = duckdb_values(logs.params());
        conn.execute(logs.sql(), values.as_slice())?;
        Ok(())
    })?;
    Ok(trace_ids)
}

/// Delete specific spans by (trace_id, span_id) pairs
pub fn delete_spans(
    conn: &Connection,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<u64, DuckdbError> {
    let Some(query) =
        dml::delete_spans(dml::MutationTarget::duckdb("otel_spans"), project_id, spans)
    else {
        return Ok(0);
    };

    in_transaction(conn, |conn| {
        super::search::delete_for_spans(conn, project_id, spans)?;
        let values = duckdb_values(query.params());
        let deleted = conn.execute(query.sql(), values.as_slice())?;
        let logs =
            dml::delete_logs_for_spans(dml::MutationTarget::duckdb("otel_logs"), project_id, spans)
                .expect("non-empty span set produces a log delete");
        let values = duckdb_values(logs.params());
        conn.execute(logs.sql(), values.as_slice())?;
        Ok(deleted as u64)
    })
}

/// Delete all OTEL data for a project: spans and metrics both.
///
/// Metrics were left behind, which the ClickHouse twin has always deleted - so the same deletion left
/// different residue depending on the backend, and on DuckDB a project's metrics outlived the project
/// itself with nothing able to reach them. One transaction, so a project's analytics data goes or
/// stays as a whole. The returned count is spans, which is what the caller reports.
pub fn delete_project_data(conn: &Connection, project_id: &str) -> Result<u64, DuckdbError> {
    let plan = dml::delete_project_data(
        dml::MutationTarget::duckdb("otel_spans"),
        dml::MutationTarget::duckdb("otel_metrics"),
        dml::MutationTarget::duckdb("otel_logs"),
        project_id,
    );
    in_transaction(conn, |conn| {
        super::search::delete_for_project(conn, project_id)?;
        let span_values = duckdb_values(plan.delete_spans.params());
        let deleted = conn.execute(plan.delete_spans.sql(), span_values.as_slice())?;
        let metric_values = duckdb_values(plan.delete_metrics.params());
        conn.execute(plan.delete_metrics.sql(), metric_values.as_slice())?;
        let log_values = duckdb_values(plan.delete_logs.params());
        conn.execute(plan.delete_logs.sql(), log_values.as_slice())?;
        Ok(deleted as u64)
    })
}

/// Count every row a project still owns, spans and metrics together.
///
/// Deletion verification reads this: "the data is gone" has to mean all of it, and metrics live in their
/// own table.
pub fn count_project_rows(conn: &Connection, project_id: &str) -> Result<u64, DuckdbError> {
    let plan = analytics::project_row_count(project_id, Backend::Duckdb, None);
    let spans = execute_count_values(conn, &plan.spans)?;
    // Rows, plainly. The write path replaces a datapoint's row rather than appending a second one
    // (`replace_existing`), so a row *is* a datapoint and this agrees with ClickHouse's `FINAL` count
    // without having to compensate for duplicates at read time.
    //
    // It used to be `COUNT(DISTINCT datapoint_id)`, which hid physical duplicates instead of preventing
    // them: two rows for one datapoint stayed, holding two possibly different measurements of the same
    // instant with nothing to say which was current. Deduplicating a count is not storing correct data.
    let metrics = execute_count_values(conn, &plan.metrics)?;
    let logs = execute_count_values(conn, &plan.logs)?;
    Ok(spans + metrics + logs)
}

pub fn patch_project_hold(
    conn: &Connection,
    project_id: &str,
    hold_until: chrono::DateTime<Utc>,
) -> Result<(), DuckdbError> {
    let statements = dml::patch_project_hold(
        dml::MutationTarget::duckdb("otel_spans"),
        dml::MutationTarget::duckdb("otel_metrics"),
        dml::MutationTarget::duckdb("otel_logs"),
        project_id,
        hold_until,
    );
    in_transaction(conn, |conn| {
        for statement in &statements {
            let values = duckdb_values(statement.params());
            conn.execute(statement.sql(), values.as_slice())?;
        }
        Ok(())
    })
}

pub fn project_logical_bytes(
    conn: &Connection,
    project_id: &str,
    held_at: Option<chrono::DateTime<Utc>>,
) -> Result<u64, DuckdbError> {
    let plan = analytics::project_logical_bytes(project_id, Backend::Duckdb, held_at);
    Ok(execute_count_values(conn, &plan.spans)?
        + execute_count_values(conn, &plan.metrics)?
        + execute_count_values(conn, &plan.logs)?)
}

pub fn oldest_reclaimable_spans(
    conn: &Connection,
    project_id: &str,
    target_bytes: u64,
    now: chrono::DateTime<Utc>,
    limit: usize,
) -> Result<Vec<sideseat_ports::types::PressureSpanCandidate>, DuckdbError> {
    let statement =
        analytics::oldest_reclaimable_spans(project_id, Backend::Duckdb, target_bytes, now, limit);
    let values = duckdb_values(statement.params());
    let mut prepared = conn.prepare(statement.sql())?;
    let rows = prepared.query_map(values.as_slice(), |row| {
        Ok(sideseat_ports::types::PressureSpanCandidate {
            trace_id: row.get(0)?,
            span_id: row.get(1)?,
            logical_bytes: row.get(2)?,
        })
    })?;
    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    Ok(candidates)
}

/// The newest committed ingestion time for a project, in microseconds. See the trait method.
pub fn max_ingested_at_us(conn: &Connection, project_id: &str) -> Result<Option<i64>, DuckdbError> {
    let query = analytics::max_ingested_at_us(project_id, Backend::Duckdb);
    let values = duckdb_values(query.params());
    let value: Option<i64> = conn.query_row(query.sql(), values.as_slice(), |row| row.get(0))?;
    Ok(value)
}

/// Count spans grouped by project for a set of project IDs.
pub fn count_spans_by_project(
    conn: &Connection,
    project_ids: &[String],
) -> Result<std::collections::HashMap<String, u64>, DuckdbError> {
    use std::collections::HashMap;

    let Some(query) = analytics::span_counts_by_project(project_ids, Backend::Duckdb) else {
        return Ok(HashMap::new());
    };
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;

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

fn execute_filter_option_query(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<FilterOptionRow>, DuckdbError> {
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;
    let mut options = Vec::new();
    while let Some(row) = rows.next()? {
        let value: Option<String> = row.get(0)?;
        let count: i64 = row.get(1)?;
        if let Some(value) = value {
            options.push(FilterOptionRow {
                value,
                count: count as u64,
            });
        }
    }
    Ok(options)
}

/// Get distinct values with counts for trace filter options.
pub fn get_trace_filter_options(
    conn: &Connection,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<std::collections::HashMap<String, Vec<FilterOptionRow>>, DuckdbError> {
    let mut results = std::collections::HashMap::new();
    for option in analytics::trace_filter_options(
        project_id,
        columns,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        Backend::Duckdb,
    ) {
        let rows = execute_filter_option_query(conn, &option.query)?;
        results.insert(option.column, rows);
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
    let query = analytics::trace_tag_options(
        project_id,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        Backend::Duckdb,
    );
    execute_filter_option_query(conn, &query)
}

/// Get distinct values with counts for span filter options.
pub fn get_span_filter_options(
    conn: &Connection,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
    observations_only: bool,
) -> Result<std::collections::HashMap<String, Vec<FilterOptionRow>>, DuckdbError> {
    let mut results = std::collections::HashMap::new();
    for option in analytics::span_filter_options(
        project_id,
        columns,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        observations_only,
        Backend::Duckdb,
    ) {
        let rows = execute_filter_option_query(conn, &option.query)?;
        results.insert(option.column, rows);
    }
    Ok(results)
}

/// Get distinct values with counts for session filter options.
pub fn get_session_filter_options(
    conn: &Connection,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<std::collections::HashMap<String, Vec<FilterOptionRow>>, DuckdbError> {
    let mut results = std::collections::HashMap::new();
    for option in analytics::session_filter_options(
        project_id,
        columns,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        Backend::Duckdb,
    ) {
        let rows = execute_filter_option_query(conn, &option.query)?;
        results.insert(option.column, rows);
    }
    Ok(results)
}

/// Repository-level regression tests.
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use sideseat_ports::types::ProjectId;

    // ============================================================================
    // Integration tests for leaf generation span filtering (cost deduplication)
    // ============================================================================

    use crate::models::ObservationType;
    use crate::repositories::span::insert_batch;
    use crate::{DuckdbService, NormalizedSpan};
    use sideseat_core::core::storage::AppStorage;
    use tempfile::TempDir;

    async fn create_test_service() -> (TempDir, DuckdbService) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let duckdb_dir = temp_dir.path().join("duckdb");
        tokio::fs::create_dir_all(&duckdb_dir)
            .await
            .expect("Failed to create duckdb dir");
        let storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
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

    #[tokio::test]
    async fn typed_deletes_preserve_tenant_and_identity_boundaries() {
        let (_tmp, service) = create_test_service().await;
        let spans = [
            make_agent_span("project-a", "shared-trace", "span-1", None),
            make_agent_span("project-a", "shared-trace", "span-2", None),
            make_agent_span("project-a", "other-trace", "span-1", None),
            make_agent_span("project-b", "shared-trace", "span-1", None),
        ];

        let conn = service.conn();
        insert_batch(&conn, &spans).expect("insert fixture");

        assert_eq!(
            delete_spans(
                &conn,
                "project-a",
                &[("shared-trace".to_string(), "span-1".to_string())],
            )
            .expect("delete one span"),
            1
        );
        assert_eq!(
            delete_traces(&conn, "project-a", &["other-trace".to_string()])
                .expect("delete one trace"),
            1
        );

        let remaining = |project: &str, trace: &str, span: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM otel_spans \
                 WHERE project_id = ? AND trace_id = ? AND span_id = ?",
                duckdb::params![project, trace, span],
                |row| row.get(0),
            )
            .expect("count remaining identity")
        };
        assert_eq!(remaining("project-a", "shared-trace", "span-1"), 0);
        assert_eq!(remaining("project-a", "shared-trace", "span-2"), 1);
        assert_eq!(remaining("project-a", "other-trace", "span-1"), 0);
        assert_eq!(
            remaining("project-b", "shared-trace", "span-1"),
            1,
            "colliding identity in another tenant must survive"
        );
    }

    #[tokio::test]
    async fn get_span_returns_the_latest_delivery() {
        let (_temp_dir, analytics) = create_test_service().await;
        let event_time =
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("valid timestamp");
        let first_ingest =
            DateTime::<Utc>::from_timestamp(1_700_000_001, 0).expect("valid timestamp");
        let second_ingest =
            DateTime::<Utc>::from_timestamp(1_700_000_002, 0).expect("valid timestamp");

        let first = NormalizedSpan {
            project_id: Some("test-project".to_string()),
            trace_id: "trace-point-read".to_string(),
            span_id: "span-point-read".to_string(),
            span_name: "old delivery".to_string(),
            timestamp_start: event_time,
            ingested_at: Some(first_ingest),
            ..Default::default()
        };
        let second = NormalizedSpan {
            span_name: "winning delivery".to_string(),
            ingested_at: Some(second_ingest),
            ..first.clone()
        };

        let conn = analytics.conn();
        insert_batch(&conn, &[first, second]).expect("insert both deliveries");
        let row = get_span(&conn, "test-project", "trace-point-read", "span-point-read")
            .expect("point read")
            .expect("span exists");

        assert_eq!(row.span_name.as_deref(), Some("winning delivery"));
        assert_eq!(row.ingested_at, second_ingest);

        let trace_rows =
            get_spans_for_trace(&conn, "test-project", "trace-point-read").expect("trace spans");
        assert_eq!(trace_rows.len(), 1, "one winning row per span identity");
        assert_eq!(trace_rows[0].span_name.as_deref(), Some("winning delivery"));
    }

    #[tokio::test]
    async fn bulk_span_counts_use_the_latest_delivery() {
        let (_temp_dir, analytics) = create_test_service().await;
        let event_time =
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("valid timestamp");
        let first_ingest =
            DateTime::<Utc>::from_timestamp(1_700_000_001, 0).expect("valid timestamp");
        let second_ingest =
            DateTime::<Utc>::from_timestamp(1_700_000_002, 0).expect("valid timestamp");
        let raw = |events: usize, links: usize| {
            serde_json::json!({
                "events": vec![serde_json::json!({"name": "event"}); events],
                "links": vec![serde_json::json!({"trace_id": "linked"}); links],
            })
            .to_string()
        };
        let first = NormalizedSpan {
            project_id: Some("test-project".to_string()),
            trace_id: "trace-counts".to_string(),
            span_id: "span-counts".to_string(),
            span_name: "first".to_string(),
            timestamp_start: event_time,
            ingested_at: Some(first_ingest),
            raw_span: Some(raw(3, 2)),
            ..Default::default()
        };
        let second = NormalizedSpan {
            span_name: "second".to_string(),
            ingested_at: Some(second_ingest),
            raw_span: Some(raw(1, 4)),
            ..first.clone()
        };

        let conn = analytics.conn();
        insert_batch(&conn, &[first, second]).expect("insert revisions");
        let counts = get_span_counts_bulk(
            &conn,
            "test-project",
            &[("trace-counts".to_string(), "span-counts".to_string())],
        )
        .expect("counts");
        let counts = counts
            .get(&("trace-counts".to_string(), "span-counts".to_string()))
            .expect("requested identity");
        assert_eq!(counts.event_count, 1);
        assert_eq!(counts.link_count, 4);
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

    #[tokio::test]
    async fn corrected_unbilled_child_reactivates_the_winning_parent() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-corrected-child";
        let first_ingest = Utc
            .with_ymd_and_hms(2026, 9, 22, 10, 0, 0)
            .single()
            .unwrap();
        let corrected_ingest = first_ingest + chrono::Duration::seconds(1);

        let mut parent = make_generation_span(project_id, trace_id, "parent", None, 0.40, 400);
        parent.ingested_at = Some(first_ingest);
        let mut child =
            make_generation_span(project_id, trace_id, "child", Some("parent"), 0.10, 100);
        child.ingested_at = Some(first_ingest);
        let corrected_child = NormalizedSpan {
            gen_ai_cost_total: 0.0,
            gen_ai_usage_input_tokens: 0,
            gen_ai_usage_output_tokens: 0,
            gen_ai_usage_total_tokens: 0,
            ingested_at: Some(corrected_ingest),
            ..child.clone()
        };

        let conn = analytics.conn();
        insert_batch(&conn, &[parent, child]).expect("insert billed parent and child");
        let before = get_trace(&conn, project_id, trace_id)
            .expect("query")
            .expect("trace");
        assert_eq!(
            before.total_tokens, 100,
            "the billed child suppresses its parent"
        );
        assert_eq!(before.total_cost, 0.10);

        insert_batch(&conn, &[corrected_child]).expect("insert corrected child delivery");
        let after = get_trace(&conn, project_id, trace_id)
            .expect("query")
            .expect("trace");
        assert_eq!(
            after.total_tokens, 400,
            "the losing billed delivery must not keep suppressing the parent"
        );
        assert_eq!(after.total_cost, 0.40);
    }

    #[tokio::test]
    async fn cost_only_generation_child_suppresses_its_parent() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-cost-only-child";
        let parent = make_generation_span(project_id, trace_id, "parent", None, 0.40, 400);
        let child = make_generation_span(project_id, trace_id, "child", Some("parent"), 0.10, 0);

        let conn = analytics.conn();
        insert_batch(&conn, &[parent, child]).expect("insert fixture");
        let trace = get_trace(&conn, project_id, trace_id)
            .expect("query")
            .expect("trace");
        assert_eq!(trace.total_tokens, 0);
        assert_eq!(
            trace.total_cost, 0.10,
            "cost without tokens is still billed and suppresses the parent"
        );
    }

    #[tokio::test]
    async fn deleting_a_billed_child_reactivates_its_parent() {
        let (_temp_dir, analytics) = create_test_service().await;
        let project_id = "test-project";
        let trace_id = "trace-deleted-child";
        let parent = make_generation_span(project_id, trace_id, "parent", None, 0.40, 400);
        let child = make_generation_span(project_id, trace_id, "child", Some("parent"), 0.10, 100);

        let conn = analytics.conn();
        insert_batch(&conn, &[parent, child]).expect("insert fixture");
        assert_eq!(
            get_trace(&conn, project_id, trace_id)
                .expect("query")
                .expect("trace")
                .total_tokens,
            100
        );

        delete_spans(
            &conn,
            project_id,
            &[(trace_id.to_string(), "child".to_string())],
        )
        .expect("delete child");
        let trace = get_trace(&conn, project_id, trace_id)
            .expect("query")
            .expect("trace");
        assert_eq!(trace.total_tokens, 400);
        assert_eq!(trace.total_cost, 0.40);
        let child_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans
                 WHERE project_id = ? AND trace_id = ? AND span_id = 'child'",
                duckdb::params![project_id, trace_id],
                |row| row.get(0),
            )
            .expect("count child rows");
        assert_eq!(child_rows, 0);
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
            project_id: ProjectId::from(project_id),
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
            project_id: ProjectId::from(project_id),
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
            project_id: ProjectId::from(project_id),
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
            project_id: ProjectId::from(project_id),
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
            project_id: ProjectId::from(project_id),
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
            project_id: ProjectId::from(project_id),
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
            project_id: ProjectId::from(project_id),
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
            project_id: ProjectId::from(project_id),
            limit: 2,
            ..Default::default()
        };
        let page1 = get_feed_spans(&conn, &params).expect("Query should succeed");
        assert_eq!(page1.len(), 2, "First page should have 2 spans");

        // Second page (with cursor from last span of page1)
        let last_span = page1.last().unwrap();
        let cursor_time_us = last_span.ingested_at.timestamp_micros();
        let params = FeedSpansParams {
            project_id: ProjectId::from(project_id),
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
            project_id: ProjectId::from("nonexistent-project"),
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

    use sideseat_ports::filters::{Filter, NullOp, NumberOp, OptionsOp, StringOp};

    fn trace_filter_params(project_id: &str, filters: Vec<Filter>) -> ListTracesParams {
        ListTracesParams {
            project_id: ProjectId::from(project_id),
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
            project_id: ProjectId::from(project_id),
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

    /// Session membership resolves at the traversal's instant, not at the current one.
    ///
    /// A feed traversal reads its rows as of a watermark so that it is a view of one instant. Membership was
    /// resolved against current data, so the two could disagree: a trace re-delivered into another session
    /// mid-traversal is read with its old content but expanded under its *new* session, so the session it
    /// actually replays is never loaded, its replayed history has nothing to collapse against, and the reader
    /// sees the turn twice across pages.
    #[tokio::test]
    async fn membership_is_resolved_as_of_the_traversal_watermark() {
        let (_tmp, service) = create_test_service().await;
        let project = "p";

        let span = |session: &str, ingested: chrono::DateTime<Utc>| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "trace-1".to_string(),
            span_id: "span-1".to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: Utc::now(),
            session_id: Some(session.to_string()),
            ingested_at: Some(ingested),
            ..Default::default()
        };

        let early = Utc::now() - chrono::Duration::seconds(60);
        let late = Utc::now();

        {
            let conn = service.conn();
            // Two deliveries of one span, the second moving it to another session.
            insert_batch(&conn, &[span("session-old", early)]).expect("first delivery");
            insert_batch(&conn, &[span("session-new", late)]).expect("re-delivery");
        }

        let as_of = early.timestamp_micros() + 1;
        let traces = &["trace-1".to_string()];

        let conn = service.conn();

        // Current membership: the re-delivery won, so the trace is in the new session.
        let now = get_trace_session_pairs(&conn, project, traces, None).expect("current");
        assert_eq!(
            now,
            vec![("trace-1".to_string(), "session-new".to_string())],
            "without a bound, the latest delivery decides membership"
        );

        // As of an instant before the re-delivery: the session the traversal is actually reading.
        let bounded =
            get_trace_session_pairs(&conn, project, traces, Some(as_of)).expect("bounded");
        assert_eq!(
            bounded,
            vec![("trace-1".to_string(), "session-old".to_string())],
            "a traversal must group the trace with the session its rows belong to"
        );

        // The same for both directions of the membership lookup.
        let sessions =
            get_session_ids_for_traces(&conn, project, traces, Some(as_of)).expect("sessions");
        assert_eq!(sessions, vec!["session-old".to_string()]);

        let old_traces =
            get_trace_ids_for_sessions(&conn, project, &["session-old".to_string()], Some(as_of))
                .expect("traces");
        assert_eq!(old_traces, vec!["trace-1".to_string()]);
        let new_traces =
            get_trace_ids_for_sessions(&conn, project, &["session-new".to_string()], Some(as_of))
                .expect("traces");
        assert!(
            new_traces.is_empty(),
            "the new session did not exist at the watermark"
        );
    }

    /// A session message query *with* a watermark binds two deduplicated relations in one statement.
    ///
    /// The session branch resolves membership against its own copy of the dedup relation, and the outer
    /// query against another - so with a watermark there are two `?` placeholders from two relations plus the
    /// condition binds, in an order the statement text decides. Get that order wrong and the query either
    /// fails or silently answers about the wrong instant, and no other test exercised the combination: the
    /// watermark tests all use the `trace_ids` branch and the session tests all run unbounded.
    #[tokio::test]
    async fn a_session_query_with_a_watermark_binds_in_the_right_order() {
        use crate::repositories::messages::get_messages;
        use sideseat_ports::types::MessageQueryParams;

        let (_tmp, service) = create_test_service().await;
        let project = "p";

        let payload = |text: &str| {
            Some(
                serde_json::json!([{
                    "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
                    "content": {"role": "user", "content": text}
                }])
                .to_string(),
            )
        };

        let early = Utc::now() - chrono::Duration::seconds(60);
        let late = Utc::now();

        let span = |session: &str, text: &str, ingested: chrono::DateTime<Utc>| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "trace-1".to_string(),
            span_id: "span-1".to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: early,
            session_id: Some(session.to_string()),
            messages: payload(text),
            ingested_at: Some(ingested),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(&conn, &[span("session-old", "the first answer", early)]).expect("first");
            insert_batch(&conn, &[span("session-new", "the corrected answer", late)])
                .expect("re-delivery");
        }

        let conn = service.conn();
        let as_of = early.timestamp_micros() + 1;

        // As of before the re-delivery: the trace is in the old session and carries the old content.
        let bounded = get_messages(
            &conn,
            &MessageQueryParams {
                project_id: ProjectId::from(project),
                session_id: Some("session-old".to_string()),
                ingested_before_us: Some(as_of),
                ..Default::default()
            },
        )
        .expect("the bounded session query must execute");
        assert_eq!(
            bounded.rows.len(),
            1,
            "the trace belonged to this session then"
        );
        assert!(
            bounded.rows[0].messages_json.contains("the first answer"),
            "membership and content must describe the same instant, got {}",
            bounded.rows[0].messages_json
        );

        // The session it moved *to* did not exist at that instant.
        let future_session = get_messages(
            &conn,
            &MessageQueryParams {
                project_id: ProjectId::from(project),
                session_id: Some("session-new".to_string()),
                ingested_before_us: Some(as_of),
                ..Default::default()
            },
        )
        .expect("query");
        assert!(
            future_session.rows.is_empty(),
            "a session created after the watermark must not appear in the traversal"
        );

        // Unbounded, the re-delivery wins in both membership and content.
        let current = get_messages(
            &conn,
            &MessageQueryParams {
                project_id: ProjectId::from(project),
                session_id: Some("session-new".to_string()),
                ..Default::default()
            },
        )
        .expect("query");
        assert_eq!(current.rows.len(), 1);
        assert!(
            current.rows[0]
                .messages_json
                .contains("the corrected answer"),
            "the current read must return the corrected content"
        );
    }

    /// The session list reports each trace under exactly one session.
    ///
    /// A trace whose earliest span names A and whose child names B belongs to A everywhere else. The list
    /// selected distinct `session_id` from spans, so it returned **two** sessions and attributed that
    /// trace's full spans, tokens and cost to each - and opening B showed nothing, because every read
    /// resolves it to A.
    #[tokio::test]
    async fn the_session_list_reports_each_trace_under_one_session() {
        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let span = |span_id: &str, session: &str, offset: i64, tokens: i64| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "trace-1".to_string(),
            span_id: span_id.to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0 + chrono::Duration::seconds(offset),
            timestamp_end: Some(t0 + chrono::Duration::seconds(offset + 1)),
            session_id: Some(session.to_string()),
            environment: Some("prod".to_string()),
            gen_ai_usage_total_tokens: tokens,
            gen_ai_usage_input_tokens: tokens,
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(
                &conn,
                &[
                    span("span-1", "session-canonical", 0, 100),
                    span("span-2", "session-stray", 1, 50),
                ],
            )
            .expect("insert");
        }

        let conn = service.conn();
        let (sessions, total) = list_sessions(
            &conn,
            &ListSessionsParams {
                project_id: ProjectId::from(project),
                page: 1,
                limit: 50,
                ..Default::default()
            },
        )
        .expect("list_sessions");

        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["session-canonical"],
            "only the trace's canonical session is a session"
        );
        assert_eq!(
            total, 1,
            "and the count must agree with what the list can return"
        );
        assert_eq!(
            sessions[0].total_tokens, 150,
            "the whole trace's tokens belong to its one session, counted once"
        );

        // The filter-option suggestions agree too: a dropdown must not claim more sessions than the list
        // can return. This was the ninth surface of the same question.
        let options =
            get_session_filter_options(&conn, project, &["environment".to_string()], None, None)
                .expect("filter options");
        if let Some(env_counts) = options.get("environment") {
            for opt in env_counts {
                assert_eq!(
                    opt.count, 1,
                    "environment {:?} belongs to one session, not {}",
                    opt.value, opt.count
                );
            }
        }

        // And the dashboard agrees with the list. Parity between the backends cannot establish this - they
        // can agree on being wrong - so the number itself is pinned here.
        let stats = crate::repositories::stats::get_project_stats(
            &conn,
            &sideseat_ports::types::StatsParams {
                project_id: ProjectId::from(project),
                from_timestamp: t0 - chrono::Duration::seconds(10),
                to_timestamp: t0 + chrono::Duration::seconds(600),
                timezone: None,
            },
            t0 + chrono::Duration::seconds(600),
        )
        .expect("project stats");
        assert_eq!(
            stats.counts.sessions, 1,
            "project statistics must not count a session no trace canonically belongs to"
        );
    }

    /// Dropdown values describe the same winning entities and session-wide fields as the lists.
    #[tokio::test]
    async fn filter_options_use_winning_deliveries_and_session_wide_values() {
        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let old = NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "trace-revision".to_string(),
            span_id: "root".to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0,
            session_id: Some("session-revision".to_string()),
            environment: Some("old-environment".to_string()),
            tags: vec!["old-tag".to_string()],
            ingested_at: Some(t0),
            ..Default::default()
        };
        let new = NormalizedSpan {
            environment: Some("new-environment".to_string()),
            tags: vec!["new-tag".to_string()],
            ingested_at: Some(t0 + chrono::Duration::seconds(1)),
            ..old.clone()
        };
        let session_root = NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "trace-child-value".to_string(),
            span_id: "session-root".to_string(),
            span_name: "root".to_string(),
            timestamp_start: t0,
            session_id: Some("session-child-value".to_string()),
            ..Default::default()
        };
        let session_child = NormalizedSpan {
            span_id: "session-child".to_string(),
            parent_span_id: Some("session-root".to_string()),
            session_id: None,
            user_id: Some("child-user".to_string()),
            ..session_root.clone()
        };

        {
            let conn = service.conn();
            insert_batch(&conn, &[old, new, session_root, session_child]).expect("insert");
        }

        let conn = service.conn();
        let trace = get_trace_filter_options(
            &conn,
            project,
            &["environment".to_string(), "trace_name".to_string()],
            None,
            None,
        )
        .expect("trace options");
        let trace_values = &trace["environment"];
        assert!(
            trace_values
                .iter()
                .any(|row| row.value == "new-environment")
        );
        assert!(
            !trace_values
                .iter()
                .any(|row| row.value == "old-environment")
        );
        assert!(
            trace["trace_name"]
                .iter()
                .any(|row| row.value == "generation"),
            "the aggregate trace-name option query must execute and expose displayed names"
        );

        let span = get_span_filter_options(
            &conn,
            project,
            &["environment".to_string()],
            None,
            None,
            false,
        )
        .expect("span options");
        let span_values = &span["environment"];
        assert!(span_values.iter().any(|row| row.value == "new-environment"));
        assert!(!span_values.iter().any(|row| row.value == "old-environment"));

        let tags = get_trace_tags_options(&conn, project, None, None).expect("tag options");
        assert!(tags.iter().any(|row| row.value == "new-tag"));
        assert!(!tags.iter().any(|row| row.value == "old-tag"));

        let sessions =
            get_session_filter_options(&conn, project, &["user_id".to_string()], None, None)
                .expect("session options");
        assert_eq!(sessions["user_id"][0].value, "child-user");
        assert_eq!(sessions["user_id"][0].count, 1);
    }

    /// An *advanced* session filter agrees with the dedicated session parameter.
    ///
    /// Two ways of asking the same question lived in the same query: `params.session_id` went through the
    /// canonical relation while an advanced filter on `session_id` compared the span's own column. So
    /// filtering by a session that only a later span named returned that child, though every view displays
    /// its trace under a different session.
    #[tokio::test]
    async fn an_advanced_session_filter_agrees_with_the_session_parameter() {
        use sideseat_ports::filters::{Filter, StringOp};

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let span = |span_id: &str, session: &str, offset: i64| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "trace-1".to_string(),
            span_id: span_id.to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0 + chrono::Duration::seconds(offset),
            timestamp_end: Some(t0 + chrono::Duration::seconds(offset + 1)),
            session_id: Some(session.to_string()),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(
                &conn,
                &[
                    span("span-1", "session-canonical", 0),
                    span("span-2", "session-stray", 1),
                ],
            )
            .expect("insert");
        }

        let conn = service.conn();
        let eq = |value: &str| Filter::String {
            column: "session_id".to_string(),
            operator: StringOp::Eq,
            value: value.to_string(),
        };

        for (session, expect) in [("session-canonical", 2usize), ("session-stray", 0)] {
            let spans = list_spans(
                &conn,
                &ListSpansParams {
                    project_id: ProjectId::from(project),
                    page: 1,
                    limit: 50,
                    filters: vec![eq(session)],
                    ..Default::default()
                },
            )
            .expect("list_spans");
            assert_eq!(
                spans.0.len(),
                expect,
                "span list, advanced filter session_id = {session}"
            );

            // And the dedicated parameter agrees, which is the whole point.
            let by_param = list_spans(
                &conn,
                &ListSpansParams {
                    project_id: ProjectId::from(project),
                    page: 1,
                    limit: 50,
                    session_id: Some(session.to_string()),
                    ..Default::default()
                },
            )
            .expect("list_spans");
            assert_eq!(
                spans.0.len(),
                by_param.0.len(),
                "the advanced filter and the session parameter must agree for {session}"
            );
        }
    }

    /// A negated session filter means the same thing in the trace list and in the span list.
    ///
    /// The trace list matched the *displayed* aggregate, so `session_id NOT IN ('a')` evaluated
    /// `trace_display_first(session_id) NOT IN ('a')`, which is NULL for a trace with no session at all -
    /// and NULL is not true, so the trace was absent from "none of a" while the span list, which negates the
    /// canonical subquery, returned its spans. Both routes are now the subquery.
    #[tokio::test]
    async fn a_negated_session_filter_agrees_between_the_trace_and_span_lists() {
        use sideseat_ports::filters::{Filter, OptionsOp};

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let span = |trace: &str, span_id: &str, session: Option<&str>| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: trace.to_string(),
            span_id: span_id.to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0,
            timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
            session_id: session.map(str::to_string),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(
                &conn,
                &[
                    span("t-in-a", "s1", Some("session-a")),
                    span("t-sessionless", "s2", None),
                ],
            )
            .expect("insert");
        }

        let conn = service.conn();
        let none_of_a = Filter::StringOptions {
            column: "session_id".to_string(),
            operator: OptionsOp::NoneOf,
            value: vec!["session-a".to_string()],
        };

        let (traces, total) = list_traces(
            &conn,
            &trace_filter_params(project, vec![none_of_a.clone()]),
        )
        .expect("list_traces");
        let ids: Vec<&str> = traces.iter().map(|t| t.trace_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["t-sessionless"],
            "a trace with no session is not in session A, so it matches \"none of A\""
        );
        assert_eq!(total, 1, "the count must agree with the page");

        let (spans, _) = list_spans(
            &conn,
            &ListSpansParams {
                project_id: ProjectId::from(project),
                page: 1,
                limit: 50,
                filters: vec![none_of_a],
                ..Default::default()
            },
        )
        .expect("list_spans");
        let span_traces: Vec<&str> = spans.iter().map(|s| s.trace_id.as_str()).collect();
        assert_eq!(
            span_traces, ids,
            "the two lists must select the same traces for one filter"
        );
    }

    /// The name a trace displays is the same in the list, in the detail view and to a filter.
    ///
    /// Two roots stamped with the same start instant is not exotic - a millisecond-resolution clock and two
    /// spans opened together produce it. Ordering by the timestamp alone left the choice to the engine, so
    /// the list, the detail view and a `trace_name` filter could each pick a different span's name, and the
    /// same query could answer differently twice. `span_id` makes the order total.
    #[tokio::test]
    async fn a_trace_displays_one_name_however_it_is_asked_for() {
        use sideseat_ports::filters::{Filter, StringOp};

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        // Inserted in the order that contradicts the tie-break, so arrival order cannot pass for it.
        let root = |span_id: &str, name: &str| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "t1".to_string(),
            span_id: span_id.to_string(),
            span_name: name.to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0,
            timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(&conn, &[root("z-root", "zeta"), root("a-root", "alpha")])
                .expect("insert");
        }

        let conn = service.conn();
        let (traces, _) = list_traces(&conn, &trace_filter_params(project, vec![])).expect("list");
        assert_eq!(
            traces[0].trace_name.as_deref(),
            Some("alpha"),
            "the earliest span in the total order (timestamp, span_id) names the trace"
        );

        let detail = get_trace(&conn, project, "t1")
            .expect("query")
            .expect("trace exists");
        assert_eq!(
            detail.trace_name, traces[0].trace_name,
            "the detail view and the list must show the same name"
        );

        let (filtered, _) = list_traces(
            &conn,
            &trace_filter_params(
                project,
                vec![Filter::String {
                    column: "trace_name".to_string(),
                    operator: StringOp::Eq,
                    value: traces[0].trace_name.clone().expect("a displayed name"),
                }],
            ),
        )
        .expect("list");
        assert_eq!(
            filtered.len(),
            1,
            "a filter for the displayed name must return the trace displaying it"
        );
    }

    /// No query hand-writes the displayed trace name; every one takes it from `trace_display_name`.
    ///
    /// It was hand-written in three DuckDB projections and one ClickHouse projection, and the shared helper
    /// was used only by the *filter* - so the filter's total order and the projections' partial one were
    /// different expressions, and a trace with two roots at one instant was displayed under one name and
    /// matched under another. A new copy compiles and passes every behavioural test, so this reads the source.
    #[test]
    fn the_displayed_trace_name_is_defined_once_per_backend() {
        for (path, source) in [
            (
                "duckdb/repositories/query.rs",
                include_str!("query.rs") as &str,
            ),
            (
                "clickhouse/repositories/query.rs",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../adapter-clickhouse/src/repositories/query.rs"
                )),
            ),
        ] {
            // Assembled at runtime, or this test's own source is the first thing it finds.
            let column = "span_name";
            let needles = [format!("FIRST(s.{column}"), format!("If(s.{column}")];
            for (number, line) in source.lines().enumerate() {
                let sql = line.trim_start();
                if sql.starts_with("//") {
                    continue;
                }
                for needle in &needles {
                    assert!(
                        !sql.contains(needle.as_str()),
                        "{path}:{} hand-writes the displayed trace name; \
                         call trace_display_name instead: {sql}",
                        number + 1
                    );
                }
            }
        }
    }

    /// A trace with no value for a displayed column matches "none of" that column.
    ///
    /// The trace list renders a filter on a displayed-but-per-span column against the aggregate the row
    /// shows, and the negation as written is `FIRST(user_id ORDER BY ...) NOT IN ('x')` - which is NULL for a
    /// trace that has no user id anywhere, so it was dropped. Those are precisely the traces that are not x.
    /// Every negation is now the complement of its positive form, in its own subquery.
    #[tokio::test]
    async fn a_trace_with_no_value_matches_none_of_that_value() {
        use sideseat_ports::filters::{Filter, OptionsOp};

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let span = |trace: &str, user: Option<&str>| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: trace.to_string(),
            span_id: format!("{trace}-s1"),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0,
            timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
            user_id: user.map(str::to_string),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(
                &conn,
                &[span("t-user", Some("alice")), span("t-none", None)],
            )
            .expect("insert");
        }

        let conn = service.conn();
        let (traces, total) = list_traces(
            &conn,
            &trace_filter_params(
                project,
                vec![Filter::StringOptions {
                    column: "user_id".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["alice".to_string()],
                }],
            ),
        )
        .expect("list_traces");
        let ids: Vec<&str> = traces.iter().map(|t| t.trace_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["t-none"],
            "a trace with no user id is not alice, so it matches \"none of alice\""
        );
        assert_eq!(total, 1, "the count must agree with the page");
    }

    /// A filter reads the delivery the list displays, not one it superseded.
    ///
    /// Every filter subquery and both count queries read the raw table, on the recorded grounds that
    /// "duplicates share identical data for all filterable columns". A corrected re-delivery is exactly the
    /// case that breaks: it is a second row with *different* values, and superseding the first is what
    /// `DEDUP_SPANS` is for. Measured before the fix, on one re-delivered root span: the row displayed `new`
    /// and 5,000 tokens, while `trace_name = new` returned nothing, "none of old" excluded it, `model = old`
    /// matched it, and the filter's own total was 5,100 - the sum of both deliveries.
    #[tokio::test]
    async fn a_filter_reads_the_delivery_the_row_displays() {
        use sideseat_ports::filters::{Filter, NumberOp, OptionsOp, StringOp};

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        // The same span twice: same ids, same timestamp, corrected name, model and tokens.
        let root = |name: &str, tokens: i64| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "t1".to_string(),
            span_id: "r1".to_string(),
            span_name: name.to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0,
            timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
            gen_ai_usage_total_tokens: tokens,
            gen_ai_request_model: Some(name.to_string()),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(&conn, &[root("old", 100)]).expect("insert");
        }
        {
            let conn = service.conn();
            insert_batch(&conn, &[root("new", 5000)]).expect("re-deliver");
        }

        let conn = service.conn();
        let (rows, _) = list_traces(&conn, &trace_filter_params(project, vec![])).expect("list");
        assert_eq!(rows[0].trace_name.as_deref(), Some("new"), "premise");
        assert_eq!(rows[0].total_tokens, 5_000, "premise");

        let matches = |filters: Vec<Filter>| {
            let (rows, total) =
                list_traces(&conn, &trace_filter_params(project, filters)).expect("list");
            // The count and the page must agree: they are two statements about one question.
            assert_eq!(
                rows.len() as u64,
                total,
                "the count disagrees with the page"
            );
            rows.len()
        };

        assert_eq!(
            matches(vec![Filter::String {
                column: "trace_name".to_string(),
                operator: StringOp::Eq,
                value: "new".to_string(),
            }]),
            1,
            "the row displays `new`, so a filter for `new` must return it"
        );
        assert_eq!(
            matches(vec![Filter::StringOptions {
                column: "trace_name".to_string(),
                operator: OptionsOp::NoneOf,
                value: vec!["old".to_string()],
            }]),
            1,
            "nothing displays `old`, so the trace is not excluded by \"none of old\""
        );
        assert_eq!(
            matches(vec![Filter::String {
                column: "gen_ai_request_model".to_string(),
                operator: StringOp::Eq,
                value: "old".to_string(),
            }]),
            0,
            "the superseded delivery's model is not the trace's model"
        );
        assert_eq!(
            matches(vec![Filter::Number {
                column: "total_tokens".to_string(),
                operator: NumberOp::Gt,
                value: 5_000.0,
            }]),
            0,
            "the filter's total must be the 5,000 displayed, not 5,100 summed over both deliveries"
        );
    }

    /// An input preview is the first thing the trace received; an output preview the last it produced.
    ///
    /// Both are chosen among the root spans first and fall back to any span. The fallback for the output side
    /// was already descending, and the root branch had no ordering at all - so once a tie-break made it
    /// deterministic it deterministically chose the *earliest* root output, which is the wrong end of the
    /// trace. Two roots is what makes the two ends distinguishable.
    #[tokio::test]
    async fn a_trace_shows_its_first_input_and_its_last_output() {
        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let root = |span_id: &str, offset: i64, text: &str| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "t1".to_string(),
            span_id: span_id.to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0 + chrono::Duration::seconds(offset),
            timestamp_end: Some(t0 + chrono::Duration::seconds(offset + 1)),
            input_preview: Some(format!("in-{text}")),
            output_preview: Some(format!("out-{text}")),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(&conn, &[root("r1", 0, "first"), root("r2", 5, "last")]).expect("insert");
        }

        let conn = service.conn();
        let (rows, _) = list_traces(&conn, &trace_filter_params(project, vec![])).expect("list");
        assert_eq!(rows[0].input_preview.as_deref(), Some("in-first"));
        assert_eq!(rows[0].output_preview.as_deref(), Some("out-last"));

        let detail = get_trace(&conn, project, "t1")
            .expect("query")
            .expect("trace exists");
        assert_eq!(
            (detail.input_preview, detail.output_preview),
            (
                rows[0].input_preview.clone(),
                rows[0].output_preview.clone()
            ),
            "the detail view and the list must show the same previews"
        );
    }

    /// Deleting a session reports every trace it removed, not the caller's earlier snapshot.
    ///
    /// The delete re-resolves the session, so it also removes a trace that joined after the caller resolved
    /// it - correctly, since the caller is answered 204. But the caller tombstones and reclaims files for
    /// what it resolved, so such a trace used to lose its rows while keeping its file associations forever,
    /// with no tombstone: the trace sweep walks tombstones and had none, and the session sweep resolves
    /// sessions through analytics rows that were gone. Returning the ids is what lets the caller cover it.
    #[tokio::test]
    async fn deleting_a_session_reports_every_trace_it_removed() {
        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let span = |trace: &str, offset: i64| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: trace.to_string(),
            span_id: format!("{trace}-s1"),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0 + chrono::Duration::seconds(offset),
            timestamp_end: Some(t0 + chrono::Duration::seconds(offset + 1)),
            session_id: Some("session-1".to_string()),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(&conn, &[span("trace-early", 0)]).expect("insert");
        }

        // What a caller resolves before deleting. Scoped: the connection is exclusive, so holding it while
        // opening another deadlocks.
        {
            let conn = service.conn();
            let snapshot =
                get_trace_ids_for_sessions(&conn, project, &["session-1".to_string()], None)
                    .expect("resolve");
            assert_eq!(snapshot, vec!["trace-early".to_string()], "premise");
        }

        // A trace joins the session after that resolution - a writer that passed the fence earlier.
        {
            let conn = service.conn();
            insert_batch(&conn, &[span("trace-late", 5)]).expect("insert late");
        }

        let conn = service.conn();
        let mut deleted =
            delete_sessions(&conn, project, &["session-1".to_string()]).expect("delete");
        deleted.sort();
        assert_eq!(
            deleted,
            vec!["trace-early".to_string(), "trace-late".to_string()],
            "the deletion removed both traces, so it must report both"
        );

        // And both really are gone, so the report is not a claim about rows that survived.
        for trace in ["trace-early", "trace-late"] {
            assert!(
                get_trace(&conn, project, trace).expect("query").is_none(),
                "{trace} is still readable after its session was deleted"
            );
        }
    }

    /// A filter with an empty value list changes nothing, on every list.
    ///
    /// An empty option list means "no value chosen", which the renderers answer with `1=1` - neutral in the
    /// query's own WHERE and *not* neutral once wrapped in a subquery over a narrower relation. `session_id
    /// any of []` became `trace_id IN (traces that have a session)`, so every sessionless trace vanished from
    /// a list nobody had filtered. Such a filter now contributes no condition at all.
    #[tokio::test]
    async fn an_empty_value_list_is_not_a_filter() {
        use sideseat_ports::filters::{Filter, OptionsOp};

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let span = |trace: &str, session: Option<&str>| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: trace.to_string(),
            span_id: format!("{trace}-s1"),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0,
            timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
            session_id: session.map(str::to_string),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(
                &conn,
                &[
                    span("t-in-a", Some("session-a")),
                    span("t-sessionless", None),
                ],
            )
            .expect("insert");
        }

        let conn = service.conn();
        let unfiltered = list_traces(&conn, &trace_filter_params(project, vec![]))
            .expect("list_traces")
            .0
            .len();
        assert_eq!(unfiltered, 2, "premise of the test");

        for operator in [OptionsOp::AnyOf, OptionsOp::NoneOf] {
            for column in ["session_id", "user_id", "gen_ai_request_model"] {
                let empty = vec![Filter::StringOptions {
                    column: column.to_string(),
                    operator: operator.clone(),
                    value: vec![],
                }];
                let (rows, total) =
                    list_traces(&conn, &trace_filter_params(project, empty.clone()))
                        .expect("list_traces");
                assert_eq!(
                    (rows.len(), total),
                    (unfiltered, unfiltered as u64),
                    "an empty {column} list must leave the trace list alone"
                );

                let (spans, _) = list_spans(
                    &conn,
                    &ListSpansParams {
                        project_id: ProjectId::from(project),
                        page: 1,
                        limit: 50,
                        filters: empty,
                        ..Default::default()
                    },
                )
                .expect("list_spans");
                assert_eq!(
                    spans.len(),
                    2,
                    "an empty {column} list must leave the span list alone"
                );
            }
        }
    }

    /// A session-list filter selects sessions, whichever span of the session carries the value.
    ///
    /// Three defects in one predicate, all the trace list's already-fixed shape by another route: a filter on
    /// a column only the session's *children* carry matched nothing, because a child names no session and the
    /// row predicate requires one; `none of alice` returned a session that used alice in one span and bob in
    /// the next; and it dropped every session with no value at all, which is exactly the sessions that are
    /// not alice.
    #[tokio::test]
    async fn a_session_list_filter_selects_sessions_not_span_rows() {
        use sideseat_ports::filters::{Filter, OptionsOp, StringOp};

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let span = |trace: &str, span_id: &str, session: &str, user: Option<&str>| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: trace.to_string(),
            span_id: span_id.to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0,
            timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
            session_id: Some(session.to_string()),
            user_id: user.map(str::to_string),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(
                &conn,
                &[
                    // session-1 used alice once and bob once, so it is *not* "none of alice".
                    span("t1", "s1", "session-1", Some("alice")),
                    span("t1", "s2", "session-1", Some("bob")),
                    // session-2's root names the session and carries no user; its child carries the
                    // user and the model, and names no session - the ordinary framework shape.
                    span("t2", "s1", "session-2", None),
                    NormalizedSpan {
                        user_id: Some("carol".to_string()),
                        gen_ai_request_model: Some("haiku".to_string()),
                        parent_span_id: Some("s1".to_string()),
                        session_id: None,
                        span_id: "s2".to_string(),
                        ..span("t2", "s2", "session-2", None)
                    },
                ],
            )
            .expect("insert");
        }

        let conn = service.conn();
        let ask = |params: ListSessionsParams| {
            let (rows, total) = list_sessions(&conn, &params).expect("list_sessions");
            let mut ids: Vec<String> = rows.into_iter().map(|r| r.session_id).collect();
            ids.sort();
            (ids, total)
        };
        let base = || ListSessionsParams {
            project_id: ProjectId::from(project),
            page: 1,
            limit: 50,
            ..Default::default()
        };
        let only_two = (vec!["session-2".to_string()], 1);

        assert_eq!(
            ask(ListSessionsParams {
                filters: vec![Filter::StringOptions {
                    column: "user_id".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["alice".to_string()],
                }],
                ..base()
            }),
            only_two,
            "the session that used alice is not \"none of alice\"; the one with no user is"
        );

        assert_eq!(
            ask(ListSessionsParams {
                filters: vec![Filter::String {
                    column: "gen_ai_request_model".to_string(),
                    operator: StringOp::Eq,
                    value: "haiku".to_string(),
                }],
                ..base()
            }),
            only_two,
            "the model sits on a child span that names no session, and it is still the session's"
        );

        // The dedicated parameters take the same route, for the same reason.
        assert_eq!(
            ask(ListSessionsParams {
                user_id: Some("carol".to_string()),
                ..base()
            }),
            only_two,
            "a user recorded on a child span is the session's user"
        );
        assert_eq!(
            ask(ListSessionsParams {
                user_id: Some("nobody".to_string()),
                ..base()
            }),
            (vec![], 0),
            "and a user nobody has selects nothing, so the subquery is not a no-op"
        );
    }

    /// A negated aggregate filter binds in step with its neighbours, in every order.
    ///
    /// It contributes its own subquery - optionally with a `gen_totals` join whose scope binds are rendered
    /// *before* the subquery's own project id - in the middle of a filter list. A mistake in that order is a
    /// query that runs and compares the wrong values, so it is checked by asking the same question with the
    /// filters permuted and requiring one answer.
    #[tokio::test]
    async fn a_negated_aggregate_filter_binds_in_step_with_its_neighbours() {
        use sideseat_ports::filters::{Filter, NumberOp, OptionsOp, StringOp};

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let span = |trace: &str, user: Option<&str>, model: &str, tokens: i64| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: trace.to_string(),
            span_id: format!("{trace}-s1"),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0,
            timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
            user_id: user.map(str::to_string),
            gen_ai_request_model: Some(model.to_string()),
            gen_ai_usage_input_tokens: tokens,
            gen_ai_usage_total_tokens: tokens,
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(
                &conn,
                &[
                    span("t-alice", Some("alice"), "haiku", 3000),
                    span("t-none", None, "haiku", 3000),
                    span("t-other", Some("bob"), "sonnet", 100),
                ],
            )
            .expect("insert");
        }

        let conn = service.conn();
        let names = |filters: Vec<Filter>| {
            let (rows, total) =
                list_traces(&conn, &trace_filter_params(project, filters)).expect("list_traces");
            let mut ids: Vec<String> = rows.into_iter().map(|t| t.trace_id).collect();
            ids.sort();
            (ids, total)
        };

        let not_alice = Filter::StringOptions {
            column: "user_id".to_string(),
            operator: OptionsOp::NoneOf,
            value: vec!["alice".to_string()],
        };
        let haiku = Filter::String {
            column: "gen_ai_request_model".to_string(),
            operator: StringOp::Eq,
            value: "haiku".to_string(),
        };
        let big = Filter::Number {
            column: "total_tokens".to_string(),
            operator: NumberOp::Gt,
            value: 2500.0,
        };

        // "Not alice, on haiku, over 2500 tokens" is t-none alone, whatever order the filters arrive in.
        let expected = (vec!["t-none".to_string()], 1);
        assert_eq!(
            names(vec![not_alice.clone(), haiku.clone(), big.clone()]),
            expected
        );
        assert_eq!(
            names(vec![haiku.clone(), not_alice.clone(), big.clone()]),
            expected
        );
        assert_eq!(names(vec![big, haiku, not_alice]), expected);

        // And the path that carries a `gen_totals` join, whose scope binds are rendered *ahead* of the
        // subquery's own project id. A time window makes the scope carry a bind of its own, so the two
        // groups are distinguishable: with them swapped the project id is compared against a timestamp.
        // `total_tokens` is an aggregate, so "none of 3000" is the complement over traces totalling 3000.
        let (rows, total) = list_traces(
            &conn,
            &ListTracesParams {
                project_id: ProjectId::from(project),
                page: 1,
                limit: 50,
                from_timestamp: Some(t0 - chrono::Duration::seconds(5)),
                filters: vec![Filter::StringOptions {
                    column: "total_tokens".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["3000".to_string()],
                }],
                ..Default::default()
            },
        )
        .expect("list_traces");
        let mut ids: Vec<String> = rows.into_iter().map(|t| t.trace_id).collect();
        ids.sort();
        assert_eq!(
            (ids, total),
            (vec!["t-other".to_string()], 1),
            "only the trace that does not total 3000 survives, and the count must agree"
        );
    }

    /// The session filter's binds stay in step with other filters, and with negation.
    ///
    /// It contributes a subquery with its own placeholders in the middle of a filter list, so its binds have
    /// to interleave with the others in exactly the order the conditions are joined. A mistake there is a
    /// silently wrong answer, not an error - the query runs and compares the wrong values.
    #[tokio::test]
    async fn a_session_filter_binds_in_step_with_its_neighbours() {
        use sideseat_ports::filters::{Filter, OptionsOp, StringOp};

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let span =
            |trace: &str, span_id: &str, session: &str, model: &str, offset: i64| NormalizedSpan {
                project_id: Some(project.to_string()),
                trace_id: trace.to_string(),
                span_id: span_id.to_string(),
                span_name: "generation".to_string(),
                observation_type: Some(ObservationType::Generation),
                timestamp_start: t0 + chrono::Duration::seconds(offset),
                timestamp_end: Some(t0 + chrono::Duration::seconds(offset + 1)),
                session_id: Some(session.to_string()),
                gen_ai_request_model: Some(model.to_string()),
                ..Default::default()
            };

        {
            let conn = service.conn();
            insert_batch(
                &conn,
                &[
                    span("trace-a", "s1", "session-a", "haiku", 0),
                    span("trace-b", "s1", "session-b", "sonnet", 10),
                ],
            )
            .expect("insert");
        }

        let conn = service.conn();
        let count = |filters: Vec<Filter>| {
            list_spans(
                &conn,
                &ListSpansParams {
                    project_id: ProjectId::from(project),
                    page: 1,
                    limit: 50,
                    filters,
                    ..Default::default()
                },
            )
            .expect("list_spans")
            .0
            .len()
        };

        let session = |value: &str| Filter::String {
            column: "session_id".to_string(),
            operator: StringOp::Eq,
            value: value.to_string(),
        };
        let model = |value: &str| Filter::String {
            column: "gen_ai_request_model".to_string(),
            operator: StringOp::Eq,
            value: value.to_string(),
        };

        // The session filter before and after another filter: both orders must agree, which they only do
        // if each filter's binds follow its own condition.
        assert_eq!(count(vec![session("session-a"), model("haiku")]), 1);
        assert_eq!(count(vec![model("haiku"), session("session-a")]), 1);
        // And a combination that matches nothing, so a swapped bind cannot pass by luck.
        assert_eq!(count(vec![session("session-a"), model("sonnet")]), 0);
        assert_eq!(count(vec![model("sonnet"), session("session-a")]), 0);

        // Negation is the complement over *sessions*, not the negated row predicate.
        assert_eq!(
            count(vec![Filter::StringOptions {
                column: "session_id".to_string(),
                operator: OptionsOp::NoneOf,
                value: vec!["session-a".to_string()],
            }]),
            1,
            "excluding session-a must leave exactly the other trace's span"
        );
    }

    /// A session *filter* on the trace and span lists also honours the canonical session.
    ///
    /// Parity between the backends is not enough on its own - they can agree on being wrong - so this pins
    /// the answer itself. The filter asked whether *any* span of the trace named the session, so a row
    /// displayed under one session matched a filter for another.
    #[tokio::test]
    async fn a_session_filter_selects_only_traces_canonically_in_it() {
        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let span = |span_id: &str, session: &str, offset: i64| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "trace-1".to_string(),
            span_id: span_id.to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0 + chrono::Duration::seconds(offset),
            session_id: Some(session.to_string()),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(
                &conn,
                &[
                    span("span-1", "session-canonical", 0),
                    span("span-2", "session-stray", 1),
                ],
            )
            .expect("insert");
        }

        let conn = service.conn();
        for (session, expect_traces, expect_spans) in [
            ("session-canonical", 1usize, 2usize),
            ("session-stray", 0, 0),
        ] {
            let traces = list_traces(
                &conn,
                &ListTracesParams {
                    project_id: ProjectId::from(project),
                    session_id: Some(session.to_string()),
                    page: 1,
                    limit: 50,
                    ..Default::default()
                },
            )
            .expect("list_traces");
            assert_eq!(
                traces.0.len(),
                expect_traces,
                "trace list filtered by {session}"
            );

            let spans = list_spans(
                &conn,
                &ListSpansParams {
                    project_id: ProjectId::from(project),
                    session_id: Some(session.to_string()),
                    page: 1,
                    limit: 50,
                    ..Default::default()
                },
            )
            .expect("list_spans");
            assert_eq!(
                spans.0.len(),
                expect_spans,
                "span list filtered by {session}"
            );
        }
    }

    /// A trace belongs to exactly one session, and deleting another session leaves it alone.
    ///
    /// Every *view* already treats a trace's session as the one on its earliest span, but membership asked
    /// whether *any* span of the trace named the session. So a trace whose spans name two sessions belonged
    /// to both: both sessions' reads returned it in full, and deleting either deleted the whole trace -
    /// taking content the UI was displaying under the other. That last one is data loss, not a display
    /// inconsistency.
    #[tokio::test]
    async fn a_trace_belongs_to_one_session_across_reads_and_deletion() {
        use crate::repositories::messages::get_messages;
        use sideseat_ports::types::MessageQueryParams;

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(60);

        let payload = |text: &str| {
            Some(
                serde_json::json!([{
                    "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
                    "content": {"role": "user", "content": text}
                }])
                .to_string(),
            )
        };

        // One trace, two spans naming different sessions. The earliest span is the canonical one.
        let span = |span_id: &str, session: &str, offset: i64, text: &str| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "trace-1".to_string(),
            span_id: span_id.to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0 + chrono::Duration::seconds(offset),
            session_id: Some(session.to_string()),
            messages: payload(text),
            ..Default::default()
        };

        {
            let conn = service.conn();
            insert_batch(
                &conn,
                &[
                    span("span-1", "session-canonical", 0, "the first turn"),
                    span(
                        "span-2",
                        "session-stray",
                        1,
                        "a later span with another session",
                    ),
                ],
            )
            .expect("insert");
        }

        let rows_for = |session: &str| {
            let conn = service.conn();
            get_messages(
                &conn,
                &MessageQueryParams {
                    project_id: ProjectId::from(project),
                    session_id: Some(session.to_string()),
                    ..Default::default()
                },
            )
            .expect("query")
            .rows
        };

        assert_eq!(
            rows_for("session-canonical").len(),
            2,
            "the canonical session holds the whole trace"
        );
        assert!(
            rows_for("session-stray").is_empty(),
            "a session named only by a later span does not own the trace"
        );

        // And which sessions the trace reports is the same single answer.
        {
            let conn = service.conn();
            let sessions =
                get_session_ids_for_traces(&conn, project, &["trace-1".to_string()], None)
                    .expect("sessions");
            assert_eq!(sessions, vec!["session-canonical".to_string()]);
        }

        // Deleting the stray session must not touch the trace. This is the data-loss case.
        {
            let conn = service.conn();
            let deleted =
                delete_sessions(&conn, project, &["session-stray".to_string()]).expect("delete");
            assert!(
                deleted.is_empty(),
                "no trace belongs to the stray session: {deleted:?}"
            );
        }
        assert_eq!(
            rows_for("session-canonical").len(),
            2,
            "the trace survived the other session's deletion"
        );
    }

    /// What the canonical session-membership subquery costs, measured against the predicate it replaced.
    ///
    /// `make bench-http` showed the concurrent session-read p95 at 154.6 ms against its 150 ms ceiling right
    /// after this area changed - but on a machine at load 13-35, where CLAUDE.md already records this figure
    /// moving between 43 and 135 ms for another operation. That number cannot settle the question.
    ///
    /// This can, without a quiet machine: both formulations run **interleaved in one process**, so whatever
    /// else the host is doing hits them equally and the *ratio* is meaningful even when the absolute times
    /// are not. The concern is structural rather than speculative - membership now deduplicates, which is a
    /// window function over the project's spans and cannot use an index, so the read may be paying a second
    /// full pass.
    ///
    /// `cargo test --locked --release -p sideseat-server bench_session_membership -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn bench_session_membership() {
        const TRACES: usize = 4_000;
        const SPANS_PER_TRACE: usize = 5;
        const ITERATIONS: usize = 15;

        let (_tmp, service) = create_test_service().await;
        let project = "p";
        let t0 = Utc::now() - chrono::Duration::seconds(7_200);

        // One session holding a tenth of the traces, which is the shape a session read faces.
        let mut spans = Vec::with_capacity(TRACES * SPANS_PER_TRACE);
        for t in 0..TRACES {
            let session = (t % 10 == 0).then(|| format!("session-{}", t / 10));
            for sp in 0..SPANS_PER_TRACE {
                spans.push(NormalizedSpan {
                    project_id: Some(project.to_string()),
                    trace_id: format!("trace-{t:05}"),
                    span_id: format!("span-{sp}"),
                    span_name: "generation".to_string(),
                    observation_type: Some(ObservationType::Generation),
                    timestamp_start: t0 + chrono::Duration::milliseconds((t * 10 + sp) as i64),
                    session_id: session.clone(),
                    messages: Some(
                        serde_json::json!([{
                            "source": {"event": {"name": "gen_ai.user.message",
                                                 "time": "2025-01-01T00:00:00Z"}},
                            "content": {"role": "user", "content": "a turn of conversation"}
                        }])
                        .to_string(),
                    ),
                    ..Default::default()
                });
            }
        }
        {
            let conn = service.conn();
            for chunk in spans.chunks(2_000) {
                insert_batch(&conn, chunk).expect("insert");
            }
        }

        // The predicate this replaced, kept here only as the comparison arm.
        const LOOSE: &str =
            "SELECT DISTINCT trace_id FROM otel_spans WHERE project_id = ? AND session_id = ?";

        let conn = service.conn();
        let production_query = sideseat_query_sql::messages::session_trace_ids(
            project,
            "session-7",
            None,
            Backend::Duckdb,
        );
        // The form this replaced: deduplicate the whole project, then pick the canonical session. Kept
        // here only as a comparison arm, to show what the narrowing buys.
        const CANONICAL_WIDE: &str = "SELECT trace_id FROM ( \
               SELECT trace_id, arg_min(session_id, (timestamp_start, span_id)) AS canonical_session \
               FROM (SELECT * FROM otel_spans \
                     QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                                ORDER BY ingested_at DESC, rowid DESC) = 1) \
               WHERE project_id = ? AND session_id IS NOT NULL AND session_id != '' \
               GROUP BY trace_id \
             ) WHERE canonical_session = ?";

        let run = |subquery: &str, subquery_binds: usize| -> (std::time::Duration, Vec<String>) {
            let sql = format!(
                "SELECT DISTINCT trace_id FROM {DEDUP_SPANS} \
                 WHERE project_id = ? AND trace_id IN ({subquery})"
            );
            let mut binds: Vec<String> = vec![project.to_string()];
            // The subquery's own binds, in the order its `?` appear.
            if subquery_binds == 4 {
                binds.extend([
                    project.to_string(),
                    project.to_string(),
                    "session-7".to_string(),
                    "session-7".to_string(),
                ]);
            } else {
                binds.extend([project.to_string(), "session-7".to_string()]);
            }
            let started = std::time::Instant::now();
            let mut stmt = conn.prepare(&sql).expect("prepare");
            let params: Vec<&dyn duckdb::ToSql> =
                binds.iter().map(|v| v as &dyn duckdb::ToSql).collect();
            // The trace ids, not a count. Two formulations selecting the same *number* of different traces
            // would pass an equality check on counts, so a faster wrong answer could have been adopted on
            // the strength of it.
            let mut rows = stmt.query(params.as_slice()).expect("query");
            let mut ids: Vec<String> = Vec::new();
            while let Some(row) = rows.next().expect("row") {
                ids.push(row.get(0).expect("trace_id"));
            }
            assert!(!ids.is_empty(), "the fixture must match traces, got none");
            ids.sort();
            (started.elapsed(), ids)
        };

        // The two correct forms must select the same rows, or a speed comparison means nothing.
        let (_, want) = run(CANONICAL_WIDE, 2);
        let (_, got) = run(production_query.sql(), 4);
        assert_eq!(
            want, got,
            "narrowing the deduplication must not change *which* traces are selected"
        );

        // Interleaved, so whatever else the host is doing hits every arm equally - which is what makes the
        // ratio meaningful on a machine this benchmark cannot have to itself.
        let _ = run(LOOSE, 2);
        let mut wide = Vec::with_capacity(ITERATIONS);
        let mut loose = Vec::with_capacity(ITERATIONS);
        let mut production = Vec::with_capacity(ITERATIONS);
        for _ in 0..ITERATIONS {
            wide.push(run(CANONICAL_WIDE, 2).0);
            loose.push(run(LOOSE, 2).0);
            production.push(run(production_query.sql(), 4).0);
        }
        wide.sort();
        loose.sort();
        production.sort();

        let median = |v: &[std::time::Duration]| v[v.len() / 2].as_secs_f64() * 1000.0;

        eprintln!(
            "bench_session_membership: {} traces x {} spans, {ITERATIONS} interleaved iterations\n  \
             loose (raw, any span - the old, incorrect predicate): {:.2} ms\n  \
             canonical, dedup whole project:                       {:.2} ms  ({:.2}x loose)\n  \
             canonical, dedup candidates only (production):        {:.2} ms  ({:.2}x loose)",
            TRACES,
            SPANS_PER_TRACE,
            median(&loose),
            median(&wide),
            median(&wide) / median(&loose),
            median(&production),
            median(&production) / median(&loose),
        );
    }

    /// Session-membership SQL is owned by the typed builder, not either adapter.
    ///
    /// It has four placeholders in a non-obvious order (project, project, session, session), so a copy that
    /// drifts is a silently wrong answer rather than an error - no test fails, the query simply selects
    /// different traces. Eleven call sites ask this question, and they had already drifted twice: some read
    /// the raw table instead of the deduplicated one, and both message reads kept a hand-written copy of the
    /// SQL that a later change to the shared definition would not have reached.
    ///
    /// Checked on the source text because a second copy compiles and passes every behavioural test. The
    /// membership *test* is what identifies it - `WHERE canonical_session = ?` - since the two mirror
    /// queries legitimately select the same expression without testing it that way.
    #[test]
    fn the_session_membership_subquery_is_owned_by_the_builder() {
        const MEMBERSHIP_TEST: &str = "WHERE canonical_session = ?";
        for (name, source) in [
            ("duckdb/query.rs", include_str!("query.rs")),
            ("duckdb/messages.rs", include_str!("messages.rs")),
            (
                "clickhouse/query.rs",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../adapter-clickhouse/src/repositories/query.rs"
                )),
            ),
            (
                "clickhouse/messages.rs",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../adapter-clickhouse/src/repositories/messages.rs"
                )),
            ),
        ] {
            let code = source
                .split("#[cfg(test)]")
                .next()
                .expect("source before its test module");
            assert!(
                !code.contains(MEMBERSHIP_TEST),
                "{name} reintroduced session-membership SQL outside the typed builder"
            );
        }

        let builder = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../query-sql/src/messages.rs"
        ));
        let code = builder
            .split("#[cfg(test)]")
            .next()
            .expect("builder source before tests");
        assert_eq!(
            code.matches(MEMBERSHIP_TEST).count(),
            2,
            "the builder must own exactly one DuckDB and one ClickHouse lowering"
        );
    }
}
