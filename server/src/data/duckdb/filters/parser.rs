//! Filter parsing
//!
//! Parses JSON filter definitions into Filter structs with validation.

use super::types::{Filter, FilterError};

/// Maximum size of filter JSON in bytes (64KB)
const MAX_FILTER_JSON_SIZE: usize = 64 * 1024;

/// Maximum number of filters allowed
const MAX_FILTERS: usize = 50;

/// Parse filters from JSON query param
///
/// Validates JSON size, parses into Filter structs, and validates columns.
pub fn parse_filters(json_str: &str, allowed_columns: &[&str]) -> Result<Vec<Filter>, FilterError> {
    if json_str.len() > MAX_FILTER_JSON_SIZE {
        return Err(FilterError::TooLarge {
            message: format!(
                "Filter JSON exceeds maximum size of {} bytes",
                MAX_FILTER_JSON_SIZE
            ),
        });
    }

    let filters: Vec<Filter> =
        serde_json::from_str(json_str).map_err(|e| FilterError::Malformed {
            message: e.to_string(),
        })?;

    if filters.len() > MAX_FILTERS {
        return Err(FilterError::TooMany {
            message: format!("Maximum {} filters allowed", MAX_FILTERS),
        });
    }

    for filter in &filters {
        filter.validate(allowed_columns)?;
    }

    Ok(filters)
}

#[cfg(test)]
mod tests {
    use super::super::columns;
    use super::*;

    #[test]
    fn parse_filters_valid_json() {
        let json = r#"[
            {"type": "string", "column": "trace_id", "operator": "=", "value": "abc123"}
        ]"#;
        let result = parse_filters(json, columns::TRACE_FILTERABLE);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 1);
    }

    #[test]
    fn parse_filters_multiple() {
        let json = r#"[
            {"type": "string", "column": "trace_id", "operator": "=", "value": "abc"},
            {"type": "number", "column": "duration_ms", "operator": ">", "value": 100}
        ]"#;
        let result = parse_filters(json, columns::TRACE_FILTERABLE);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
    }

    /// The two limits are two remedies, so they must not share a code.
    ///
    /// `FilterError` was introduced to replace `ApiError` in this layer, and folding both refusals into
    /// `TooMany` changed what an oversized payload returns from `FILTER_JSON_TOO_LARGE` to
    /// `TOO_MANY_FILTERS` — telling a caller to send fewer clauses when the fix is to send less text. Both
    /// codes are asserted here because asserting only `is_err()` is what let the collapse through: every
    /// prior test on this path passed with one code doing both jobs.
    #[test]
    fn the_size_limit_and_the_count_limit_report_different_codes() {
        let oversized = format!(
            r#"[{{"type": "string", "column": "trace_id", "operator": "=", "value": "{}"}}]"#,
            "x".repeat(MAX_FILTER_JSON_SIZE)
        );
        assert_eq!(
            parse_filters(&oversized, columns::TRACE_FILTERABLE)
                .expect_err("a payload past the size limit is refused")
                .code(),
            "FILTER_JSON_TOO_LARGE"
        );

        // Comfortably inside the size limit, comfortably past the count limit - so the two cannot be
        // satisfied by one branch.
        let many = format!(
            "[{}]",
            (0..=MAX_FILTERS)
                .map(
                    |_| r#"{"type": "string", "column": "trace_id", "operator": "=", "value": "a"}"#
                )
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(
            many.len() < MAX_FILTER_JSON_SIZE,
            "the count case must not also trip the size limit"
        );
        assert_eq!(
            parse_filters(&many, columns::TRACE_FILTERABLE)
                .expect_err("a payload past the count limit is refused")
                .code(),
            "TOO_MANY_FILTERS"
        );
    }

    #[test]
    fn parse_filters_invalid_json() {
        let json = "not valid json";
        let result = parse_filters(json, columns::TRACE_FILTERABLE);
        assert!(result.is_err());
    }

    #[test]
    fn parse_filters_invalid_column() {
        let json = r#"[
            {"type": "string", "column": "invalid_column", "operator": "=", "value": "test"}
        ]"#;
        let result = parse_filters(json, columns::TRACE_FILTERABLE);
        assert!(result.is_err());
    }

    #[test]
    fn parse_filters_datetime() {
        let json = r#"[
            {"type": "datetime", "column": "start_time", "operator": ">=", "value": "2024-01-01T00:00:00Z"}
        ]"#;
        let result = parse_filters(json, columns::TRACE_FILTERABLE);
        assert!(result.is_ok());
    }

    #[test]
    fn parse_filters_string_options() {
        let json = r#"[
            {"type": "string_options", "column": "environment", "operator": "any of", "value": ["prod", "dev"]}
        ]"#;
        let result = parse_filters(json, columns::TRACE_FILTERABLE);
        assert!(result.is_ok());
    }

    #[test]
    fn parse_filters_null() {
        let json = r#"[
            {"type": "null", "column": "session_id", "operator": "is null"}
        ]"#;
        let result = parse_filters(json, columns::TRACE_FILTERABLE);
        assert!(result.is_ok());
    }
}
