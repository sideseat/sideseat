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
}
