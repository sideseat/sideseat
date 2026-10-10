//! The frame records a framed request span's view opens with (`CarrierRule::frames_requests`).
//!
//! One read, bounded in the statement itself: the log records of the request's trace whose derived `frame_key` is
//! exactly the request's, each identity's winning revision once, in `(timestamp, log_digest, ordinal)` order - a
//! total order over identities, the one the span reads concatenate log messages in - and at most one past
//! `REQUEST_FRAMES_MAX_RECORDS` of them, so the view can tell it was cut. The key is stored in its project's and
//! trace's form, sixteen bytes as an unsigned 128-bit integer, so the records it finds are the trace's frames and no
//! other. DuckDB keeps one row per identity and reads through the key's index, the key alone in the scan, so it
//! reads the key's records; `frame_key` is NULL on every record that frames nothing, which the index holds no entry
//! for. ClickHouse, whose null map would cost a byte a row, stores zero there, reads `FINAL` and skips other traces
//! by its bloom filter, so its work follows the granules the trace's log records lie in. The key is bound as its
//! decimal text and cast in the statement, so it needs no placeholder type of its own.

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};

/// The columns a frame read returns, in the order the record parser reads them by name.
pub const FRAME_COLUMNS: &[&str] = &[
    "trace_id",
    "span_id",
    "timestamp_us",
    "log_digest",
    "ordinal",
    "messages",
];

