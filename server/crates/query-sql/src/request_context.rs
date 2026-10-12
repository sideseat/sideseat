//! The rows a request span's view is composed from, for a producer that exports each request's delta.
//!
//! Two reads beside the span's own, both keyed: the earlier requests of its thread, and the tool spans holding the
//! calls those requests' deltas answer. Keyed means the key is the scan's only filter, so DuckDB reads through the
//! single-column index rather than the whole table (`keyed`), and ClickHouse's bloom filter skips the granules;
//! every other condition applies to the small set that scan materialises.

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};
use crate::keyed::{KEYED_CHUNK, duckdb_keyed};

/// The columns a composed request reads from a thread's earlier requests, in the order the row parser reads them.
///
/// The messages and the facts that place them, and nothing else: the search terms, the previews and the raw
/// pointers are bytes a composition never looks at, and on both backends a projection is what decides how much of
/// a row is read. The marks are among those facts: a composed row goes through the same projections as the row's
/// own view, and a projection that asks about a mark must see the one the span was stored with, or a request
/// withdrawn from its own view would still be composed into the next one's.
pub const THREAD_COLUMNS: &str = "trace_id, span_id, timestamp_start, status_code, messages, \
     observation_type, span_name, scope_name, scope_version, session_id, span_marks";

/// The same, plus the two columns a winner condition reads: a keyed relation is materialised before the condition
/// applies, so a column the condition names has to be in it.
const KEYED_COLUMNS: &str = "trace_id, span_id, timestamp_start, status_code, messages, \
     observation_type, span_name, scope_name, scope_version, session_id, span_marks, superseded_at, ingested_at";

/// The same, plus the call id a call read narrows on afterwards.
const KEYED_CALL_COLUMNS: &str = "trace_id, span_id, timestamp_start, status_code, messages, \
     observation_type, span_name, scope_name, scope_version, session_id, span_marks, superseded_at, ingested_at, \
     gen_ai_tool_call_id";

/// The earlier requests of one thread: every winning span whose derived `request_thread` is `thread` and which
/// started at or before `before_us`, oldest first.
///
/// The thread key alone inside the keyed scan. `project_id` is **not** a filter here: a thread key holds the
/// producer's own session id and the rule's id, so it does not span projects, and a second predicate in the scan
/// is what turns a DuckDB index read into a table read. The watermark and the start bound apply outside it.
pub fn thread_requests(
    thread: &str,
    before_us: i64,
    watermark_us: Option<i64>,
    backend: Backend,
) -> ParameterizedQuery {
    let mut values = vec![QueryValue::String(thread.to_string())];
    let (sql, bound) = match backend {
        Backend::Duckdb => {
            let source = duckdb_keyed("otel_spans", "request_thread", KEYED_COLUMNS, 1);
            let (winner, winner_values) = crate::winners::duckdb_winner_condition(watermark_us);
            values.extend(winner_values);
            (
                format!("SELECT {THREAD_COLUMNS} FROM {source} WHERE {winner}"),
                "EPOCH_US(timestamp_start) <= ?",
            )
        }
        Backend::Clickhouse => {
            let mut conditions = vec!["request_thread = ?".to_string()];
            if let Some(watermark) = watermark_us {
                conditions.push("toInt64(toUnixTimestamp64Micro(ingested_at)) < ?".to_string());
                values.push(QueryValue::Int64(watermark));
            }
            (
                format!(
                    "SELECT {THREAD_COLUMNS} FROM otel_spans WHERE {} \
                     ORDER BY ingested_at DESC LIMIT 1 BY project_id, trace_id, span_id",
                    conditions.join(" AND ")
                ),
                "toInt64(toUnixTimestamp64Micro(timestamp_start)) <= ?",
            )
        }
    };
    values.push(QueryValue::Int64(before_us));
    ParameterizedQuery::new(
        format!(
            "SELECT {THREAD_COLUMNS} FROM ({sql}) WHERE {bound} \
             ORDER BY timestamp_start ASC, span_id ASC, trace_id ASC"
        ),
        values,
    )
}

/// The tool spans of `trace_ids` that hold one of `call_ids`, read in the same columns.
///
/// Keyed on the **trace**, not on the call id: a thread's tool spans sit in the traces its requests sit in, and
/// `trace_id` already has the index every trace read uses. Keying on the call id would need a second index,
/// maintained on every span for the few that carry one. The call ids narrow the keyed rows afterwards, so the scan
/// keeps to the index and the read is proportional to the thread's traces.
///
/// One read per [`KEYED_CHUNK`] of the distinct traces, which is what keeps DuckDB on the index; every trace is
/// read. A trace's spans all come back in its own chunk, so the reads never overlap, and their rows sorted by
/// start, span and trace are in the order one read gives.
///
/// Each read is built as it is asked for, so only one copy of the call ids is held at a time.
pub fn calls_in_traces<'a>(
    trace_ids: &[&'a str],
    call_ids: &'a [&'a str],
    watermark_us: Option<i64>,
    backend: Backend,
) -> impl Iterator<Item = ParameterizedQuery> + 'a {
    let traces = crate::keyed::distinct_keys(trace_ids.iter().copied());
    (0..traces.len()).step_by(KEYED_CHUNK).map(move |at| {
        let chunk = &traces[at..traces.len().min(at + KEYED_CHUNK)];
        calls_in_trace_chunk(chunk, call_ids, watermark_us, backend)
    })
}

