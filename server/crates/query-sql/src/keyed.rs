//! DuckDB reads keyed by one identity column, answered from that column's index.
//!
//! DuckDB reads through an ART index only when the scan carries exactly one filter: an equality or IN list on the
//! column of a single-column index. A compound index is never read for a scan, and a second predicate pushed into
//! the same scan - `project_id = ?` beside the key - turns it into a read of the whole table. Measured on DuckDB
//! 1.5 over a million spans, `project_id = ? AND (trace_id, span_id) IN (...)` scanned every row with or without
//! the compound indexes, while the keyed form below scanned the matching rows only. So the keyed rows are
//! materialised alone, which keeps every other condition out of the scan, and the rest of the query applies to
//! that small set. This is what keeps ingest-time lookups proportional to the batch rather than to the store.

/// The most keys one keyed read binds.
///
/// DuckDB keeps to the index while the rows it finds stay under `index_scan_max_count` (2048 by default) or a
/// thousandth of the table, and reads the whole table past that. A key matches about one row per revision, so a
/// longer list is read in chunks of this many and every chunk stays on the index.
pub const KEYED_CHUNK: usize = 512;

/// A relation over the rows of `table` whose `key` is one of `keys` placeholders, with `columns` and their
/// `keyed_rowid` - the tie-break a winner needs, since `rowid` does not survive the materialisation.
pub fn duckdb_keyed(table: &str, key: &str, columns: &str, keys: usize) -> String {
    format!(
        "(WITH keyed AS MATERIALIZED (SELECT {columns}, rowid AS keyed_rowid FROM {table} \
         WHERE {key} IN ({})) SELECT * FROM keyed)",
        std::iter::repeat_n("?", keys)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The distinct keys of `values`, in a stable order, for a keyed read's placeholders.
pub fn distinct_keys<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut keys: Vec<&str> = values.into_iter().collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_the_only_filter_inside_the_materialised_rows() {
        let sql = duckdb_keyed("otel_spans", "span_id", "project_id, span_id", 3);
        assert_eq!(
            sql,
            "(WITH keyed AS MATERIALIZED (SELECT project_id, span_id, rowid AS keyed_rowid FROM otel_spans \
             WHERE span_id IN (?, ?, ?)) SELECT * FROM keyed)"
        );
        assert_eq!(distinct_keys(["b", "a", "b"]), vec!["a", "b"]);
    }
}
