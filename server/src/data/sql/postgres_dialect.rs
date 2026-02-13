//! PostgreSQL SQL dialect implementation

use super::SqlDialect;

/// PostgreSQL SQL dialect
pub struct PostgresDialect;

impl SqlDialect for PostgresDialect {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn placeholder(&self, index: usize) -> String {
        format!("${}", index)
    }

    fn array_contains(&self, array_col: &str, param_idx: usize) -> String {
        format!("${} = ANY({})", param_idx, array_col)
    }

    fn array_flatten(&self, col: &str) -> String {
        format!("UNNEST({})", col)
    }

    fn timestamp_to_micros(&self, col: &str) -> String {
        format!("(EXTRACT(EPOCH FROM {})::BIGINT * 1000000)", col)
    }

    fn duration_ms(&self, start: &str, end: &str) -> String {
        format!("(EXTRACT(EPOCH FROM ({} - {})) * 1000)::BIGINT", end, start)
    }

    fn cast_to_json(&self, col: &str) -> String {
        format!("{}::JSONB", col)
    }

    fn cast_to_string(&self, col: &str) -> String {
        format!("{}::TEXT", col)
    }

    fn now_utc(&self) -> &'static str {
        "NOW() AT TIME ZONE 'UTC'"
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
        let dialect = PostgresDialect;
        assert_eq!(dialect.placeholder(1), "$1");
        assert_eq!(dialect.placeholder(5), "$5");
    }

    #[test]
    fn test_array_contains() {
        let dialect = PostgresDialect;
        assert_eq!(dialect.array_contains("tags", 1), "$1 = ANY(tags)");
    }

    #[test]
    fn test_timestamp_to_micros() {
        let dialect = PostgresDialect;
        assert_eq!(
            dialect.timestamp_to_micros("created_at"),
            "(EXTRACT(EPOCH FROM created_at)::BIGINT * 1000000)"
        );
    }

    #[test]
    fn test_duration_ms() {
        let dialect = PostgresDialect;
        assert_eq!(
            dialect.duration_ms("start_time", "end_time"),
            "(EXTRACT(EPOCH FROM (end_time - start_time)) * 1000)::BIGINT"
        );
    }

    #[test]
    fn test_order_by_with_nulls() {
        let dialect = PostgresDialect;
        assert_eq!(
            dialect.order_by_with_nulls("timestamp", true, true),
            "timestamp DESC NULLS LAST"
        );
    }
}
