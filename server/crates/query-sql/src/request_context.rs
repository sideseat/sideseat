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
/// a row is read.
pub const THREAD_COLUMNS: &str = "trace_id, span_id, timestamp_start, status_code, messages, \
     observation_type, span_name, scope_name, scope_version, session_id";

/// The same, plus the two columns a winner condition reads: a keyed relation is materialised before the condition
/// applies, so a column the condition names has to be in it.
const KEYED_COLUMNS: &str = "trace_id, span_id, timestamp_start, status_code, messages, \
     observation_type, span_name, scope_name, scope_version, session_id, superseded_at, ingested_at";

/// The same, plus the call id a call read narrows on afterwards.
const KEYED_CALL_COLUMNS: &str = "trace_id, span_id, timestamp_start, status_code, messages, \
     observation_type, span_name, scope_name, scope_version, session_id, superseded_at, ingested_at, \
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
/// At most [`KEYED_CHUNK`] traces per read, which is what keeps DuckDB on the index; a caller with more reads in
/// chunks.
pub fn calls_in_traces(
    trace_ids: &[&str],
    call_ids: &[&str],
    watermark_us: Option<i64>,
    backend: Backend,
) -> ParameterizedQuery {
    let traces = &trace_ids[..trace_ids.len().min(KEYED_CHUNK)];
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
                    calls_in_traces(&["tr"], &["a", "b"], watermark, backend),
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
        let calls = calls_in_traces(&["tr"], &["a"], None, Backend::Duckdb);
        assert!(
            calls.sql().contains("WHERE trace_id IN (?)"),
            "{}",
            calls.sql()
        );
        assert!(calls.sql().contains("WHERE gen_ai_tool_call_id IN (?)"));
    }

    /// More traces than one keyed read binds are cut to the chunk, so the read stays on the index.
    #[test]
    fn a_call_read_binds_at_most_one_chunk_of_traces() {
        let many: Vec<String> = (0..KEYED_CHUNK + 10).map(|n| n.to_string()).collect();
        let traces: Vec<&str> = many.iter().map(String::as_str).collect();
        let query = calls_in_traces(&traces, &["a"], None, Backend::Duckdb);
        assert_eq!(
            query.params().len(),
            KEYED_CHUNK + 1,
            "the chunk of traces, and the one call id"
        );
    }
}
