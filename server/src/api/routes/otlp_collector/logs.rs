//! Logs export endpoint

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};

use super::encoding::{OtlpContentType, decode_request, success_response};
use super::{OtlpState, inject_project_id_logs};
use crate::api::extractors::is_valid_project_id;
use crate::core::constants::BACKPRESSURE_RETRY_AFTER_SECS;
use crate::utils::debug::write_debug;

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
    let mut request: ExportLogsServiceRequest = match decode_request(&body, content_type) {
        Ok(req) => req,
        Err(e) => return e.into_response(content_type),
    };

    // Inject project_id into resource attributes
    inject_project_id_logs(&mut request, &project_id);

    // Write to debug file if debug mode is enabled
    if let Some(ref debug_path) = state.debug_path {
        write_debug(debug_path, "logs.jsonl", &project_id, &request).await;
    }

    if let Err(e) = state.logs_publisher.publish(request) {
        tracing::warn!(error = %e, "Failed to publish logs to topic");
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
    let response = ExportLogsServiceResponse {
        partial_success: None,
    };
    success_response(&response, content_type)
}
