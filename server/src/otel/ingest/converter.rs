//! OTLP proto to internal span conversion

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{InstrumentationScope, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use super::validator::truncate_string;
use crate::otel::error::OtelError;
use crate::otel::normalize::{
    DetectorRegistry, NormalizedSpan, SpanCategory, SpanEvent, extract_common_fields,
};

// Maximum lengths for string fields to prevent memory exhaustion
const MAX_SPAN_NAME_LEN: usize = 1024;
const MAX_SERVICE_NAME_LEN: usize = 256;
const MAX_ATTRIBUTE_KEY_LEN: usize = 256;
const MAX_ATTRIBUTE_VALUE_LEN: usize = 8192;
const MAX_ATTRIBUTES_JSON_LEN: usize = 65536;
const MAX_RECURSION_DEPTH: usize = 32;

/// Convert HTTP trace request body to normalized spans
pub fn convert_traces_request(
    body: &[u8],
    content_type: &str,
) -> Result<Vec<NormalizedSpan>, OtelError> {
    let request = if content_type.contains("json") {
        serde_json::from_slice::<ExportTraceServiceRequest>(body)
            .map_err(|e| OtelError::ParseError(format!("Invalid JSON: {}", e)))?
    } else {
        use prost::Message;
        ExportTraceServiceRequest::decode(body)
            .map_err(|e| OtelError::ParseError(format!("Invalid protobuf: {}", e)))?
    };

    let registry = DetectorRegistry::new();
    Ok(convert_otlp_spans(request, &registry))
}

/// Convert OTLP ExportTraceServiceRequest to normalized spans
pub fn convert_otlp_spans(
    request: ExportTraceServiceRequest,
    registry: &DetectorRegistry,
) -> Vec<NormalizedSpan> {
    let mut spans = Vec::new();

    for resource_span in request.resource_spans {
        let resource = resource_span.resource.unwrap_or_default();
        let resource_attrs = extract_resource_attributes(&resource);

        for scope_span in resource_span.scope_spans {
            let scope = scope_span.scope.unwrap_or_default();

            for otlp_span in scope_span.spans {
                let mut normalized = convert_single_span(&otlp_span, &resource, &scope);
                normalized.resource_attributes_json = Some(resource_attrs.clone());

                // Detect framework and extract fields
                let detection = registry.process(&otlp_span, &resource, &scope);
                normalized.detected_framework = detection.framework.as_str().to_string();
                normalized.detected_category = Some(category_to_string(detection.category));

                if let Some(extractor) = detection.extractor {
                    extractor.extract(&otlp_span, &mut normalized);
                }

                // Apply common cross-framework normalization
                extract_common_fields(&otlp_span, &resource, &mut normalized);

                spans.push(normalized);
            }
        }
    }

    spans
}

/// Convert a single OTLP span to NormalizedSpan
fn convert_single_span(
    span: &OtlpSpan,
    resource: &Resource,
    scope: &InstrumentationScope,
) -> NormalizedSpan {
    let mut normalized = NormalizedSpan::default();

    // Core identification
    normalized.trace_id = hex::encode(&span.trace_id);
    normalized.span_id = hex::encode(&span.span_id);
    normalized.parent_span_id =
        if span.parent_span_id.is_empty() { None } else { Some(hex::encode(&span.parent_span_id)) };

    // Timing
    normalized.start_time_unix_nano = span.start_time_unix_nano as i64;
    normalized.end_time_unix_nano =
        if span.end_time_unix_nano > 0 { Some(span.end_time_unix_nano as i64) } else { None };
    normalized.duration_ns =
        normalized.end_time_unix_nano.map(|e| e.saturating_sub(normalized.start_time_unix_nano));

    // Span info - with length limits
    normalized.span_name = truncate_string(&span.name, MAX_SPAN_NAME_LEN);
    normalized.span_kind = span.kind as i8;
    normalized.status_code = span.status.as_ref().map(|s| s.code as i8).unwrap_or(0);
    normalized.status_message = span.status.as_ref().and_then(|s| {
        if s.message.is_empty() {
            None
        } else {
            Some(truncate_string(&s.message, MAX_ATTRIBUTE_VALUE_LEN))
        }
    });

    // Resource info - with length limits
    for attr in &resource.attributes {
        match attr.key.as_str() {
            "service.name" => {
                if let Some(v) = get_string_value(&attr.value) {
                    normalized.service_name = truncate_string(&v, MAX_SERVICE_NAME_LEN);
                }
            }
            "service.version" => {
                normalized.service_version = get_string_value(&attr.value)
                    .map(|v| truncate_string(&v, MAX_ATTRIBUTE_VALUE_LEN))
            }
            "telemetry.sdk.name" => {
                normalized.sdk_name = get_string_value(&attr.value)
                    .map(|v| truncate_string(&v, MAX_ATTRIBUTE_VALUE_LEN))
            }
            "telemetry.sdk.language" => {
                normalized.sdk_language = get_string_value(&attr.value)
                    .map(|v| truncate_string(&v, MAX_ATTRIBUTE_VALUE_LEN))
            }
            "server.address" => {
                normalized.server_address = get_string_value(&attr.value)
                    .map(|v| truncate_string(&v, MAX_ATTRIBUTE_VALUE_LEN))
            }
            "server.port" => {
                if let Some(v) = &attr.value
                    && let Some(any_value::Value::IntValue(i)) = &v.value
                {
                    normalized.server_port = Some(*i as i32);
                }
            }
            _ => {}
        }
    }

    // Scope info - with length limits
    normalized.scope_name = if scope.name.is_empty() {
        None
    } else {
        Some(truncate_string(&scope.name, MAX_ATTRIBUTE_VALUE_LEN))
    };
    normalized.scope_version = if scope.version.is_empty() {
        None
    } else {
        Some(truncate_string(&scope.version, MAX_ATTRIBUTE_VALUE_LEN))
    };

    // Serialize all span attributes to JSON with key/value length limits
    let attrs_map: serde_json::Map<String, serde_json::Value> = span
        .attributes
        .iter()
        .map(|a| {
            (
                truncate_string(&a.key, MAX_ATTRIBUTE_KEY_LEN),
                any_value_to_json_limited(&a.value, MAX_ATTRIBUTE_VALUE_LEN, 0),
            )
        })
        .collect();
    let attrs_json = serde_json::to_string(&attrs_map).unwrap_or_else(|_| "{}".to_string());
    normalized.attributes_json = truncate_string(&attrs_json, MAX_ATTRIBUTES_JSON_LEN);

    // Extract span events (limit to 100 events per span)
    normalized.events = span
        .events
        .iter()
        .take(100)
        .map(|e| {
            let event_attrs: serde_json::Map<String, serde_json::Value> = e
                .attributes
                .iter()
                .map(|a| {
                    (
                        truncate_string(&a.key, MAX_ATTRIBUTE_KEY_LEN),
                        any_value_to_json_limited(&a.value, MAX_ATTRIBUTE_VALUE_LEN, 0),
                    )
                })
                .collect();
            let event_attrs_json =
                serde_json::to_string(&event_attrs).unwrap_or_else(|_| "{}".to_string());
            let event_attrs_json = truncate_string(&event_attrs_json, MAX_ATTRIBUTES_JSON_LEN);

            // Content preview is first 200 chars of attributes_json
            let content_preview = if event_attrs_json.len() > 2 {
                Some(truncate_string(&event_attrs_json, 200))
            } else {
                None
            };

            SpanEvent {
                span_id: normalized.span_id.clone(),
                trace_id: normalized.trace_id.clone(),
                event_time_ns: e.time_unix_nano as i64,
                event_name: truncate_string(&e.name, MAX_SPAN_NAME_LEN),
                content_preview,
                attributes_json: event_attrs_json,
            }
        })
        .collect();

    normalized
}

/// Extract resource attributes as JSON string with length limits
fn extract_resource_attributes(resource: &Resource) -> String {
    let attrs_map: serde_json::Map<String, serde_json::Value> = resource
        .attributes
        .iter()
        .map(|a| {
            (
                truncate_string(&a.key, MAX_ATTRIBUTE_KEY_LEN),
                any_value_to_json_limited(&a.value, MAX_ATTRIBUTE_VALUE_LEN, 0),
            )
        })
        .collect();
    let json = serde_json::to_string(&attrs_map).unwrap_or_else(|_| "{}".to_string());
    truncate_string(&json, MAX_ATTRIBUTES_JSON_LEN)
}

/// Get string value from AnyValue
fn get_string_value(
    value: &Option<opentelemetry_proto::tonic::common::v1::AnyValue>,
) -> Option<String> {
    value.as_ref().and_then(|v| {
        if let Some(any_value::Value::StringValue(s)) = &v.value { Some(s.clone()) } else { None }
    })
}

/// Convert AnyValue to serde_json::Value with length and depth limits
fn any_value_to_json_limited(
    value: &Option<opentelemetry_proto::tonic::common::v1::AnyValue>,
    max_len: usize,
    depth: usize,
) -> serde_json::Value {
    // Prevent stack overflow from deeply nested structures
    if depth >= MAX_RECURSION_DEPTH {
        return serde_json::Value::String("[max depth exceeded]".to_string());
    }

    match value.as_ref().and_then(|v| v.value.as_ref()) {
        Some(any_value::Value::StringValue(s)) => {
            serde_json::Value::String(truncate_string(s, max_len))
        }
        Some(any_value::Value::IntValue(i)) => serde_json::json!(*i),
        Some(any_value::Value::DoubleValue(d)) => serde_json::json!(*d),
        Some(any_value::Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(any_value::Value::ArrayValue(arr)) => {
            // Limit array to first 100 elements to prevent abuse
            let values: Vec<serde_json::Value> = arr
                .values
                .iter()
                .take(100)
                .map(|v| any_value_to_json_limited(&Some(v.clone()), max_len, depth + 1))
                .collect();
            serde_json::Value::Array(values)
        }
        Some(any_value::Value::KvlistValue(kv)) => {
            // Limit to first 100 key-value pairs
            let map: serde_json::Map<String, serde_json::Value> = kv
                .values
                .iter()
                .take(100)
                .map(|attr| {
                    (
                        truncate_string(&attr.key, MAX_ATTRIBUTE_KEY_LEN),
                        any_value_to_json_limited(&attr.value, max_len, depth + 1),
                    )
                })
                .collect();
            serde_json::Value::Object(map)
        }
        Some(any_value::Value::BytesValue(b)) => {
            use base64::Engine;
            // Limit bytes to 64KB before base64 encoding
            let limited = if b.len() > 65536 { &b[..65536] } else { b };
            serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(limited))
        }
        None => serde_json::Value::Null,
    }
}

/// Convert SpanCategory to string
fn category_to_string(category: SpanCategory) -> String {
    match category {
        SpanCategory::Agent => "agent".to_string(),
        SpanCategory::Llm => "llm".to_string(),
        SpanCategory::Tool => "tool".to_string(),
        SpanCategory::Chain => "chain".to_string(),
        SpanCategory::Retriever => "retriever".to_string(),
        SpanCategory::Embedding => "embedding".to_string(),
        SpanCategory::Memory => "memory".to_string(),
        SpanCategory::Unknown => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, ArrayValue, KeyValue, KeyValueList};

    fn make_string_value(s: &str) -> Option<AnyValue> {
        Some(AnyValue { value: Some(any_value::Value::StringValue(s.to_string())) })
    }

    fn make_int_value(i: i64) -> Option<AnyValue> {
        Some(AnyValue { value: Some(any_value::Value::IntValue(i)) })
    }

    fn make_double_value(d: f64) -> Option<AnyValue> {
        Some(AnyValue { value: Some(any_value::Value::DoubleValue(d)) })
    }

    fn make_bool_value(b: bool) -> Option<AnyValue> {
        Some(AnyValue { value: Some(any_value::Value::BoolValue(b)) })
    }

    fn make_array_value(values: Vec<AnyValue>) -> Option<AnyValue> {
        Some(AnyValue { value: Some(any_value::Value::ArrayValue(ArrayValue { values })) })
    }

    fn make_kvlist_value(pairs: Vec<(&str, AnyValue)>) -> Option<AnyValue> {
        let values: Vec<KeyValue> = pairs
            .into_iter()
            .map(|(k, v)| KeyValue { key: k.to_string(), value: Some(v) })
            .collect();
        Some(AnyValue { value: Some(any_value::Value::KvlistValue(KeyValueList { values })) })
    }

    fn make_bytes_value(bytes: Vec<u8>) -> Option<AnyValue> {
        Some(AnyValue { value: Some(any_value::Value::BytesValue(bytes)) })
    }

    #[test]
    fn test_any_value_to_json_string() {
        let value = make_string_value("hello");
        let result = any_value_to_json_limited(&value, 1000, 0);
        assert_eq!(result, serde_json::Value::String("hello".to_string()));
    }

    #[test]
    fn test_any_value_to_json_int() {
        let value = make_int_value(42);
        let result = any_value_to_json_limited(&value, 1000, 0);
        assert_eq!(result, serde_json::json!(42));
    }

    #[test]
    fn test_any_value_to_json_double() {
        let value = make_double_value(1.5);
        let result = any_value_to_json_limited(&value, 1000, 0);
        assert_eq!(result, serde_json::json!(1.5));
    }

    #[test]
    fn test_any_value_to_json_bool() {
        let value = make_bool_value(true);
        let result = any_value_to_json_limited(&value, 1000, 0);
        assert_eq!(result, serde_json::Value::Bool(true));
    }

    #[test]
    fn test_any_value_to_json_null() {
        let result = any_value_to_json_limited(&None, 1000, 0);
        assert_eq!(result, serde_json::Value::Null);
    }

    #[test]
    fn test_any_value_to_json_empty_any_value() {
        let value = Some(AnyValue { value: None });
        let result = any_value_to_json_limited(&value, 1000, 0);
        assert_eq!(result, serde_json::Value::Null);
    }

    #[test]
    fn test_any_value_to_json_string_truncation() {
        let long_string = "a".repeat(100);
        let value = make_string_value(&long_string);
        let result = any_value_to_json_limited(&value, 10, 0);
        // Should truncate and add "..."
        if let serde_json::Value::String(s) = result {
            assert!(s.len() <= 13); // 10 chars + "..."
            assert!(s.ends_with("..."));
        } else {
            panic!("Expected string value");
        }
    }

    #[test]
    fn test_any_value_to_json_array() {
        let values = vec![
            AnyValue { value: Some(any_value::Value::IntValue(1)) },
            AnyValue { value: Some(any_value::Value::IntValue(2)) },
            AnyValue { value: Some(any_value::Value::IntValue(3)) },
        ];
        let value = make_array_value(values);
        let result = any_value_to_json_limited(&value, 1000, 0);
        assert_eq!(result, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn test_any_value_to_json_array_limit_100() {
        // Create array with 150 elements
        let values: Vec<AnyValue> =
            (0..150).map(|i| AnyValue { value: Some(any_value::Value::IntValue(i)) }).collect();
        let value = make_array_value(values);
        let result = any_value_to_json_limited(&value, 1000, 0);
        if let serde_json::Value::Array(arr) = result {
            assert_eq!(arr.len(), 100); // Should be limited to 100
        } else {
            panic!("Expected array value");
        }
    }

    #[test]
    fn test_any_value_to_json_kvlist() {
        let pairs = vec![
            ("key1", AnyValue { value: Some(any_value::Value::StringValue("value1".to_string())) }),
            ("key2", AnyValue { value: Some(any_value::Value::IntValue(42)) }),
        ];
        let value = make_kvlist_value(pairs);
        let result = any_value_to_json_limited(&value, 1000, 0);
        let expected = serde_json::json!({"key1": "value1", "key2": 42});
        assert_eq!(result, expected);
    }

    #[test]
    fn test_any_value_to_json_kvlist_limit_100() {
        // Create kvlist with 150 entries
        let pairs: Vec<(&str, AnyValue)> = (0..150)
            .map(|i| {
                // Leak the string to get a &'static str (only for test purposes)
                let key: &'static str = Box::leak(format!("key{}", i).into_boxed_str());
                (key, AnyValue { value: Some(any_value::Value::IntValue(i)) })
            })
            .collect();
        let value = make_kvlist_value(pairs);
        let result = any_value_to_json_limited(&value, 1000, 0);
        if let serde_json::Value::Object(map) = result {
            assert_eq!(map.len(), 100); // Should be limited to 100
        } else {
            panic!("Expected object value");
        }
    }

    #[test]
    fn test_any_value_to_json_bytes() {
        let value = make_bytes_value(vec![0x48, 0x65, 0x6c, 0x6c, 0x6f]); // "Hello"
        let result = any_value_to_json_limited(&value, 1000, 0);
        if let serde_json::Value::String(s) = result {
            // Should be base64 encoded
            use base64::Engine;
            let decoded = base64::engine::general_purpose::STANDARD.decode(&s).unwrap();
            assert_eq!(decoded, b"Hello");
        } else {
            panic!("Expected string value");
        }
    }

    #[test]
    fn test_any_value_to_json_bytes_limit_64kb() {
        // Create bytes larger than 64KB limit
        let large_bytes = vec![0u8; 100_000];
        let value = make_bytes_value(large_bytes);
        let result = any_value_to_json_limited(&value, 1000, 0);
        if let serde_json::Value::String(s) = result {
            use base64::Engine;
            let decoded = base64::engine::general_purpose::STANDARD.decode(&s).unwrap();
            assert_eq!(decoded.len(), 65536); // Should be limited to 64KB
        } else {
            panic!("Expected string value");
        }
    }

    #[test]
    fn test_any_value_to_json_max_recursion_depth() {
        // Test that recursion is limited at MAX_RECURSION_DEPTH (32)
        let value = make_string_value("test");
        let result = any_value_to_json_limited(&value, 1000, MAX_RECURSION_DEPTH);
        assert_eq!(result, serde_json::Value::String("[max depth exceeded]".to_string()));
    }

    #[test]
    fn test_any_value_to_json_nested_array_depth() {
        // Create nested arrays to test depth limiting
        fn make_nested_array(depth: usize) -> Option<AnyValue> {
            if depth == 0 {
                make_string_value("leaf")
            } else {
                let inner = AnyValue { value: make_nested_array(depth - 1).and_then(|v| v.value) };
                make_array_value(vec![inner])
            }
        }

        // 30 levels of nesting should work
        let nested_30 = make_nested_array(30);
        let result = any_value_to_json_limited(&nested_30, 1000, 0);
        // Should not hit max depth
        let json_str = result.to_string();
        assert!(!json_str.contains("[max depth exceeded]"));

        // 35 levels of nesting should hit the limit
        let nested_35 = make_nested_array(35);
        let result = any_value_to_json_limited(&nested_35, 1000, 0);
        let json_str = result.to_string();
        assert!(json_str.contains("[max depth exceeded]"));
    }

    #[test]
    fn test_any_value_to_json_nested_kvlist_depth() {
        // Create nested kvlist to test depth limiting
        fn make_nested_kvlist(depth: usize) -> Option<AnyValue> {
            if depth == 0 {
                make_string_value("leaf")
            } else {
                let inner = AnyValue { value: make_nested_kvlist(depth - 1).and_then(|v| v.value) };
                make_kvlist_value(vec![("nested", inner)])
            }
        }

        // 30 levels should work
        let nested_30 = make_nested_kvlist(30);
        let result = any_value_to_json_limited(&nested_30, 1000, 0);
        let json_str = result.to_string();
        assert!(!json_str.contains("[max depth exceeded]"));

        // 35 levels should hit the limit
        let nested_35 = make_nested_kvlist(35);
        let result = any_value_to_json_limited(&nested_35, 1000, 0);
        let json_str = result.to_string();
        assert!(json_str.contains("[max depth exceeded]"));
    }

    #[test]
    fn test_category_to_string() {
        assert_eq!(category_to_string(SpanCategory::Agent), "agent");
        assert_eq!(category_to_string(SpanCategory::Llm), "llm");
        assert_eq!(category_to_string(SpanCategory::Tool), "tool");
        assert_eq!(category_to_string(SpanCategory::Chain), "chain");
        assert_eq!(category_to_string(SpanCategory::Retriever), "retriever");
        assert_eq!(category_to_string(SpanCategory::Embedding), "embedding");
        assert_eq!(category_to_string(SpanCategory::Memory), "memory");
        assert_eq!(category_to_string(SpanCategory::Unknown), "unknown");
    }

    #[test]
    fn test_get_string_value_some() {
        let value = make_string_value("test");
        assert_eq!(get_string_value(&value), Some("test".to_string()));
    }

    #[test]
    fn test_get_string_value_none() {
        assert_eq!(get_string_value(&None), None);
    }

    #[test]
    fn test_get_string_value_wrong_type() {
        let value = make_int_value(42);
        assert_eq!(get_string_value(&value), None);
    }

    #[test]
    fn test_convert_single_span_basic() {
        use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

        let span = OtlpSpan {
            trace_id: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            span_id: vec![1, 2, 3, 4, 5, 6, 7, 8],
            parent_span_id: vec![],
            name: "test-span".to_string(),
            kind: 1, // INTERNAL
            start_time_unix_nano: 1000000000,
            end_time_unix_nano: 2000000000,
            attributes: vec![],
            status: None,
            ..Default::default()
        };

        let resource = Resource::default();
        let scope = InstrumentationScope::default();

        let normalized = convert_single_span(&span, &resource, &scope);

        assert_eq!(normalized.trace_id, "0102030405060708090a0b0c0d0e0f10");
        assert_eq!(normalized.span_id, "0102030405060708");
        assert!(normalized.parent_span_id.is_none());
        assert_eq!(normalized.span_name, "test-span");
        assert_eq!(normalized.span_kind, 1);
        assert_eq!(normalized.start_time_unix_nano, 1000000000);
        assert_eq!(normalized.end_time_unix_nano, Some(2000000000));
        assert_eq!(normalized.duration_ns, Some(1000000000));
    }

    #[test]
    fn test_convert_single_span_with_parent() {
        use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

        let span = OtlpSpan {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            parent_span_id: vec![3; 8],
            name: "child-span".to_string(),
            start_time_unix_nano: 1000,
            end_time_unix_nano: 2000,
            ..Default::default()
        };

        let normalized =
            convert_single_span(&span, &Resource::default(), &InstrumentationScope::default());

        assert_eq!(normalized.parent_span_id, Some("0303030303030303".to_string()));
    }

    #[test]
    fn test_convert_single_span_with_status() {
        use opentelemetry_proto::tonic::trace::v1::{Span as OtlpSpan, Status, status::StatusCode};

        let span = OtlpSpan {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            name: "error-span".to_string(),
            start_time_unix_nano: 1000,
            end_time_unix_nano: 2000,
            status: Some(Status {
                code: StatusCode::Error as i32,
                message: "Something went wrong".to_string(),
            }),
            ..Default::default()
        };

        let normalized =
            convert_single_span(&span, &Resource::default(), &InstrumentationScope::default());

        assert_eq!(normalized.status_code, StatusCode::Error as i8);
        assert_eq!(normalized.status_message, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_convert_single_span_with_resource_attrs() {
        use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

        let span = OtlpSpan {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            name: "test".to_string(),
            start_time_unix_nano: 1000,
            ..Default::default()
        };

        let resource = Resource {
            attributes: vec![
                KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue("my-service".to_string())),
                    }),
                },
                KeyValue {
                    key: "service.version".to_string(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue("1.0.0".to_string())),
                    }),
                },
            ],
            ..Default::default()
        };

        let normalized = convert_single_span(&span, &resource, &InstrumentationScope::default());

        assert_eq!(normalized.service_name, "my-service");
        assert_eq!(normalized.service_version, Some("1.0.0".to_string()));
    }

    #[test]
    fn test_convert_single_span_with_scope() {
        use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

        let span = OtlpSpan {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            name: "test".to_string(),
            start_time_unix_nano: 1000,
            ..Default::default()
        };

        let scope = InstrumentationScope {
            name: "opentelemetry.instrumentation.flask".to_string(),
            version: "0.40.0".to_string(),
            ..Default::default()
        };

        let normalized = convert_single_span(&span, &Resource::default(), &scope);

        assert_eq!(normalized.scope_name, Some("opentelemetry.instrumentation.flask".to_string()));
        assert_eq!(normalized.scope_version, Some("0.40.0".to_string()));
    }

    #[test]
    fn test_convert_single_span_with_attributes() {
        use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

        let span = OtlpSpan {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            name: "test".to_string(),
            start_time_unix_nano: 1000,
            attributes: vec![
                KeyValue {
                    key: "http.method".to_string(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue("GET".to_string())),
                    }),
                },
                KeyValue {
                    key: "http.status_code".to_string(),
                    value: Some(AnyValue { value: Some(any_value::Value::IntValue(200)) }),
                },
            ],
            ..Default::default()
        };

        let normalized =
            convert_single_span(&span, &Resource::default(), &InstrumentationScope::default());

        // Verify attributes are serialized to JSON
        let attrs: serde_json::Value = serde_json::from_str(&normalized.attributes_json).unwrap();
        assert_eq!(attrs["http.method"], "GET");
        assert_eq!(attrs["http.status_code"], 200);
    }

    #[test]
    fn test_convert_single_span_truncates_long_name() {
        use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

        let long_name = "x".repeat(2000);
        let span = OtlpSpan {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            name: long_name,
            start_time_unix_nano: 1000,
            ..Default::default()
        };

        let normalized =
            convert_single_span(&span, &Resource::default(), &InstrumentationScope::default());

        // MAX_SPAN_NAME_LEN is 1024, should be truncated with "..."
        assert!(normalized.span_name.len() <= 1027); // 1024 + "..."
        assert!(normalized.span_name.ends_with("..."));
    }

    #[test]
    fn test_extract_resource_attributes() {
        let resource = Resource {
            attributes: vec![
                KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue("test-service".to_string())),
                    }),
                },
                KeyValue {
                    key: "deployment.environment".to_string(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue("production".to_string())),
                    }),
                },
            ],
            ..Default::default()
        };

        let json_str = extract_resource_attributes(&resource);
        let attrs: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(attrs["service.name"], "test-service");
        assert_eq!(attrs["deployment.environment"], "production");
    }

    #[test]
    fn test_extract_resource_attributes_empty() {
        let resource = Resource::default();
        let json_str = extract_resource_attributes(&resource);
        assert_eq!(json_str, "{}");
    }
}
