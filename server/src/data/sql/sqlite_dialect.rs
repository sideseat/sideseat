//! SQLite SQL dialect implementation

use super::SqlDialect;

/// SQLite SQL dialect
pub struct SqliteDialect;

impl SqlDialect for SqliteDialect {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn placeholder(&self, _index: usize) -> String {
        "?".to_string()
    }

    fn array_contains(&self, array_col: &str, _param_idx: usize) -> String {
        // SQLite stores arrays as JSON text, use json_each to search
        format!(
            "EXISTS (SELECT 1 FROM json_each({}) WHERE value = ?)",
            array_col
        )
    }

    fn array_flatten(&self, col: &str) -> String {
        format!("json_each({})", col)
    }

    fn timestamp_to_micros(&self, col: &str) -> String {
        // SQLite stores timestamps as integers (microseconds)
        col.to_string()
    }

    fn duration_ms(&self, start: &str, end: &str) -> String {
        // SQLite timestamps are stored as microseconds
        format!("({} - {}) / 1000", end, start)
    }

    fn cast_to_json(&self, col: &str) -> String {
        // SQLite stores JSON as TEXT
        format!("json({})", col)
    }

    fn cast_to_string(&self, col: &str) -> String {
        format!("CAST({} AS TEXT)", col)
    }

    fn now_utc(&self) -> &'static str {
        // SQLite: return current timestamp as microseconds
        "CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER)"
    }

    fn order_by_with_nulls(&self, col: &str, desc: bool, nulls_last: bool) -> String {
        // SQLite doesn't support NULLS FIRST/LAST, emulate with CASE
        let dir = if desc { "DESC" } else { "ASC" };
        if nulls_last {
            format!(
                "CASE WHEN {} IS NULL THEN 1 ELSE 0 END, {} {}",
                col, col, dir
            )
        } else {
            format!(
                "CASE WHEN {} IS NULL THEN 0 ELSE 1 END, {} {}",
                col, col, dir
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder() {
        let dialect = SqliteDialect;
        assert_eq!(dialect.placeholder(1), "?");
        assert_eq!(dialect.placeholder(5), "?");
    }

    #[test]
    fn test_array_contains() {
        let dialect = SqliteDialect;
        assert_eq!(
            dialect.array_contains("tags", 1),
            "EXISTS (SELECT 1 FROM json_each(tags) WHERE value = ?)"
        );
    }

    #[test]
    fn test_duration_ms() {
        let dialect = SqliteDialect;
        assert_eq!(
            dialect.duration_ms("start_time", "end_time"),
            "(end_time - start_time) / 1000"
        );
    }

    #[test]
    fn test_order_by_with_nulls() {
        let dialect = SqliteDialect;
        assert_eq!(
            dialect.order_by_with_nulls("timestamp", true, true),
            "CASE WHEN timestamp IS NULL THEN 1 ELSE 0 END, timestamp DESC"
        );
        assert_eq!(
            dialect.order_by_with_nulls("name", false, false),
            "CASE WHEN name IS NULL THEN 0 ELSE 1 END, name ASC"
        );
    }
}
