//! Query repository for OTEL API queries.

mod rows;

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use duckdb::{Connection, Row};

use crate::{DuckdbError, in_transaction};
use sideseat_core::utils::time::{micros_to_datetime, parse_iso_timestamp};
use sideseat_ports::types::{
    EventRow, FeedSpansParams, LinkRow, ListSessionsParams, ListSpansParams, ListTracesParams,
    ProjectId, SessionRow, SpanRow, TraceRow, parse_tags,
};
use sideseat_query_sql::confirmations;
use sideseat_query_sql::{Backend, analytics, dml};

use rows::{
    duckdb_values, execute_count_values, execute_filter_option_query, execute_session_query_values,
    execute_span_query_values, execute_trace_query_values, row_to_session, row_to_span,
    row_to_trace,
};

/// Inline winning-span relation used by repository contract tests.
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
/// **The latest delivery wins**, matching ClickHouse's `ReplacingMergeTree(ingested_at)` semantics and the
/// exporter's corrected replacement.
///
/// `QUALIFY ROW_NUMBER() = 1` keeps exactly one row when multiple deliveries share the maximum timestamp.
///
/// `rowid DESC` breaks equal-microsecond ties by physical insert order. Neither backend distinguishes
/// sub-microsecond recency because the version timestamp has microsecond precision.
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
    limit: usize,
) -> Result<Vec<SpanRow>, DuckdbError> {
    let query = analytics::spans_for_trace(project_id, trace_id, limit, Backend::Duckdb);
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

pub fn spans_with_matching_content(
    conn: &Connection,
    project_id: &str,
    records: &[(String, String, String)],
) -> Result<HashSet<(String, String, String)>, DuckdbError> {
    let Some(query) = confirmations::matching_spans(project_id, records, Backend::Duckdb) else {
        return Ok(HashSet::new());
    };
    let values = duckdb_values(query.params());
    let mut statement = conn.prepare(query.sql())?;
    let rows = statement.query_map(values.as_slice(), |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
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
/// Reads `DEDUP_SPANS`, not the append-only table, because only the winning revision can keep a file
/// association alive.
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
    // The write path replaces each datapoint row, so physical row count is the integrity check and agrees
    // with ClickHouse's `FINAL` count.
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

pub fn analytics_project_ids(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<ProjectId>, DuckdbError> {
    let query = analytics::analytics_project_ids(Backend::Duckdb, limit);
    let values = duckdb_values(query.params());
    let mut statement = conn.prepare(query.sql())?;
    let rows = statement.query_map(values.as_slice(), |row| row.get::<_, String>(0))?;
    let mut projects = Vec::new();
    for row in rows {
        projects.push(ProjectId::from(row?));
    }
    Ok(projects)
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

/// Repository contract tests.
#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
