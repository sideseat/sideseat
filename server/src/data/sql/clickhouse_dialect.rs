//! ClickHouse SQL dialect implementation

use super::SqlDialect;

/// ClickHouse SQL dialect
pub struct ClickhouseDialect;

impl SqlDialect for ClickhouseDialect {
    fn name(&self) -> &'static str {
        "clickhouse"
    }

    fn placeholder(&self, _index: usize) -> String {
        // ClickHouse uses ? for positional parameters
        "?".to_string()
    }

    fn array_contains(&self, array_col: &str, _param_idx: usize) -> String {
        format!("has({}, ?)", array_col)
    }

    fn array_flatten(&self, col: &str) -> String {
        format!("arrayJoin({})", col)
    }

    fn timestamp_to_micros(&self, col: &str) -> String {
        format!("toInt64(toUnixTimestamp64Micro({}))", col)
    }

    fn duration_ms(&self, start: &str, end: &str) -> String {
        format!("dateDiff('millisecond', {}, {})", start, end)
    }

    fn cast_to_json(&self, col: &str) -> String {
        // ClickHouse stores JSON as String
        col.to_string()
    }

    fn cast_to_string(&self, col: &str) -> String {
        format!("toString({})", col)
    }

    fn now_utc(&self) -> &'static str {
        "now64(6)"
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
        let dialect = ClickhouseDialect;
        assert_eq!(dialect.placeholder(1), "?");
        assert_eq!(dialect.placeholder(5), "?");
    }

    #[test]
    fn test_array_contains() {
        let dialect = ClickhouseDialect;
        assert_eq!(dialect.array_contains("tags", 1), "has(tags, ?)");
    }

    #[test]
    fn test_timestamp_to_micros() {
        let dialect = ClickhouseDialect;
        assert_eq!(
            dialect.timestamp_to_micros("timestamp_start"),
            "toInt64(toUnixTimestamp64Micro(timestamp_start))"
        );
    }

    #[test]
    fn test_duration_ms() {
        let dialect = ClickhouseDialect;
        assert_eq!(
            dialect.duration_ms("start_time", "end_time"),
            "dateDiff('millisecond', start_time, end_time)"
        );
    }

    #[test]
    fn test_array_flatten() {
        let dialect = ClickhouseDialect;
        assert_eq!(dialect.array_flatten("tags"), "arrayJoin(tags)");
    }
}
