//! Query filter types

use serde::{Deserialize, Serialize};

/// Filter for trace queries
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraceFilter {
    /// Filter by service name
    pub service_name: Option<String>,

    /// Filter by detected framework
    pub framework: Option<String>,

    /// Filter by minimum start time (nanoseconds)
    pub start_time_ns: Option<i64>,

    /// Filter by maximum start time (nanoseconds)
    pub end_time_ns: Option<i64>,

    /// Filter for traces with errors
    pub has_errors: Option<bool>,

    /// Filter by agent name
    pub agent_name: Option<String>,

    /// Filter by minimum duration (nanoseconds)
    pub min_duration_ns: Option<i64>,

    /// Filter by maximum duration (nanoseconds)
    pub max_duration_ns: Option<i64>,

    /// Text search in trace/span names
    pub search: Option<String>,

    /// EAV attribute filters
    #[serde(default)]
    pub attributes: Vec<AttributeFilter>,
}

/// Attribute filter for EAV-based filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeFilter {
    /// Attribute key name
    pub key: String,
    /// Filter operator
    pub op: FilterOperator,
    /// Filter value
    pub value: serde_json::Value,
}

/// Filter operators for attribute queries
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FilterOperator {
    /// Equality (=)
    #[default]
    Eq,
    /// Not equal (!=)
    Ne,
    /// Greater than (>)
    Gt,
    /// Less than (<)
    Lt,
    /// Greater than or equal (>=)
    Gte,
    /// Less than or equal (<=)
    Lte,
    /// Contains substring (LIKE %value%)
    Contains,
    /// Starts with (LIKE value%)
    StartsWith,
    /// In list (IN (...))
    In,
    /// Is null
    IsNull,
    /// Is not null
    IsNotNull,
}

impl FilterOperator {
    /// Convert to SQL operator for string values
    pub fn to_sql_str(&self) -> &'static str {
        match self {
            FilterOperator::Eq => "=",
            FilterOperator::Ne => "!=",
            FilterOperator::Gt => ">",
            FilterOperator::Lt => "<",
            FilterOperator::Gte => ">=",
            FilterOperator::Lte => "<=",
            FilterOperator::Contains => "LIKE",
            FilterOperator::StartsWith => "LIKE",
            FilterOperator::In => "IN",
            FilterOperator::IsNull => "IS NULL",
            FilterOperator::IsNotNull => "IS NOT NULL",
        }
    }
}

