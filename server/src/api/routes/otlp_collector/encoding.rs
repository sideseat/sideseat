//! OTLP content-type encoding and decoding
//!
//! Supports both protobuf (application/x-protobuf) and JSON (application/json) formats
//! per the OpenTelemetry Protocol specification.

use std::fmt;

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use prost::Message;
use serde::{Deserialize, Serialize};

/// Content type for OTLP requests/responses
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtlpContentType {
    Protobuf,
    Json,
}

impl OtlpContentType {
    /// Parse content type from HTTP headers.
    /// Defaults to protobuf if content type is missing or unrecognized.
    #[inline]
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let content_type = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if content_type.starts_with("application/json") {
            OtlpContentType::Json
        } else {
            OtlpContentType::Protobuf
        }
    }

    /// Get the content type header value for responses
    #[inline]
    pub fn as_header_value(self) -> &'static str {
        match self {
            OtlpContentType::Protobuf => "application/x-protobuf",
            OtlpContentType::Json => "application/json",
        }
    }

    #[inline]
    fn decode_error_message(self) -> &'static str {
        match self {
            OtlpContentType::Protobuf => "Failed to decode protobuf request",
            OtlpContentType::Json => "Failed to decode JSON request",
        }
    }
}

/// Decode an OTLP request from bytes based on content type
#[inline]
pub fn decode_request<T>(body: &Bytes, content_type: OtlpContentType) -> Result<T, DecodeError>
where
    T: Message + Default + for<'de> Deserialize<'de>,
{
    match content_type {
        OtlpContentType::Protobuf => {
            T::decode(body.as_ref()).map_err(|e| DecodeError::Protobuf(e.to_string()))
        }
        OtlpContentType::Json => {
            serde_json::from_slice(body.as_ref()).map_err(|e| DecodeError::Json(e.to_string()))
        }
    }
}

/// Encode an OTLP response to bytes based on content type
fn encode_response<T>(response: &T, content_type: OtlpContentType) -> Result<Vec<u8>, String>
where
    T: Message + Serialize,
{
    match content_type {
        OtlpContentType::Protobuf => Ok(response.encode_to_vec()),
        OtlpContentType::Json => serde_json::to_vec(response).map_err(|e| e.to_string()),
    }
}

/// Create a successful OTLP response with the correct content type
pub fn success_response<T>(response: &T, content_type: OtlpContentType) -> Response
where
    T: Message + Serialize,
{
    match encode_response(response, content_type) {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type.as_header_value())],
            bytes,
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to encode OTLP response");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "text/plain")],
                "Internal server error",
            )
                .into_response()
        }
    }
}

/// Error returned when decoding fails
#[derive(Debug)]
pub enum DecodeError {
    Protobuf(String),
    Json(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Protobuf(e) => write!(f, "protobuf decode error: {}", e),
            DecodeError::Json(e) => write!(f, "JSON decode error: {}", e),
        }
    }
}

impl std::error::Error for DecodeError {}

impl DecodeError {
    /// Create an error response for a decode failure.
    /// Internal error details are logged but not exposed to clients.
    pub fn into_response(self, content_type: OtlpContentType) -> Response {
        tracing::warn!(
            error = %self,
            content_type = content_type.as_header_value(),
            "Failed to decode OTLP request"
        );

        (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/plain")],
            content_type.decode_error_message(),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::collector::logs::v1::{
        ExportLogsServiceRequest, ExportLogsServiceResponse,
    };
    use opentelemetry_proto::tonic::collector::metrics::v1::{
        ExportMetricsServiceRequest, ExportMetricsServiceResponse,
    };
    use opentelemetry_proto::tonic::collector::trace::v1::{
        ExportTraceServiceRequest, ExportTraceServiceResponse,
    };
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

    // ==========================================================================
    // Content-Type Detection Tests
    // ==========================================================================

    #[test]
    fn test_content_type_from_headers_protobuf() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/x-protobuf".parse().unwrap(),
        );
        assert_eq!(
            OtlpContentType::from_headers(&headers),
            OtlpContentType::Protobuf
        );
    }