/// The frame records of `trace_id` stating `key`, in the stored form (`RequestFramesParams::key`).
///
/// Two passes over the key's records, both in the one statement: the first reads every record's identity and the
/// length of its messages and chooses the records within both bounds; the second reads messages for those alone.
/// Nothing holds the messages of a record the bounds leave out, so a trace with more frames than fit in memory is
/// still answered: measured, choosing from 5,000 records of 64 KB under one key ran DuckDB out of memory when the
/// first pass carried the messages.
pub fn frames_in_trace(
    project_id: &str,
    trace_id: &str,
    key: u128,
    watermark_us: Option<i64>,
    backend: Backend,
) -> ParameterizedQuery {
    let key = QueryValue::String(key.to_string());
    // The conditions' values, in the order the conditions are written.
    let mut values = vec![
        QueryValue::String(project_id.to_string()),
        QueryValue::String(trace_id.to_string()),
    ];
    // Zero is every record's that frames nothing on ClickHouse, so it names no frame: refused in the statement, not
    // trusted to the caller. The key's equality is not among these: it is the scan's.
    let mut conditions = vec![
        "frame_key != 0".to_string(),
        "project_id = ?".to_string(),
        "trace_id = ?".to_string(),
        "span_id IS NOT NULL".to_string(),
        "messages != '[]'".to_string(),
    ];
    if let Some(watermark) = watermark_us {
        conditions.push(match backend {
            Backend::Duckdb => "EPOCH_US(ingested_at) < ?::BIGINT".to_string(),
            Backend::Clickhouse => "toInt64(toUnixTimestamp64Micro(ingested_at)) < ?".to_string(),
        });
        values.push(QueryValue::Int64(watermark));
    }
    let conditions = conditions.join(" AND ");
    // One past the bound, so the view can tell a trace with more frames than it joins from one with exactly as
    // many, and say it was cut.
    let limit = sideseat_core::constants::REQUEST_FRAMES_MAX_RECORDS + 1;
    // The byte bound in the statement as well: a record whose bytes would carry the running total past it comes
    // back as its identity alone, so no more than the bound is ever transferred or parsed.
    let max_bytes = sideseat_core::constants::REQUEST_FRAMES_MAX_BYTES;
    let window =
        "ORDER BY timestamp, log_digest, ordinal ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW";
    let sql = match backend {
        // Each pass an aggregate straight over a key-only scan through the key's index, keeping the first records
        // by `min_by(.., n)`: state of `n` rows whatever the key finds, where a materialised relation of the key's
        // records grew with them. The conditions gate the ordering value rather than filter the scan, since a
        // filter beside the key turns the index scan into a table scan (`crate::keyed`); `min_by` passes over a
        // row whose ordering value is NULL. The second pass asks which rows were chosen in the same place, for the
        // same reason: as a join, its scan took a dynamic `rowid IN (...)` filter and read 737,280 rows for 8.
        //
        // The record bound is not `ORDER BY ... LIMIT` over the table, which DuckDB answers by late
        // materialisation - the top rows' ids, then their columns by a join against a scan of the whole table.
        Backend::Duckdb => {
            values.extend([key.clone(), key]);
            let order = "{'ts': timestamp, 'log_digest': log_digest, 'ordinal': ordinal}";
            format!(
                "WITH picked AS (\
                   SELECT min_by({{'keyed_rowid': rowid, 'trace_id': trace_id, 'span_id': span_id, \
                                   'ts': timestamp, 'log_digest': log_digest, 'ordinal': ordinal, \
                                   'bytes': strlen(messages)}}, \
                                 CASE WHEN {conditions} THEN {order} END, {limit}) AS chosen \
                   FROM otel_logs WHERE frame_key = CAST(? AS UHUGEINT)), \
                 chosen AS MATERIALIZED (\
                   SELECT c.keyed_rowid, c.trace_id, c.span_id, c.ts AS timestamp, c.log_digest, c.ordinal, \
                          sum(c.bytes) OVER (ORDER BY c.ts, c.log_digest, c.ordinal \
                                             ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS running \
                   FROM (SELECT unnest(chosen) AS c FROM picked)), \
                 fetched AS (\
                   SELECT min_by({{'fetched_rowid': rowid, 'messages': messages}}, \
                                 CASE WHEN rowid IN (SELECT keyed_rowid FROM chosen WHERE running <= {max_bytes}) \
                                      THEN {order} END, {limit}) AS got \
                   FROM otel_logs WHERE frame_key = CAST(? AS UHUGEINT)) \
                 SELECT c.trace_id, c.span_id, EPOCH_US(c.timestamp) AS timestamp_us, c.log_digest, c.ordinal, \
                        f.messages \
                 FROM chosen c \
                 LEFT JOIN (SELECT g.fetched_rowid, g.messages FROM (SELECT unnest(got) AS g FROM fetched)) f \
                   ON f.fetched_rowid = c.keyed_rowid \
                 ORDER BY c.timestamp, c.log_digest, c.ordinal"
            )
        }
        // The selected ids are named apart from the columns, which the conditions test: ClickHouse resolves a
        // select alias inside `WHERE`, so `span_id IS NOT NULL` would ask it of `assumeNotNull(span_id)`.
        //
        // `FINAL` before the filters: the key, the watermark and the non-empty test apply to the winning delivery,
        // which is what DuckDB's single row per identity is. `FINAL` resolves a partition at a time
        // (`do_not_merge_across_partitions_select_final`), and every delivery of a log record lands in one
        // partition (`OTEL_LOGS_PARTITION`, by the times its identity covers), so a delivery a re-derivation has
        // re-keyed or cleared is superseded before its key is asked. The record bound is `ORDER BY ... LIMIT`,
        // which ClickHouse answers keeping the top rows alone - 14 MB at a million records under one key -
        // and the running total is a window over those: a window over every keyed row buffered the rows with
        // their messages, 610 MB for 200 MB of frames. The second pass is the right side of the join, which
        // ClickHouse holds in memory, so it is narrowed to the chosen identities - by `GLOBAL IN`: both sides read
        // the `Distributed` table, where a plain `IN` is refused (`distributed_product_mode = deny`), and the
        // chosen identities are a bounded set the initiator sends each shard once.
        Backend::Clickhouse => {
            values.insert(0, key.clone());
            values.extend([
                key,
                QueryValue::String(project_id.to_string()),
                QueryValue::String(trace_id.to_string()),
            ]);
            format!(
                "WITH chosen AS (\
                   SELECT frame_trace_id AS trace_id, frame_span_id AS span_id, timestamp_us, timestamp, \
                          log_digest, ordinal, sum(bytes) OVER ({window}) AS running \
                   FROM (SELECT assumeNotNull(trace_id) AS frame_trace_id, \
                                assumeNotNull(span_id) AS frame_span_id, \
                                toInt64(toUnixTimestamp64Micro(timestamp)) AS timestamp_us, timestamp, log_digest, \
                                ordinal, length(messages) AS bytes \
                         FROM otel_logs FINAL \
                         WHERE frame_key = toUInt128(?) AND {conditions} \
                         ORDER BY timestamp, log_digest, ordinal LIMIT {limit})) \
                 SELECT c.trace_id AS trace_id, c.span_id AS span_id, c.timestamp_us AS timestamp_us, \
                        c.log_digest AS log_digest, c.ordinal AS ordinal, \
                        if(c.running <= {max_bytes}, f.messages, NULL) AS messages \
                 FROM chosen AS c \
                 LEFT JOIN (SELECT log_digest, ordinal, messages FROM otel_logs FINAL \
                            WHERE frame_key = toUInt128(?) AND project_id = ? AND trace_id = ? \
                              AND (log_digest, ordinal) GLOBAL IN \
                                  (SELECT log_digest, ordinal FROM chosen WHERE running <= {max_bytes})) AS f \
                   ON f.log_digest = c.log_digest AND f.ordinal = c.ordinal \
                 ORDER BY c.timestamp, c.log_digest, c.ordinal"
            )
        }
    };
    ParameterizedQuery::new(sql, values)
}
