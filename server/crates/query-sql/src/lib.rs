//! Shared analytical SQL rendering vocabulary.
//!
//! Queries in this crate target the analytical backends: DuckDB and ClickHouse.

/// A trace's canonical session, as an aggregate over its rows that carry a session: the session of its root - a span
/// with no parent - and, for a trace whose stored rows have no root with a session, the session of its earliest such
/// span. The root is the trace's own claim, made once; a child produced on another host can start before its parent,
/// so the earliest span alone let a child's session overrule the root's. Every read, live event, fence and deletion
/// resolves sessions through this one rule (`docs/engineering/write-protocol.md`, section 6).
macro_rules! duckdb_canonical_session {
    () => {
        duckdb_canonical_session!("")
    };
    ($t:literal) => {
        concat!(
            "coalesce(arg_min(",
            $t,
            "session_id, (",
            $t,
            "timestamp_start, ",
            $t,
            "span_id)) \
               FILTER (WHERE coalesce(",
            $t,
            "parent_span_id, '') = '' \
                 AND ",
            $t,
            "session_id IS NOT NULL AND ",
            $t,
            "session_id != ''), \
             arg_min(",
            $t,
            "session_id, (",
            $t,
            "timestamp_start, ",
            $t,
            "span_id)) \
               FILTER (WHERE ",
            $t,
            "session_id IS NOT NULL AND ",
            $t,
            "session_id != ''))"
        )
    };
}

/// [`duckdb_canonical_session`] in ClickHouse, for rows already restricted to those carrying a session: a `String`, as
/// the membership reads decode it. `nullable` is the form for an aggregate over every row of a trace, NULL - as on
/// DuckDB - for a trace no row of which carries a session.
macro_rules! clickhouse_canonical_session {
    () => {
        clickhouse_canonical_session!("")
    };
    (nullable $t:literal) => {
        concat!("nullIf(", clickhouse_canonical_session!($t), ", '')")
    };
    ($t:literal) => {
        concat!(
            "if(countIf(ifNull(",
            $t,
            "parent_span_id, '') = '' AND ifNull(",
            $t,
            "session_id, '') != '') > 0, \
               argMinIf(assumeNotNull(",
            $t,
            "session_id), (",
            $t,
            "timestamp_start, ",
            $t,
            "span_id), \
                 ifNull(",
            $t,
            "parent_span_id, '') = '' AND ifNull(",
            $t,
            "session_id, '') != ''), \
               argMinIf(assumeNotNull(",
            $t,
            "session_id), (",
            $t,
            "timestamp_start, ",
            $t,
            "span_id), \
                 ifNull(",
            $t,
            "session_id, '') != ''))"
        )
    };
}

/// Typed analytical statements and their migration registry.
pub mod analytics;
/// Strict content confirmation reads for staged signal deliveries.
pub mod confirmations;
/// Dialect-specific SQL for displayed values.
pub mod display;
/// Typed tenant-scoped mutations for analytical stores.
pub mod dml;
/// DuckDB reads keyed by one identity column, answered from its index.
pub mod keyed;
/// The media references of stored log rows.
pub mod log_media;
/// Typed OTLP log reads.
pub mod logs;
/// Typed message-context and message-feed statements.
pub mod messages;
/// Typed metric reads and aggregates.
pub mod metrics;
/// Order-clause rendering.
pub mod order;
/// The keyed reads a delta-exporting producer's request view composes from.
pub mod request_context;
pub mod request_frames;
/// Typed three-valued chronological search queries.
pub mod search;
/// Typed project-statistics statements and bucket planning.
pub mod stats;
/// DuckDB's winning span revisions, as a condition on the row.
pub mod winners;

/// Analytical database backend identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Duckdb,
    Clickhouse,
}

impl Backend {
    /// Get the backend name.
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Duckdb => "duckdb",
            Backend::Clickhouse => "clickhouse",
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}