    #[test]
    fn test_content_type_from_headers_json() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        assert_eq!(
            OtlpContentType::from_headers(&headers),
            OtlpContentType::Json
        );
    }

    #[test]
    fn test_content_type_from_headers_json_with_charset() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        assert_eq!(
            OtlpContentType::from_headers(&headers),
            OtlpContentType::Json
        );
    }

    #[test]
    fn test_content_type_from_headers_unknown_defaults_to_protobuf() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
        assert_eq!(
            OtlpContentType::from_headers(&headers),
            OtlpContentType::Protobuf
        );
    }

    #[test]
    fn test_content_type_from_headers_missing_defaults_to_protobuf() {
        let headers = HeaderMap::new();
        assert_eq!(
            OtlpContentType::from_headers(&headers),
            OtlpContentType::Protobuf
        );
    }

    #[test]
    fn test_content_type_header_value() {
        assert_eq!(
            OtlpContentType::Protobuf.as_header_value(),
            "application/x-protobuf"
        );
        assert_eq!(OtlpContentType::Json.as_header_value(), "application/json");
    }

    // ==========================================================================
    // Traces - Protobuf Tests
    // ==========================================================================

    #[test]
    fn test_traces_decode_protobuf_empty() {
        let request = ExportTraceServiceRequest {
            resource_spans: vec![],
        };
        let bytes = Bytes::from(request.encode_to_vec());

        let decoded: ExportTraceServiceRequest =
            decode_request(&bytes, OtlpContentType::Protobuf).unwrap();
        assert_eq!(decoded.resource_spans.len(), 0);
    }

    #[test]
    fn test_traces_decode_protobuf_with_data() {
        let request = create_trace_request();
        let bytes = Bytes::from(request.encode_to_vec());

        let decoded: ExportTraceServiceRequest =
            decode_request(&bytes, OtlpContentType::Protobuf).unwrap();
        assert_eq!(decoded.resource_spans.len(), 1);
        assert_eq!(decoded.resource_spans[0].scope_spans.len(), 1);
        assert_eq!(decoded.resource_spans[0].scope_spans[0].spans.len(), 1);
        assert_eq!(
            decoded.resource_spans[0].scope_spans[0].spans[0].name,
            "test-span"
        );
    }

    #[test]
    fn test_traces_response_protobuf_roundtrip() {
        let response = ExportTraceServiceResponse {
            partial_success: None,
        };
        let bytes = response.encode_to_vec();
        let decoded = ExportTraceServiceResponse::decode(bytes.as_slice()).unwrap();
        assert!(decoded.partial_success.is_none());
    }

    #[test]
    fn test_traces_roundtrip_protobuf() {
        let request = create_trace_request();
        let bytes = Bytes::from(request.encode_to_vec());

        let decoded: ExportTraceServiceRequest =
            decode_request(&bytes, OtlpContentType::Protobuf).unwrap();
        let re_encoded = decoded.encode_to_vec();

        assert_eq!(request.encode_to_vec(), re_encoded);
    }

    // ==========================================================================
    // Traces - JSON Tests
    // ==========================================================================

    #[test]
    fn test_traces_decode_json_empty() {
        let json = r#"{"resourceSpans":[]}"#;
        let bytes = Bytes::from(json);

        let decoded: ExportTraceServiceRequest =
            decode_request(&bytes, OtlpContentType::Json).unwrap();
        assert_eq!(decoded.resource_spans.len(), 0);
    }

    #[test]
    fn test_traces_decode_json_with_data() {
        let json = r#"{
            "resourceSpans": [{
                "resource": {
                    "attributes": [{
                        "key": "service.name",
                        "value": {"stringValue": "test-service"}
                    }]
                },
                "scopeSpans": [{
                    "spans": [{
                        "traceId": "0102030405060708090a0b0c0d0e0f10",
                        "spanId": "0102030405060708",
                        "name": "test-span"
                    }]
                }]
            }]
        }"#;
        let bytes = Bytes::from(json);

        let decoded: ExportTraceServiceRequest =
            decode_request(&bytes, OtlpContentType::Json).unwrap();
        assert_eq!(decoded.resource_spans.len(), 1);
        assert_eq!(
            decoded.resource_spans[0].scope_spans[0].spans[0].name,
            "test-span"
        );
    }

    #[test]
    fn test_traces_response_json_roundtrip() {
        let response = ExportTraceServiceResponse {
            partial_success: None,
        };
        let json_bytes = serde_json::to_vec(&response).unwrap();
        let decoded: ExportTraceServiceResponse = serde_json::from_slice(&json_bytes).unwrap();
        assert!(decoded.partial_success.is_none());
    }

    #[test]
    fn test_traces_roundtrip_json() {
        let request = create_trace_request();
        let json_bytes = serde_json::to_vec(&request).unwrap();
        let bytes = Bytes::from(json_bytes);

        let decoded: ExportTraceServiceRequest =
            decode_request(&bytes, OtlpContentType::Json).unwrap();

        assert_eq!(request.resource_spans.len(), decoded.resource_spans.len());
        assert_eq!(
            request.resource_spans[0].scope_spans[0].spans[0].name,
            decoded.resource_spans[0].scope_spans[0].spans[0].name
        );
    }

    // ==========================================================================
    // Metrics - Protobuf Tests
    // ==========================================================================

    #[test]
    fn test_metrics_decode_protobuf_empty() {
        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![],
        };
        let bytes = Bytes::from(request.encode_to_vec());

        let decoded: ExportMetricsServiceRequest =
            decode_request(&bytes, OtlpContentType::Protobuf).unwrap();
        assert_eq!(decoded.resource_metrics.len(), 0);
    }

    #[test]
    fn test_metrics_response_protobuf_roundtrip() {
        let response = ExportMetricsServiceResponse {
            partial_success: None,
        };
        let bytes = response.encode_to_vec();
        let decoded = ExportMetricsServiceResponse::decode(bytes.as_slice()).unwrap();
        assert!(decoded.partial_success.is_none());
    }

    // ==========================================================================
    // Metrics - JSON Tests
    // ==========================================================================

    #[test]
    fn test_metrics_decode_json_empty() {
        let json = r#"{"resourceMetrics":[]}"#;
        let bytes = Bytes::from(json);

        let decoded: ExportMetricsServiceRequest =
            decode_request(&bytes, OtlpContentType::Json).unwrap();
        assert_eq!(decoded.resource_metrics.len(), 0);
    }

    #[test]
    fn test_metrics_response_json_roundtrip() {
        let response = ExportMetricsServiceResponse {
            partial_success: None,
        };
        let json_bytes = serde_json::to_vec(&response).unwrap();
        let decoded: ExportMetricsServiceResponse = serde_json::from_slice(&json_bytes).unwrap();
        assert!(decoded.partial_success.is_none());
    }

    // ==========================================================================
    // Logs - Protobuf Tests
    // ==========================================================================

    #[test]
    fn test_logs_decode_protobuf_empty() {
        let request = ExportLogsServiceRequest {
            resource_logs: vec![],
        };
        let bytes = Bytes::from(request.encode_to_vec());

        let decoded: ExportLogsServiceRequest =
            decode_request(&bytes, OtlpContentType::Protobuf).unwrap();
        assert_eq!(decoded.resource_logs.len(), 0);
    }

    #[test]
    fn test_logs_response_protobuf_roundtrip() {
        let response = ExportLogsServiceResponse {
            partial_success: None,
        };
        let bytes = response.encode_to_vec();
        let decoded = ExportLogsServiceResponse::decode(bytes.as_slice()).unwrap();
        assert!(decoded.partial_success.is_none());
    }

    // ==========================================================================
    // Logs - JSON Tests
    // ==========================================================================

    #[test]
    fn test_logs_decode_json_empty() {
        let json = r#"{"resourceLogs":[]}"#;
        let bytes = Bytes::from(json);

        let decoded: ExportLogsServiceRequest =
            decode_request(&bytes, OtlpContentType::Json).unwrap();
        assert_eq!(decoded.resource_logs.len(), 0);
    }

    #[test]
    fn test_logs_response_json_roundtrip() {
        let response = ExportLogsServiceResponse {
            partial_success: None,
        };
        let json_bytes = serde_json::to_vec(&response).unwrap();
        let decoded: ExportLogsServiceResponse = serde_json::from_slice(&json_bytes).unwrap();
        assert!(decoded.partial_success.is_none());
    }

    // ==========================================================================
    // Error Cases
    // ==========================================================================

    #[test]
    fn test_decode_error_display() {
        let protobuf_err = DecodeError::Protobuf("invalid wire type".to_string());
        assert_eq!(
            protobuf_err.to_string(),
            "protobuf decode error: invalid wire type"
        );

        let json_err = DecodeError::Json("expected ':'".to_string());
        assert_eq!(json_err.to_string(), "JSON decode error: expected ':'");
    }

    #[test]
    fn test_decode_invalid_protobuf() {
        let bytes = Bytes::from("not valid protobuf");
        let result: Result<ExportTraceServiceRequest, _> =
            decode_request(&bytes, OtlpContentType::Protobuf);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DecodeError::Protobuf(_)));
    }

    #[test]
    fn test_decode_invalid_json() {
        let bytes = Bytes::from("not valid json");
        let result: Result<ExportTraceServiceRequest, _> =
            decode_request(&bytes, OtlpContentType::Json);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DecodeError::Json(_)));
    }

    #[test]
    fn test_decode_wrong_json_schema() {
        let json = r#"{"wrongField": "value"}"#;
        let bytes = Bytes::from(json);

        // Missing required field should fail deserialization
        let result: Result<ExportTraceServiceRequest, _> =
            decode_request(&bytes, OtlpContentType::Json);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DecodeError::Json(_)));
    }

    #[test]
    fn test_decode_empty_body_protobuf() {
        let bytes = Bytes::new();
        // Empty bytes is valid protobuf for a message with no required fields
        let decoded: ExportTraceServiceRequest =
            decode_request(&bytes, OtlpContentType::Protobuf).unwrap();
        assert_eq!(decoded.resource_spans.len(), 0);
    }

    #[test]
    fn test_decode_empty_body_json() {
        let bytes = Bytes::new();
        let result: Result<ExportTraceServiceRequest, _> =
            decode_request(&bytes, OtlpContentType::Json);
        assert!(result.is_err());
    }

    // ==========================================================================
    // Test Helpers
    // ==========================================================================

    fn create_trace_request() -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".to_string(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue("test-service".to_string())),
                        }),
                    }],
                    dropped_attributes_count: 0,
                }),
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    spans: vec![Span {
                        trace_id: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
                        span_id: vec![1, 2, 3, 4, 5, 6, 7, 8],
                        trace_state: String::new(),
                        parent_span_id: vec![],
                        flags: 0,
                        name: "test-span".to_string(),
                        kind: 1,
                        start_time_unix_nano: 1000000000,
                        end_time_unix_nano: 2000000000,
                        attributes: vec![],
                        dropped_attributes_count: 0,
                        events: vec![],
                        dropped_events_count: 0,
                        links: vec![],
                        dropped_links_count: 0,
                        status: None,
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
    }
}
