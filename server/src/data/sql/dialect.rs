//! SQL dialect trait for multi-database support
//!
//! This trait defines the interface for generating database-specific SQL syntax.

/// SQL dialect trait for generating database-specific SQL
///
/// Different databases have different syntax for:
/// - Parameter placeholders (? vs $1)
/// - Array operations
/// - Timestamp handling
/// - Limit/offset clauses
/// - Type casting
pub trait SqlDialect: Send + Sync {
    /// Get the dialect name
    fn name(&self) -> &'static str;

    /// Generate a parameter placeholder for the given index (1-based)
    ///
    /// - SQLite/DuckDB: Always returns "?"
    /// - PostgreSQL: Returns "$1", "$2", etc.
    /// - ClickHouse: Returns "?"
    fn placeholder(&self, index: usize) -> String;

    /// Generate SQL for checking if an array contains a value
    ///
    /// - DuckDB: `array_contains(col, ?)`
    /// - PostgreSQL: `? = ANY(col)`
    /// - ClickHouse: `has(col, ?)`
    fn array_contains(&self, array_col: &str, param_idx: usize) -> String;

    /// Generate SQL to flatten an array column
    ///
    /// - DuckDB: `UNNEST(col)`
    /// - PostgreSQL: `UNNEST(col)`
    /// - ClickHouse: `arrayJoin(col)`
    fn array_flatten(&self, col: &str) -> String;

    /// Convert timestamp column to microseconds since epoch
    ///
    /// - DuckDB: `EPOCH_US(col)`
    /// - PostgreSQL: `(EXTRACT(EPOCH FROM col)::BIGINT * 1000000)`
    /// - ClickHouse: `toInt64(toUnixTimestamp64Micro(col))`
    fn timestamp_to_micros(&self, col: &str) -> String;

    /// Calculate duration in milliseconds between two timestamps
    ///
    /// - DuckDB: `DATE_DIFF('millisecond', start, end)`
    /// - PostgreSQL: `(EXTRACT(EPOCH FROM (end - start)) * 1000)::BIGINT`
    /// - ClickHouse: `dateDiff('millisecond', start, end)`
    fn duration_ms(&self, start: &str, end: &str) -> String;

    /// Generate LIMIT/OFFSET clause
    ///
    /// Most databases use `LIMIT x OFFSET y`, but syntax may vary.
    fn limit_offset(&self, limit: u32, offset: u32) -> String {
        format!("LIMIT {} OFFSET {}", limit, offset)
    }

    /// Cast a column to JSON type
    ///
    /// - DuckDB: `col::JSON`
    /// - PostgreSQL: `col::JSONB`
    /// - ClickHouse: `col` (String stores JSON)
    fn cast_to_json(&self, col: &str) -> String;

    /// Cast a column to string type
    ///
    /// - DuckDB: `col::VARCHAR`
    /// - PostgreSQL: `col::TEXT`
    /// - ClickHouse: `toString(col)`
    fn cast_to_string(&self, col: &str) -> String;

    /// Generate SQL for current timestamp (UTC)
    ///
    /// - DuckDB: `NOW()`
    /// - PostgreSQL: `NOW() AT TIME ZONE 'UTC'`
    /// - ClickHouse: `now64(6)`
    fn now_utc(&self) -> &'static str;

    /// Generate ORDER BY clause with NULL handling
    ///
    /// - Most: `col DESC NULLS LAST`
    /// - SQLite: Doesn't support NULLS FIRST/LAST
    fn order_by_with_nulls(&self, col: &str, desc: bool, nulls_last: bool) -> String;
}