/// Filter for span queries
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpanFilter {
    /// Filter by trace ID
    pub trace_id: Option<String>,

    /// Filter by parent span ID
    pub parent_span_id: Option<String>,

    /// Filter by service name
    pub service_name: Option<String>,

    /// Filter by detected framework
    pub framework: Option<String>,

    /// Filter by span category
    pub category: Option<String>,

    /// Filter by agent name
    pub agent_name: Option<String>,

    /// Filter by tool name
    pub tool_name: Option<String>,

    /// Filter by model name
    pub model: Option<String>,

    /// Filter by minimum start time
    pub start_time_ns: Option<i64>,

    /// Filter by maximum start time
    pub end_time_ns: Option<i64>,

    /// Filter by status code
    pub status_code: Option<i32>,

    /// Cursor timestamp for pagination
    #[serde(skip)]
    pub cursor_timestamp: Option<i64>,

    /// Cursor ID for pagination
    #[serde(skip)]
    pub cursor_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_filter_default() {
        let filter = TraceFilter::default();
        assert!(filter.service_name.is_none());
        assert!(filter.framework.is_none());
        assert!(filter.start_time_ns.is_none());
        assert!(filter.end_time_ns.is_none());
        assert!(filter.has_errors.is_none());
        assert!(filter.agent_name.is_none());
        assert!(filter.min_duration_ns.is_none());
        assert!(filter.max_duration_ns.is_none());
        assert!(filter.search.is_none());
    }

    #[test]
    fn test_trace_filter_serialization() {
        let filter = TraceFilter {
            service_name: Some("my-service".to_string()),
            framework: Some("langchain".to_string()),
            has_errors: Some(true),
            min_duration_ns: Some(1000000),
            search: Some("query".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_string(&filter).unwrap();
        let deserialized: TraceFilter = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.service_name, Some("my-service".to_string()));
        assert_eq!(deserialized.framework, Some("langchain".to_string()));
        assert_eq!(deserialized.has_errors, Some(true));
        assert_eq!(deserialized.min_duration_ns, Some(1000000));
        assert_eq!(deserialized.search, Some("query".to_string()));
    }

    #[test]
    fn test_trace_filter_partial_json() {
        let json = r#"{"service_name": "test-svc"}"#;
        let filter: TraceFilter = serde_json::from_str(json).unwrap();

        assert_eq!(filter.service_name, Some("test-svc".to_string()));
        assert!(filter.framework.is_none());
        assert!(filter.has_errors.is_none());
    }

    #[test]
    fn test_span_filter_default() {
        let filter = SpanFilter::default();
        assert!(filter.trace_id.is_none());
        assert!(filter.parent_span_id.is_none());
        assert!(filter.service_name.is_none());
        assert!(filter.framework.is_none());
        assert!(filter.category.is_none());
        assert!(filter.agent_name.is_none());
        assert!(filter.tool_name.is_none());
        assert!(filter.model.is_none());
        assert!(filter.status_code.is_none());
    }

    #[test]
    fn test_span_filter_serialization() {
        let filter = SpanFilter {
            trace_id: Some("trace123".to_string()),
            service_name: Some("my-service".to_string()),
            category: Some("llm".to_string()),
            model: Some("gpt-4".to_string()),
            status_code: Some(0),
            ..Default::default()
        };

        let json = serde_json::to_string(&filter).unwrap();
        let deserialized: SpanFilter = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.trace_id, Some("trace123".to_string()));
        assert_eq!(deserialized.service_name, Some("my-service".to_string()));
        assert_eq!(deserialized.category, Some("llm".to_string()));
        assert_eq!(deserialized.model, Some("gpt-4".to_string()));
        assert_eq!(deserialized.status_code, Some(0));
    }

    #[test]
    fn test_span_filter_with_time_range() {
        let filter = SpanFilter {
            start_time_ns: Some(1000000000),
            end_time_ns: Some(2000000000),
            ..Default::default()
        };

        let json = serde_json::to_string(&filter).unwrap();
        let deserialized: SpanFilter = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.start_time_ns, Some(1000000000));
        assert_eq!(deserialized.end_time_ns, Some(2000000000));
    }

    #[test]
    fn test_filter_clone() {
        let filter = TraceFilter {
            service_name: Some("service".to_string()),
            has_errors: Some(true),
            ..Default::default()
        };

        let cloned = filter.clone();
        assert_eq!(cloned.service_name, filter.service_name);
        assert_eq!(cloned.has_errors, filter.has_errors);
    }

    #[test]
    fn test_filter_operator_to_sql_str() {
        assert_eq!(FilterOperator::Eq.to_sql_str(), "=");
        assert_eq!(FilterOperator::Ne.to_sql_str(), "!=");
        assert_eq!(FilterOperator::Gt.to_sql_str(), ">");
        assert_eq!(FilterOperator::Lt.to_sql_str(), "<");
        assert_eq!(FilterOperator::Gte.to_sql_str(), ">=");
        assert_eq!(FilterOperator::Lte.to_sql_str(), "<=");
        assert_eq!(FilterOperator::Contains.to_sql_str(), "LIKE");
        assert_eq!(FilterOperator::StartsWith.to_sql_str(), "LIKE");
        assert_eq!(FilterOperator::In.to_sql_str(), "IN");
        assert_eq!(FilterOperator::IsNull.to_sql_str(), "IS NULL");
        assert_eq!(FilterOperator::IsNotNull.to_sql_str(), "IS NOT NULL");
    }

    #[test]
    fn test_filter_operator_default() {
        let op = FilterOperator::default();
        assert!(matches!(op, FilterOperator::Eq));
    }

    #[test]
    fn test_attribute_filter_serialization() {
        let filter = AttributeFilter {
            key: "user.id".to_string(),
            op: FilterOperator::Eq,
            value: serde_json::json!("user123"),
        };

        let json = serde_json::to_string(&filter).unwrap();
        let deserialized: AttributeFilter = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.key, "user.id");
        assert!(matches!(deserialized.op, FilterOperator::Eq));
        assert_eq!(deserialized.value, serde_json::json!("user123"));
    }

    #[test]
    fn test_attribute_filter_with_numeric_value() {
        let filter = AttributeFilter {
            key: "request.count".to_string(),
            op: FilterOperator::Gte,
            value: serde_json::json!(100),
        };

        let json = serde_json::to_string(&filter).unwrap();
        let deserialized: AttributeFilter = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.key, "request.count");
        assert!(matches!(deserialized.op, FilterOperator::Gte));
        assert_eq!(deserialized.value, serde_json::json!(100));
    }

    #[test]
    fn test_attribute_filter_with_array_value() {
        let filter = AttributeFilter {
            key: "status".to_string(),
            op: FilterOperator::In,
            value: serde_json::json!(["active", "pending"]),
        };

        let json = serde_json::to_string(&filter).unwrap();
        let deserialized: AttributeFilter = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.key, "status");
        assert!(matches!(deserialized.op, FilterOperator::In));
    }

    #[test]
    fn test_trace_filter_with_attributes() {
        let filter = TraceFilter {
            service_name: Some("my-service".to_string()),
            attributes: vec![AttributeFilter {
                key: "env".to_string(),
                op: FilterOperator::Eq,
                value: serde_json::json!("production"),
            }],
            ..Default::default()
        };

        let json = serde_json::to_string(&filter).unwrap();
        let deserialized: TraceFilter = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.attributes.len(), 1);
        assert_eq!(deserialized.attributes[0].key, "env");
    }

    #[test]
    fn test_filter_operator_serialization_snake_case() {
        let ops = vec![
            (FilterOperator::Eq, "\"eq\""),
            (FilterOperator::Ne, "\"ne\""),
            (FilterOperator::Gt, "\"gt\""),
            (FilterOperator::Lt, "\"lt\""),
            (FilterOperator::Gte, "\"gte\""),
            (FilterOperator::Lte, "\"lte\""),
            (FilterOperator::Contains, "\"contains\""),
            (FilterOperator::StartsWith, "\"starts_with\""),
            (FilterOperator::In, "\"in\""),
            (FilterOperator::IsNull, "\"is_null\""),
            (FilterOperator::IsNotNull, "\"is_not_null\""),
        ];

        for (op, expected_json) in ops {
            let json = serde_json::to_string(&op).unwrap();
            assert_eq!(json, expected_json, "Failed for {:?}", op);
        }
    }
}
