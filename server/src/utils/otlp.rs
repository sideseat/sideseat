//! OTLP utility functions
//!
//! Provides reusable functions for working with OTLP protobuf types:
//! - Project ID injection into resource attributes
//! - Attribute extraction and conversion
//! - Shared attribute keys for context extraction

use std::collections::HashMap;

use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use serde_json::Value as JsonValue;

pub const PROJECT_ID_ATTR: &str = "sideseat.project_id";

// ============================================================================
// SHARED ATTRIBUTE KEYS
// ============================================================================

/// Shared attribute keys for context extraction (used by both traces and metrics)
pub mod keys {
    pub const SERVICE_NAME: &str = "service.name";
    pub const SERVICE_VERSION: &str = "service.version";
    pub const SERVICE_NAMESPACE: &str = "service.namespace";
    pub const SERVICE_INSTANCE_ID: &str = "service.instance.id";
    pub const DEPLOYMENT_ENV: &str = "deployment.environment";
    pub const DEPLOYMENT_ENV_NAME: &str = "deployment.environment.name";
    pub const SESSION_ID: &str = "session.id";
    pub const USER_ID: &str = "user.id";
    pub const ENDUSER_ID: &str = "enduser.id";
}

// ============================================================================
// CONTEXT EXTRACTION HELPERS
// ============================================================================

/// Convert HashMap<String, String> to JsonValue object
pub fn attrs_to_json(attrs: &HashMap<String, String>) -> JsonValue {
    let map: serde_json::Map<String, JsonValue> = attrs
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::json!(v)))
        .collect();
    JsonValue::Object(map)
}

/// Extract session_id from attributes
pub fn get_session_id(attrs: &HashMap<String, String>) -> Option<String> {
    attrs.get(keys::SESSION_ID).cloned()
}

/// Extract user_id from attributes (tries user.id then enduser.id)
pub fn get_user_id(attrs: &HashMap<String, String>) -> Option<String> {
    attrs
        .get(keys::USER_ID)
        .or_else(|| attrs.get(keys::ENDUSER_ID))
        .cloned()
}

/// Extract environment from attributes
pub fn get_environment(attrs: &HashMap<String, String>) -> Option<String> {
    attrs
        .get(keys::DEPLOYMENT_ENV)
        .or_else(|| attrs.get(keys::DEPLOYMENT_ENV_NAME))
        .cloned()
}

// ============================================================================
// ATTRIBUTE EXTRACTION
// ============================================================================

/// Extract attributes from KeyValue array into HashMap
pub fn extract_attributes(attrs: &[KeyValue]) -> HashMap<String, String> {
    attrs
        .iter()
        .filter_map(|kv| {
            kv.value
                .as_ref()
                .map(|v| (kv.key.clone(), any_value_to_string(v)))
        })
        .collect()
}

/// Convert AnyValue to string representation
pub fn any_value_to_string(value: &AnyValue) -> String {
    match &value.value {
        Some(any_value::Value::StringValue(s)) => s.clone(),
        Some(any_value::Value::BoolValue(b)) => b.to_string(),
        Some(any_value::Value::IntValue(i)) => i.to_string(),
        Some(any_value::Value::DoubleValue(d)) => d.to_string(),
        Some(any_value::Value::ArrayValue(arr)) => {
            let values: Vec<String> = arr.values.iter().map(any_value_to_string).collect();
            serde_json::to_string(&values).unwrap_or_default()
        }
        Some(any_value::Value::KvlistValue(kvlist)) => {
            let map: HashMap<String, String> = kvlist
                .values
                .iter()
                .filter_map(|kv| {
                    kv.value
                        .as_ref()
                        .map(|v| (kv.key.clone(), any_value_to_string(v)))
                })
                .collect();
            serde_json::to_string(&map).unwrap_or_default()
        }
        Some(any_value::Value::BytesValue(b)) => hex::encode(b),
        None => String::new(),
    }
}

// ============================================================================
// PROJECT ID INJECTION
// ============================================================================

/// Create a KeyValue attribute for project_id
pub fn make_project_id_attr(project_id: &str) -> KeyValue {
    KeyValue {
        key: PROJECT_ID_ATTR.to_string(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(project_id.to_string())),
        }),
    }
}

/// Inject project_id into resource attributes for traces
pub fn inject_project_id_traces(request: &mut ExportTraceServiceRequest, project_id: &str) {
    let attr = make_project_id_attr(project_id);
    for resource_spans in &mut request.resource_spans {
        if let Some(ref mut resource) = resource_spans.resource {
            resource.attributes.push(attr.clone());
        }
    }
}

