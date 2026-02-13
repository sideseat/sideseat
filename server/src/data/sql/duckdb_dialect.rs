//! DuckDB SQL dialect implementation

use super::SqlDialect;

/// DuckDB SQL dialect
pub struct DuckdbDialect;

impl SqlDialect for DuckdbDialect {
    fn name(&self) -> &'static str {
        "duckdb"
    }

    fn placeholder(&self, _index: usize) -> String {
        "?".to_string()
    }

    fn array_contains(&self, array_col: &str, _param_idx: usize) -> String {
        format!("array_contains({}, ?)", array_col)
    }

    fn array_flatten(&self, col: &str) -> String {
        format!("UNNEST({})", col)
    }

    fn timestamp_to_micros(&self, col: &str) -> String {
        format!("EPOCH_US({})", col)
    }

    fn duration_ms(&self, start: &str, end: &str) -> String {
        format!("DATE_DIFF('millisecond', {}, {})", start, end)
    }

    fn cast_to_json(&self, col: &str) -> String {
        format!("{}::JSON", col)
    }

    fn cast_to_string(&self, col: &str) -> String {
        format!("{}::VARCHAR", col)
    }

    fn now_utc(&self) -> &'static str {
        "NOW()"
    }

    fn order_by_with_nulls(&self, col: &str, desc: bool, nulls_last: bool) -> String {
        let dir = if desc { "DESC" } else { "ASC" };
        let nulls = if nulls_last {
            "NULLS LAST"
        } else {
            "NULLS FIRST"
        };
        format!("{} {} {}", col, dir, nulls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder() {
        let dialect = DuckdbDialect;
        assert_eq!(dialect.placeholder(1), "?");
        assert_eq!(dialect.placeholder(5), "?");
    }

    #[test]
    fn test_array_contains() {
        let dialect = DuckdbDialect;
        assert_eq!(dialect.array_contains("tags", 1), "array_contains(tags, ?)");
    }

    #[test]
    fn test_timestamp_to_micros() {
        let dialect = DuckdbDialect;
        assert_eq!(
            dialect.timestamp_to_micros("timestamp_start"),
            "EPOCH_US(timestamp_start)"
        );
    }

    #[test]
    fn test_duration_ms() {
        let dialect = DuckdbDialect;
        assert_eq!(
            dialect.duration_ms("start_time", "end_time"),
            "DATE_DIFF('millisecond', start_time, end_time)"
        );
    }

    #[test]
    fn test_order_by_with_nulls() {
        let dialect = DuckdbDialect;
        assert_eq!(
            dialect.order_by_with_nulls("timestamp", true, true),
            "timestamp DESC NULLS LAST"
        );
        assert_eq!(
            dialect.order_by_with_nulls("name", false, false),
            "name ASC NULLS FIRST"
        );
    }
}
