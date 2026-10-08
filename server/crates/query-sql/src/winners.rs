//! DuckDB's winning span revisions, as a condition on the row rather than a window over the table.
//!
//! A span identity keeps every revision it was delivered as, and a read answers with the latest - by
//! `ingested_at`, then by physical order. Computing that with `ROW_NUMBER() OVER (PARTITION BY identity ...)`
//! partitions every row of the table before any filter of the read can apply, and holds whole rows to do it:
//! on a million spans each list, feed and search read ran out of a 200 MB memory limit. So every row carries
//! `superseded_at` instead: the `ingested_at` of the revision that follows it in that order, `NULL` while it is
//! the latest. The write that stores a revision sets it, on the new rows and on the stored row they follow
//! (the DuckDB adapter's span writer). A winner is then a condition any scan can apply as it reads, beside the
//! read's own conditions, and it reads only the columns the read names.
//!
//! The same column answers "the latest revision ingested before a watermark", which a traversal pins so that a
//! revision delivered after it began neither appears in it nor hides the revision it replaced: a row is that
//! winner when it was ingested before the watermark and the revision that followed it, if any, was not.
//!
//! Every identity-wide delete removes all of an identity's revisions together, so the chain of revisions an
//! identity keeps is always complete and the condition never loses a winner.

use crate::analytics::QueryValue;

/// The condition a current winner meets, for a relation over `otel_spans` (unqualified columns).
pub const DUCKDB_WINNER: &str = "superseded_at IS NULL";

/// The current winning revisions of every span identity.
pub const DUCKDB_WINNING_SPANS: &str = "(SELECT * FROM otel_spans WHERE superseded_at IS NULL)";

/// The condition the winner as of a watermark meets: ingested before it, and not followed by a revision
/// ingested before it. Binds the watermark twice, in epoch microseconds ([`as_of_values`]).
pub const DUCKDB_WINNER_AS_OF: &str = "EPOCH_US(ingested_at) < ?::BIGINT \
     AND (superseded_at IS NULL OR EPOCH_US(superseded_at) >= ?::BIGINT)";

/// The winning revisions as of a watermark, binding it as [`DUCKDB_WINNER_AS_OF`] does.
pub fn duckdb_winning_spans_as_of() -> String {
    format!("(SELECT * FROM otel_spans WHERE {DUCKDB_WINNER_AS_OF})")
}

/// The values [`DUCKDB_WINNER_AS_OF`] binds for `watermark_us`.
pub fn as_of_values(watermark_us: i64) -> [QueryValue; 2] {
    [
        QueryValue::Int64(watermark_us),
        QueryValue::Int64(watermark_us),
    ]
}

/// `DUCKDB_WINNER` or, given a watermark, `DUCKDB_WINNER_AS_OF` with its values.
pub fn duckdb_winner_condition(watermark_us: Option<i64>) -> (&'static str, Vec<QueryValue>) {
    match watermark_us {
        Some(watermark) => (DUCKDB_WINNER_AS_OF, as_of_values(watermark).to_vec()),
        None => (DUCKDB_WINNER, Vec::new()),
    }
}

/// [`DUCKDB_WINNING_SPANS`] or, given a watermark, [`duckdb_winning_spans_as_of`] with its values.
pub fn duckdb_winning_spans(watermark_us: Option<i64>) -> (String, Vec<QueryValue>) {
    match watermark_us {
        Some(watermark) => (
            duckdb_winning_spans_as_of(),
            as_of_values(watermark).to_vec(),
        ),
        None => (DUCKDB_WINNING_SPANS.to_string(), Vec::new()),
    }
}