/// Inject project_id into resource attributes for metrics
pub fn inject_project_id_metrics(request: &mut ExportMetricsServiceRequest, project_id: &str) {
    let attr = make_project_id_attr(project_id);
    for resource_metrics in &mut request.resource_metrics {
        if let Some(ref mut resource) = resource_metrics.resource {
            resource.attributes.push(attr.clone());
        }
    }
}

// ============================================================================
// JSON-PRESERVING ATTRIBUTE EXTRACTION
// ============================================================================

/// Convert AnyValue to JSON value (preserves native types)
pub fn any_value_to_json(value: &AnyValue) -> JsonValue {
    match &value.value {
        Some(any_value::Value::StringValue(s)) => serde_json::json!(s),
        Some(any_value::Value::BoolValue(b)) => serde_json::json!(b),
        Some(any_value::Value::IntValue(i)) => serde_json::json!(i),
        Some(any_value::Value::DoubleValue(d)) => serde_json::json!(d),
        Some(any_value::Value::ArrayValue(arr)) => {
            serde_json::json!(arr.values.iter().map(any_value_to_json).collect::<Vec<_>>())
        }
        Some(any_value::Value::KvlistValue(kvlist)) => {
            let map: serde_json::Map<String, JsonValue> = kvlist
                .values
                .iter()
                .filter_map(|kv| {
                    kv.value
                        .as_ref()
                        .map(|v| (kv.key.clone(), any_value_to_json(v)))
                })
                .collect();
            JsonValue::Object(map)
        }
        Some(any_value::Value::BytesValue(b)) => serde_json::json!(hex::encode(b)),
        None => JsonValue::Null,
    }
}

/// Build JSON object from raw KeyValue attributes (preserves types)
pub fn build_attributes_json(attrs: &[KeyValue]) -> JsonValue {
    let map: serde_json::Map<String, JsonValue> = attrs
        .iter()
        .filter_map(|kv| {
            kv.value
                .as_ref()
                .map(|v| (kv.key.clone(), any_value_to_json(v)))
        })
        .collect();
    JsonValue::Object(map)
}

// ============================================================================
// PROJECT ID INJECTION
// ============================================================================

