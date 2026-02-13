//! Filter parsing
//!
//! Parses JSON filter definitions into Filter structs with validation.

use crate::api::types::ApiError;

use super::types::Filter;

/// Maximum size of filter JSON in bytes (64KB)
const MAX_FILTER_JSON_SIZE: usize = 64 * 1024;

/// Maximum number of filters allowed
const MAX_FILTERS: usize = 50;

/// Parse filters from JSON query param
///
/// Validates JSON size, parses into Filter structs, and validates columns.
pub fn parse_filters(json_str: &str, allowed_columns: &[&str]) -> Result<Vec<Filter>, ApiError> {
    if json_str.len() > MAX_FILTER_JSON_SIZE {
        return Err(ApiError::bad_request(
            "FILTER_JSON_TOO_LARGE",
            format!(
                "Filter JSON exceeds maximum size of {} bytes",
                MAX_FILTER_JSON_SIZE
            ),
        ));
    }

    let filters: Vec<Filter> = serde_json::from_str(json_str)
        .map_err(|e| ApiError::bad_request("INVALID_FILTER_JSON", e.to_string()))?;

    if filters.len() > MAX_FILTERS {
        return Err(ApiError::bad_request(
            "TOO_MANY_FILTERS",
            format!("Maximum {} filters allowed", MAX_FILTERS),
        ));
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