/// The read of `calls_in_traces` for one chunk of at most [`KEYED_CHUNK`] traces.
fn calls_in_trace_chunk(
    traces: &[&str],
    call_ids: &[&str],
    watermark_us: Option<i64>,
    backend: Backend,
) -> ParameterizedQuery {
    debug_assert!(traces.len() <= KEYED_CHUNK);
    let mut values: Vec<QueryValue> = traces
        .iter()
        .map(|id| QueryValue::String((*id).to_string()))
        .collect();
    let call_list = std::iter::repeat_n("?", call_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = match backend {
        Backend::Duckdb => {
            let source = duckdb_keyed("otel_spans", "trace_id", KEYED_CALL_COLUMNS, traces.len());
            let (winner, winner_values) = crate::winners::duckdb_winner_condition(watermark_us);
            values.extend(winner_values);
            format!("SELECT {THREAD_COLUMNS}, gen_ai_tool_call_id FROM {source} WHERE {winner}")
        }
        Backend::Clickhouse => {
            let placeholders = std::iter::repeat_n("?", traces.len())
                .collect::<Vec<_>>()
                .join(", ");
            let mut conditions = vec![format!("trace_id IN ({placeholders})")];
            if let Some(watermark) = watermark_us {
                conditions.push("toInt64(toUnixTimestamp64Micro(ingested_at)) < ?".to_string());
                values.push(QueryValue::Int64(watermark));
            }
            format!(
                "SELECT {THREAD_COLUMNS}, gen_ai_tool_call_id FROM otel_spans WHERE {} \
                 ORDER BY ingested_at DESC LIMIT 1 BY project_id, trace_id, span_id",
                conditions.join(" AND ")
            )
        }
    };
    values.extend(
        call_ids
            .iter()
            .map(|id| QueryValue::String((*id).to_string())),
    );
    ParameterizedQuery::new(
        format!(
            "SELECT {THREAD_COLUMNS} FROM ({sql}) WHERE gen_ai_tool_call_id IN ({call_list}) \
             ORDER BY timestamp_start ASC, span_id ASC, trace_id ASC"
        ),
        values,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key is the scan's only filter on both backends, every placeholder is bound, and the order is total.
    #[test]
    fn a_thread_read_is_keyed_and_fully_bound() {
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            for watermark in [None, Some(7)] {
                for query in [
                    thread_requests("t", 10, watermark, backend),
                    calls_in_trace_chunk(&["tr"], &["a", "b"], watermark, backend),
                ] {
                    assert_eq!(
                        query.sql().matches('?').count(),
                        query.params().len(),
                        "{backend:?} {watermark:?}: {}",
                        query.sql()
                    );
                    assert!(
                        query
                            .sql()
                            .contains("ORDER BY timestamp_start ASC, span_id ASC, trace_id ASC"),
                        "the order must be total, so two backends answer alike"
                    );
                    assert!(
                        !query.sql().contains("project_id = "),
                        "a project predicate beside the key would turn a DuckDB index read into a table read: \
                         {}",
                        query.sql()
                    );
                }
            }
            // The projection, not every column: a composition reads the messages and what places them, and on
            // both backends the projection is what decides how much of a row is read. `keyed`'s own
            // `SELECT * FROM keyed` is over the materialised projection, not over the table.
            let sql = thread_requests("t", 10, None, backend).sql().to_string();
            assert!(sql.contains(THREAD_COLUMNS));
            assert!(!sql.contains("* FROM otel_spans"), "{sql}");
        }
        // DuckDB keys inside a materialised relation; ClickHouse keys the scan itself.
        assert!(
            thread_requests("t", 10, None, Backend::Duckdb)
                .sql()
                .contains("WITH keyed AS MATERIALIZED")
        );
        // The calls are keyed on the trace, whose index every trace read already uses, and narrowed by their ids
        // outside that scan.
        let calls = calls_in_trace_chunk(&["tr"], &["a"], None, Backend::Duckdb);
        assert!(
            calls.sql().contains("WHERE trace_id IN (?)"),
            "{}",
            calls.sql()
        );
        assert!(calls.sql().contains("WHERE gen_ai_tool_call_id IN (?)"));
    }

    /// More traces than one read names are read in chunks, every distinct trace once, on both backends -
    /// never cut to the first chunk, which left a call in any later trace unread.
    #[test]
    fn a_call_read_names_every_trace_in_chunks() {
        let many: Vec<String> = (0..KEYED_CHUNK + 10).map(|n| n.to_string()).collect();
        let mut traces: Vec<&str> = many.iter().map(String::as_str).collect();
        traces.push("0");
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let reads: Vec<_> = calls_in_traces(&traces, &["a"], None, backend).collect();
            assert_eq!(reads.len(), 2);
            assert!(
                reads
                    .iter()
                    .all(|read| read.params().len() <= KEYED_CHUNK + 1)
            );
            let named: usize = reads.iter().map(|read| read.params().len() - 1).sum();
            assert_eq!(named, KEYED_CHUNK + 10, "each distinct trace, once");
        }
    }
}
