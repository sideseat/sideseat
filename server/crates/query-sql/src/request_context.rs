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
/// The call ids are chunked too, each read binding at most [`KEYED_CHUNK`] of them and
/// [`crate::keyed::BOUND_BYTES`] of their text: a long thread's calls, bound into one ClickHouse statement, passed
/// `max_query_size`. A span carries one call id, so no two reads return one
/// row, and the caller sorts what the reads return. Each read is built as it is asked for.
pub fn calls_in_traces<'a>(
    trace_ids: &[&'a str],
    call_ids: &'a [&'a str],
    watermark_us: Option<i64>,
    backend: Backend,
) -> impl Iterator<Item = ParameterizedQuery> + 'a {
    // Both lists distinct, as one `IN` list is - an id in two chunks would read its span twice - and each bounded
    // by count and by half the statement's byte budget, since both are client strings of any length. Chunk bounds
    // are worked out first and each read built only as it is asked for.
    let traces = std::sync::Arc::new(crate::keyed::distinct_keys(trace_ids.iter().copied()));
    let calls = std::sync::Arc::new(crate::keyed::distinct_keys(call_ids.iter().copied()));
    let budget = crate::keyed::BOUND_BYTES / 2;
    let ranges = |keys: &[&str]| -> Vec<std::ops::Range<usize>> {
        let mut at = 0;
        crate::keyed::bounded_chunks_within(keys, KEYED_CHUNK, budget)
            .map(|chunk| {
                let range = at..at + chunk.len();
                at = range.end;
                range
            })
            .collect()
    };
    let trace_ranges = ranges(&traces);
    // Only ClickHouse bounds a statement's size. Each call chunk reads its trace chunk again, so DuckDB, which has no
    // such bound, keeps one read per trace chunk however long the thread; ClickHouse rereads only for a thread whose
    // call ids pass half a statement's budget.
    let call_ranges = match backend {
        Backend::Duckdb => std::iter::once(0..calls.len()).collect(),
        Backend::Clickhouse => ranges(&calls),
    };
    trace_ranges.into_iter().flat_map(move |traced| {
        let traces = std::sync::Arc::clone(&traces);
        let calls = std::sync::Arc::clone(&calls);
        call_ranges.clone().into_iter().map(move |called| {
            calls_in_trace_chunk(
                &traces[traced.clone()],
                &calls[called],
                watermark_us,
                backend,
            )
        })
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

    /// A thread's call ids are each bound once - in chunks on ClickHouse, which bounds a statement's size, and in
    /// one list per trace chunk on DuckDB, which does not.
    #[test]
    fn a_long_thread_s_call_ids_are_chunked() {
        // Zero-padded, so their order is the sorted one.
        let ids: Vec<String> = (0..KEYED_CHUNK * 2 + 1)
            .map(|n| format!("call-{n:05}"))
            .collect();
        // Asked twice, read once.
        let mut calls: Vec<&str> = ids.iter().map(String::as_str).collect();
        calls.push(ids[0].as_str());
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let reads: Vec<_> = calls_in_traces(&["trace"], &calls, None, backend).collect();
            assert_eq!(reads.len(), if backend == Backend::Duckdb { 1 } else { 3 });
            let mut bound = Vec::new();
            for read in &reads {
                let these: Vec<String> = read
                    .params()
                    .iter()
                    .filter_map(|value| match value {
                        QueryValue::String(text) if text.starts_with("call-") => Some(text.clone()),
                        _ => None,
                    })
                    .collect();
                assert!(backend == Backend::Duckdb || these.len() <= KEYED_CHUNK);
                assert_eq!(read.sql().matches('?').count(), read.params().len());
                bound.extend(these);
            }
            bound.sort();
            assert_eq!(bound, ids);
        }
    }

    /// Long trace and call ids share one ClickHouse statement's byte budget: no read binds more than it, and every
    /// trace meets every call in exactly one read.
    #[test]
    fn long_ids_are_chunked_by_bytes() {
        use crate::keyed::BoundText;
        let traces: Vec<String> = (0..300).map(|n| format!("t{n:0>999}")).collect();
        let calls: Vec<String> = (0..300).map(|n| format!("c{n:0>999}")).collect();
        let trace_refs: Vec<&str> = traces.iter().map(String::as_str).collect();
        let call_refs: Vec<&str> = calls.iter().map(String::as_str).collect();
        let backend = Backend::Clickhouse;
        let mut met = std::collections::BTreeSet::new();
        for read in calls_in_traces(&trace_refs, &call_refs, None, backend) {
            let bound: Vec<&str> = read
                .params()
                .iter()
                .filter_map(|value| match value {
                    QueryValue::String(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert!(
                bound.iter().map(|text| text.bound_bytes()).sum::<usize>()
                    <= crate::keyed::BOUND_BYTES
            );
            let (ts, cs): (Vec<&str>, Vec<&str>) =
                bound.iter().partition(|text| text.starts_with('t'));
            for t in &ts {
                for c in &cs {
                    assert!(
                        met.insert((t.to_string(), c.to_string())),
                        "{t} and {c} met twice"
                    );
                }
            }
        }
        assert_eq!(met.len(), traces.len() * calls.len());
    }
}
