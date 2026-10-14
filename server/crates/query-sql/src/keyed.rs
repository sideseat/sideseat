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

/// The most bytes the bound values of one ClickHouse statement may take. The driver writes every bound value into
/// the statement's text, and ClickHouse refuses one past `max_query_size` (256 KiB by default); half of it leaves the
/// rest of the statement room. Identities are client strings of any length, so a count alone bounds nothing.
pub const BOUND_BYTES: usize = 128 * 1024;

/// What a value takes in a statement once written into it: at most twice its bytes, every character escaped, and
/// its quotes and separator.
pub trait BoundText {
    fn bound_bytes(&self) -> usize;
}

impl BoundText for str {
    fn bound_bytes(&self) -> usize {
        2 * self.len() + 4
    }
}

impl BoundText for String {
    fn bound_bytes(&self) -> usize {
        self.as_str().bound_bytes()
    }
}

impl BoundText for &str {
    fn bound_bytes(&self) -> usize {
        (**self).bound_bytes()
    }
}

impl BoundText for (String, String) {
    fn bound_bytes(&self) -> usize {
        self.0.bound_bytes() + self.1.bound_bytes()
    }
}

impl BoundText for (String, String, String) {
    fn bound_bytes(&self) -> usize {
        self.0.bound_bytes() + self.1.bound_bytes() + self.2.bound_bytes()
    }
}

/// A datapoint and its instant: the instant takes at most 32 bytes written out.
impl BoundText for (String, chrono::DateTime<chrono::Utc>) {
    fn bound_bytes(&self) -> usize {
        self.0.bound_bytes() + 32
    }
}

/// A log record's digest, ordinal and instant.
impl BoundText for (String, u32, Option<chrono::DateTime<chrono::Utc>>) {
    fn bound_bytes(&self) -> usize {
        self.0.bound_bytes() + 48
    }
}

/// `items` in consecutive chunks of at most `max_items`, each of whose values take at most [`BOUND_BYTES`] written
/// into a statement. A value larger than that alone is a chunk of its own.
pub fn bounded_chunks<T: BoundText>(items: &[T], max_items: usize) -> BoundedChunks<'_, T> {
    bounded_chunks_within(items, max_items, BOUND_BYTES)
}

/// [`bounded_chunks`] with a budget of `max_bytes`, for a statement that binds two lists and shares the budget
/// between them.
pub fn bounded_chunks_within<T: BoundText>(
    items: &[T],
    max_items: usize,
    max_bytes: usize,
) -> BoundedChunks<'_, T> {
    BoundedChunks {
        rest: items,
        max_items: max_items.max(1),
        max_bytes,
    }
}

/// The iterator [`bounded_chunks`] returns.
pub struct BoundedChunks<'a, T> {
    rest: &'a [T],
    max_items: usize,
    max_bytes: usize,
}

impl<'a, T: BoundText> Iterator for BoundedChunks<'a, T> {
    type Item = &'a [T];

    fn next(&mut self) -> Option<&'a [T]> {
        if self.rest.is_empty() {
            return None;
        }
        let mut taken = 0;
        let mut bytes = 0;
        while taken < self.rest.len() && taken < self.max_items {
            let next = self.rest[taken].bound_bytes();
            if taken > 0 && bytes + next > self.max_bytes {
                break;
            }
            bytes += next;
            taken += 1;
        }
        let (chunk, rest) = self.rest.split_at(taken);
        self.rest = rest;
        Some(chunk)
    }
}

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

    /// Chunks keep every value in order, at most a count of them, and at most the byte budget of their text; a
    /// value past the budget on its own is a chunk alone.
    #[test]
    fn chunks_are_bounded_by_count_and_by_bytes() {
        let short: Vec<String> = (0..1_100).map(|n| format!("{n:032x}")).collect();
        let chunks: Vec<&[String]> = bounded_chunks(&short, KEYED_CHUNK).collect();
        assert_eq!(
            chunks.iter().map(|c| c.len()).collect::<Vec<_>>(),
            vec![512, 512, 76]
        );
        let long: Vec<String> = (0..600).map(|n| format!("{n:0>1000}")).collect();
        let chunks: Vec<&[String]> = bounded_chunks(&long, KEYED_CHUNK).collect();
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.iter().map(BoundText::bound_bytes).sum::<usize>() <= BOUND_BYTES);
        }
        assert_eq!(chunks.concat(), long);
        let huge = vec!["a".to_string(), "b".repeat(BOUND_BYTES), "c".to_string()];
        let chunks: Vec<&[String]> = bounded_chunks(&huge, KEYED_CHUNK).collect();
        assert_eq!(
            chunks.iter().map(|c| c.len()).collect::<Vec<_>>(),
            vec![1, 1, 1]
        );
        assert_eq!(bounded_chunks::<String>(&[], KEYED_CHUNK).count(), 0);
    }

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
