//! A store grown to a given size from a corpus store, for the read-path measurements: every analytics row
//! replicated under fresh identities.
//!
//! Each replica remaps every identity a read keys on - trace, span, parent, session, raw record, log digest,
//! datapoint - by hashing it with the replica's number, so lookups stay as selective as in a store that grew
//! from real traffic, and shifts every instant by the replica's number of hours, so a replica's rows are
//! appended together with their own time range, the way a store grows. Columns a store does not have are
//! skipped, and any column not named keeps its value.

use crate::DuckdbService;

/// `column` remapped for replica `r.k`: the first `width` hex digits of its hash with the replica's number.
fn identity(column: &str, width: usize) -> String {
    format!(
        "CASE WHEN {column} IS NULL THEN NULL ELSE md5({column} || '-' || r.k::VARCHAR)[1:{width}] END"
    )
}

fn session(column: &str) -> String {
    format!("CASE WHEN {column} IS NULL THEN NULL ELSE {column} || '-' || r.k::VARCHAR END")
}

fn shifted(column: &str) -> String {
    format!("{column} + to_hours(r.k)")
}

/// The columns each table's replicas rewrite, with what they are rewritten to.
fn rewrites() -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    vec![
        (
            "otel_spans",
            vec![
                ("trace_id", identity("trace_id", 32)),
                ("span_id", identity("span_id", 16)),
                ("parent_span_id", identity("parent_span_id", 16)),
                ("session_id", session("session_id")),
                ("raw_id", identity("raw_id", 64)),
                ("timestamp_start", shifted("timestamp_start")),
                ("timestamp_end", shifted("timestamp_end")),
                ("ingested_at", shifted("ingested_at")),
                ("superseded_at", shifted("superseded_at")),
            ],
        ),
        (
            "span_terms",
            vec![
                ("trace_id", identity("trace_id", 32)),
                ("span_id", identity("span_id", 16)),
                ("ingested_at", shifted("ingested_at")),
            ],
        ),
        (
            "otel_raw",
            vec![
                ("raw_id", identity("raw_id", 64)),
                ("received_at", shifted("received_at")),
            ],
        ),
        (
            "otel_raw_traces",
            vec![
                ("trace_id", identity("trace_id", 32)),
                ("raw_id", identity("raw_id", 64)),
            ],
        ),
        (
            "otel_logs",
            vec![
                ("log_digest", identity("log_digest", 64)),
                ("trace_id", identity("trace_id", 32)),
                ("span_id", identity("span_id", 16)),
                ("session_id", session("session_id")),
                ("timestamp", shifted("timestamp")),
                ("ingested_at", shifted("ingested_at")),
            ],
        ),
        (
            "log_terms",
            vec![
                ("log_digest", identity("log_digest", 64)),
                ("ingested_at", shifted("ingested_at")),
            ],
        ),
        (
            "otel_metrics",
            vec![
                ("datapoint_id", identity("datapoint_id", 64)),
                ("exemplar_trace_id", identity("exemplar_trace_id", 32)),
                ("exemplar_span_id", identity("exemplar_span_id", 16)),
                ("session_id", session("session_id")),
                ("timestamp", shifted("timestamp")),
                ("ingested_at", shifted("ingested_at")),
            ],
        ),
    ]
}

/// Grow the store until it holds at least `spans` span rows, by whole replicas of what it holds now, and
/// checkpoint it. Returns the span rows it holds.
pub(crate) fn replicate_to(service: &DuckdbService, spans: u64) -> u64 {
    let (stored, present) = {
        let conn = service.conn();
        let stored: u64 = conn
            .query_row("SELECT count(*) FROM otel_spans", [], |row| row.get(0))
            .expect("count spans");
        let mut statement = conn
            .prepare("SELECT table_name, column_name FROM information_schema.columns")
            .expect("columns");
        let present: std::collections::HashSet<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("columns")
            .collect::<Result<_, _>>()
            .expect("columns");
        (stored, present)
    };
    assert!(stored > 0, "a store with spans to replicate");
    let copies = spans.div_ceil(stored);
    if copies <= 1 {
        return stored;
    }
    // The copy is set up under a larger limit than the reads are measured under: sorting the replicas into
    // their appended order is not one of the reads, and the measured limit is restored before any read runs.
    let limit: String = service
        .conn()
        .query_row("SELECT current_setting('memory_limit')", [], |row| {
            row.get(0)
        })
        .expect("memory limit");
    let mut sql = "SET memory_limit = '4GB';\n".to_string();
    for (table, columns) in rewrites() {
        let rewritten = columns
            .into_iter()
            .filter(|(column, _)| present.contains(&(table.to_string(), (*column).to_string())))
            .map(|(column, expression)| format!("{expression} AS {column}"))
            .collect::<Vec<_>>()
            .join(", ");
        sql.push_str(&format!(
            "INSERT INTO {table} SELECT t.* REPLACE ({rewritten}) FROM {table} t, \
             (SELECT range AS k FROM range(1, {copies})) r ORDER BY r.k;\n"
        ));
    }
    sql.push_str(&format!(
        "CHECKPOINT; SET memory_limit = '{}B';",
        sideseat_core::constants::DUCKDB_MEMORY_LIMIT_BYTES
    ));
    service
        .write(|conn| conn.execute_batch(&sql).map_err(Into::into))
        .expect("replicate the store");
    let conn = service.conn();
    let restored: String = conn
        .query_row("SELECT current_setting('memory_limit')", [], |row| {
            row.get(0)
        })
        .expect("memory limit");
    assert_eq!(restored, limit, "the reads run under the production limit");
    conn.query_row("SELECT count(*) FROM otel_spans", [], |row| row.get(0))
        .expect("count spans")
}