/// Inject project_id into resource attributes for logs
pub fn inject_project_id_logs(request: &mut ExportLogsServiceRequest, project_id: &str) {
    let attr = make_project_id_attr(project_id);
    for resource_logs in &mut request.resource_logs {
        if let Some(ref mut resource) = resource_logs.resource {
            resource.attributes.push(attr.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attrs_to_json_empty() {
        let attrs = HashMap::new();
        let json = attrs_to_json(&attrs);
        assert_eq!(json, JsonValue::Object(serde_json::Map::new()));
    }

    #[test]
    fn test_attrs_to_json_with_values() {
        let mut attrs = HashMap::new();
        attrs.insert("key1".to_string(), "value1".to_string());
        attrs.insert("key2".to_string(), "value2".to_string());

        let json = attrs_to_json(&attrs);
        let obj = json.as_object().unwrap();
        assert_eq!(obj.get("key1").unwrap(), "value1");
        assert_eq!(obj.get("key2").unwrap(), "value2");
    }

    #[test]
    fn test_get_session_id_present() {
        let mut attrs = HashMap::new();
        attrs.insert(keys::SESSION_ID.to_string(), "sess-123".to_string());

        assert_eq!(get_session_id(&attrs), Some("sess-123".to_string()));
    }

    #[test]
    fn test_get_session_id_absent() {
        let attrs = HashMap::new();
        assert_eq!(get_session_id(&attrs), None);
    }

    #[test]
    fn test_get_user_id_user_id() {
        let mut attrs = HashMap::new();
        attrs.insert(keys::USER_ID.to_string(), "user-123".to_string());

        assert_eq!(get_user_id(&attrs), Some("user-123".to_string()));
    }

    #[test]
    fn test_get_user_id_enduser_id_fallback() {
        let mut attrs = HashMap::new();
        attrs.insert(keys::ENDUSER_ID.to_string(), "enduser-456".to_string());

        assert_eq!(get_user_id(&attrs), Some("enduser-456".to_string()));
    }

    #[test]
    fn test_get_user_id_prefers_user_id() {
        let mut attrs = HashMap::new();
        attrs.insert(keys::USER_ID.to_string(), "user-123".to_string());
        attrs.insert(keys::ENDUSER_ID.to_string(), "enduser-456".to_string());

        assert_eq!(get_user_id(&attrs), Some("user-123".to_string()));
    }

    #[test]
    fn test_get_user_id_absent() {
        let attrs = HashMap::new();
        assert_eq!(get_user_id(&attrs), None);
    }

    #[test]
    fn test_get_environment_deployment_env() {
        let mut attrs = HashMap::new();
        attrs.insert(keys::DEPLOYMENT_ENV.to_string(), "production".to_string());

        assert_eq!(get_environment(&attrs), Some("production".to_string()));
    }

    #[test]
    fn test_get_environment_deployment_env_name_fallback() {
        let mut attrs = HashMap::new();
        attrs.insert(keys::DEPLOYMENT_ENV_NAME.to_string(), "staging".to_string());

        assert_eq!(get_environment(&attrs), Some("staging".to_string()));
    }

    #[test]
    fn test_get_environment_prefers_deployment_env() {
        let mut attrs = HashMap::new();
        attrs.insert(keys::DEPLOYMENT_ENV.to_string(), "production".to_string());
        attrs.insert(keys::DEPLOYMENT_ENV_NAME.to_string(), "staging".to_string());

        assert_eq!(get_environment(&attrs), Some("production".to_string()));
    }

    #[test]
    fn test_get_environment_absent() {
        let attrs = HashMap::new();
        assert_eq!(get_environment(&attrs), None);
    }

    // ================================================================
    // Regression: any_value_to_json (moved from persist.rs)
    // ================================================================

    fn make_any_value(value: any_value::Value) -> AnyValue {
        AnyValue { value: Some(value) }
    }

    #[test]
    fn test_any_value_to_json_string() {
        let av = make_any_value(any_value::Value::StringValue("hello".to_string()));
        let json = any_value_to_json(&av);
        assert_eq!(json, serde_json::json!("hello"));
    }

    #[test]
    fn test_any_value_to_json_bool_true() {
        let av = make_any_value(any_value::Value::BoolValue(true));
        assert_eq!(any_value_to_json(&av), serde_json::json!(true));
    }

    #[test]
    fn test_any_value_to_json_bool_false() {
        let av = make_any_value(any_value::Value::BoolValue(false));
        assert_eq!(any_value_to_json(&av), serde_json::json!(false));
    }

    #[test]
    fn test_any_value_to_json_int() {
        let av = make_any_value(any_value::Value::IntValue(42));
        let json = any_value_to_json(&av);
        assert_eq!(json, serde_json::json!(42));
        assert!(
            json.is_i64(),
            "Int should be preserved as i64, not stringified"
        );
    }

    #[test]
    fn test_any_value_to_json_negative_int() {
        let av = make_any_value(any_value::Value::IntValue(-100));
        let json = any_value_to_json(&av);
        assert_eq!(json, serde_json::json!(-100));
    }

    #[test]
    fn test_any_value_to_json_double() {
        let av = make_any_value(any_value::Value::DoubleValue(3.14));
        let json = any_value_to_json(&av);
        assert_eq!(json, serde_json::json!(3.14));
        assert!(
            json.is_f64(),
            "Double should be preserved as f64, not stringified"
        );
    }

    #[test]
    fn test_any_value_to_json_none() {
        let av = AnyValue { value: None };
        assert_eq!(any_value_to_json(&av), JsonValue::Null);
    }

    #[test]
    fn test_any_value_to_json_bytes() {
        let av = make_any_value(any_value::Value::BytesValue(vec![0xde, 0xad, 0xbe, 0xef]));
        let json = any_value_to_json(&av);
        assert_eq!(json, serde_json::json!("deadbeef"));
    }

    #[test]
    fn test_any_value_to_json_empty_bytes() {
        let av = make_any_value(any_value::Value::BytesValue(vec![]));
        assert_eq!(any_value_to_json(&av), serde_json::json!(""));
    }

    #[test]
    fn test_any_value_to_json_array() {
        use opentelemetry_proto::tonic::common::v1::ArrayValue;
        let arr = ArrayValue {
            values: vec![
                make_any_value(any_value::Value::IntValue(1)),
                make_any_value(any_value::Value::StringValue("two".to_string())),
                make_any_value(any_value::Value::BoolValue(true)),
            ],
        };
        let av = make_any_value(any_value::Value::ArrayValue(arr));
        let json = any_value_to_json(&av);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], serde_json::json!(1));
        assert_eq!(arr[1], serde_json::json!("two"));
        assert_eq!(arr[2], serde_json::json!(true));
    }

    #[test]
    fn test_any_value_to_json_kvlist() {
        use opentelemetry_proto::tonic::common::v1::KeyValueList;
        let kvlist = KeyValueList {
            values: vec![
                KeyValue {
                    key: "name".to_string(),
                    value: Some(make_any_value(any_value::Value::StringValue(
                        "test".to_string(),
                    ))),
                },
                KeyValue {
                    key: "count".to_string(),
                    value: Some(make_any_value(any_value::Value::IntValue(5))),
                },
            ],
        };
        let av = make_any_value(any_value::Value::KvlistValue(kvlist));
        let json = any_value_to_json(&av);
        let obj = json.as_object().unwrap();
        assert_eq!(obj.get("name").unwrap(), &serde_json::json!("test"));
        assert_eq!(obj.get("count").unwrap(), &serde_json::json!(5));
    }

    #[test]
    fn test_any_value_to_json_preserves_types_vs_to_string() {
        // Regression: any_value_to_json must preserve native types,
        // unlike any_value_to_string which converts everything to String
        let av_int = make_any_value(any_value::Value::IntValue(42));
        let json = any_value_to_json(&av_int);
        let string = any_value_to_string(&av_int);

        assert!(json.is_i64(), "JSON should preserve int type");
        assert_eq!(string, "42", "String should be '42'");
        // They should NOT be equal representations
        assert_ne!(
            json,
            serde_json::json!("42"),
            "JSON int != JSON string '42'"
        );
    }

    // ================================================================
    // Regression: build_attributes_json (moved from persist.rs)
    // ================================================================

    #[test]
    fn test_build_attributes_json_empty() {
        let json = build_attributes_json(&[]);
        assert_eq!(json, JsonValue::Object(serde_json::Map::new()));
    }

    #[test]
    fn test_build_attributes_json_mixed_types() {
        let attrs = vec![
            KeyValue {
                key: "str_key".to_string(),
                value: Some(make_any_value(any_value::Value::StringValue(
                    "hello".to_string(),
                ))),
            },
            KeyValue {
                key: "int_key".to_string(),
                value: Some(make_any_value(any_value::Value::IntValue(42))),
            },
            KeyValue {
                key: "bool_key".to_string(),
                value: Some(make_any_value(any_value::Value::BoolValue(true))),
            },
            KeyValue {
                key: "float_key".to_string(),
                value: Some(make_any_value(any_value::Value::DoubleValue(1.5))),
            },
        ];
        let json = build_attributes_json(&attrs);
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 4);
        assert_eq!(obj.get("str_key").unwrap(), &serde_json::json!("hello"));
        assert_eq!(obj.get("int_key").unwrap(), &serde_json::json!(42));
        assert_eq!(obj.get("bool_key").unwrap(), &serde_json::json!(true));
        assert_eq!(obj.get("float_key").unwrap(), &serde_json::json!(1.5));
    }

    #[test]
    fn test_build_attributes_json_skips_none_values() {
        let attrs = vec![
            KeyValue {
                key: "present".to_string(),
                value: Some(make_any_value(any_value::Value::StringValue(
                    "yes".to_string(),
                ))),
            },
            KeyValue {
                key: "missing".to_string(),
                value: None,
            },
        ];
        let json = build_attributes_json(&attrs);
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        assert!(obj.contains_key("present"));
        assert!(!obj.contains_key("missing"));
    }

    #[test]
    fn test_build_attributes_json_nested_kvlist() {
        use opentelemetry_proto::tonic::common::v1::KeyValueList;
        let inner = KeyValueList {
            values: vec![KeyValue {
                key: "inner_key".to_string(),
                value: Some(make_any_value(any_value::Value::IntValue(99))),
            }],
        };
        let attrs = vec![KeyValue {
            key: "nested".to_string(),
            value: Some(make_any_value(any_value::Value::KvlistValue(inner))),
        }];
        let json = build_attributes_json(&attrs);
        let obj = json.as_object().unwrap();
        let nested = obj.get("nested").unwrap().as_object().unwrap();
        assert_eq!(nested.get("inner_key").unwrap(), &serde_json::json!(99));
    }

    #[test]
    fn test_build_attributes_json_vs_extract_attributes_consistency() {
        // Regression: build_attributes_json (JSON) and extract_attributes (HashMap<String,String>)
        // should produce results for the same keys, just with different value types
        let attrs = vec![
            KeyValue {
                key: "name".to_string(),
                value: Some(make_any_value(any_value::Value::StringValue(
                    "test".to_string(),
                ))),
            },
            KeyValue {
                key: "count".to_string(),
                value: Some(make_any_value(any_value::Value::IntValue(10))),
            },
        ];
        let json_result = build_attributes_json(&attrs);
        let string_result = extract_attributes(&attrs);

        let json_obj = json_result.as_object().unwrap();
        // Both should have the same keys
        assert_eq!(json_obj.len(), string_result.len());
        for key in string_result.keys() {
            assert!(json_obj.contains_key(key), "JSON missing key: {}", key);
        }
    }
}
