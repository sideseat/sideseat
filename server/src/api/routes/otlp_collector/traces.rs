//! Traces export endpoint

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::common::v1::any_value;

use super::encoding::{OtlpContentType, decode_request, success_response};
use super::{OtlpState, inject_project_id_traces};
use crate::api::extractors::is_valid_project_id;
use crate::core::constants::BACKPRESSURE_RETRY_AFTER_SECS;
use crate::utils::debug::write_debug;
use crate::utils::otlp::PROJECT_ID_ATTR;

/// Maximum retry attempts for trace publish
const PUBLISH_MAX_ATTEMPTS: u32 = 3;

/// Base delay in milliseconds for exponential backoff
const PUBLISH_BASE_DELAY_MS: u64 = 50;

pub async fn export(
    State(state): State<OtlpState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Validate project_id
    if !is_valid_project_id(&project_id) {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/plain")],
            "Invalid project_id",
        )
            .into_response();
    }

    let content_type = OtlpContentType::from_headers(&headers);

    // Parse request (protobuf or JSON based on content type)
    let mut request: ExportTraceServiceRequest = match decode_request(&body, content_type) {
        Ok(req) => req,
        Err(e) => return e.into_response(content_type),
    };

    // Check for existing project_id in request and log if mismatched
    check_project_id_mismatch(&request, &project_id);

    // Inject project_id into resource attributes (path takes precedence)
    inject_project_id_traces(&mut request, &project_id);

    // Write to debug file if debug mode is enabled
    if let Some(ref debug_path) = state.debug_path {
        write_debug(debug_path, "traces.jsonl", &project_id, &request).await;
    }

    // Publish to stream topic with retry (at-least-once delivery)
    let mut last_error = None;
    for attempt in 1..=PUBLISH_MAX_ATTEMPTS {
        match state.trace_topic.publish(&request).await {
            Ok(_) => {
                if attempt > 1 {
                    tracing::debug!(attempt, "Trace publish succeeded after retry");
                }
                last_error = None;
                break;
            }
            Err(e) => {
                last_error = Some(e);
                if attempt < PUBLISH_MAX_ATTEMPTS {
                    let delay =
                        Duration::from_millis(PUBLISH_BASE_DELAY_MS * 2_u64.pow(attempt - 1));
                    tracing::warn!(
                        error = %last_error.as_ref().unwrap(),
                        attempt,
                        delay_ms = delay.as_millis(),
                        "Retrying trace publish after transient error"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    if let Some(e) = last_error {
        tracing::warn!(error = %e, attempts = PUBLISH_MAX_ATTEMPTS, "Failed to publish traces after retries");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(
                HeaderName::from_static("retry-after"),
                BACKPRESSURE_RETRY_AFTER_SECS.to_string(),
            )],
        )
            .into_response();
    }

    // Return OTLP-compliant response (matching request content type)
    let response = ExportTraceServiceResponse {
        partial_success: None,
    };
    success_response(&response, content_type)
}

/// Check if request contains a project_id that mismatches the path project_id
fn check_project_id_mismatch(request: &ExportTraceServiceRequest, path_project_id: &str) {
    for resource_spans in &request.resource_spans {
        if let Some(ref resource) = resource_spans.resource {
            for attr in &resource.attributes {
                if attr.key == PROJECT_ID_ATTR
                    && let Some(ref value) = attr.value
                    && let Some(any_value::Value::StringValue(existing_id)) = &value.value
                    && existing_id != path_project_id
                {
                    tracing::warn!(
                        path_project_id,
                        request_project_id = %existing_id,
                        "Project ID mismatch: request contains different project_id than URL path"
                    );
                }
            }
        }
    }
}
