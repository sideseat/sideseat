//! OTLP logs export endpoint.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;

use super::OtlpState;
use super::encoding::{OtlpContentType, decode_request, success_response};
use crate::extractors::is_valid_project_id;
use sideseat_core::constants::BACKPRESSURE_RETRY_AFTER_SECS;
use sideseat_ingestion::received::ReceivedPayload;
use sideseat_ingestion::signals::{SignalContext, SignalExportError, export_signal};

/// Export logs over OTLP/HTTP
#[utoipa::path(
    post,
    path = "/otel/{project_id}/v1/logs",
    tag = "otlp",
    params(("project_id" = String, Path, description = "Project ID")),
    request_body(content = String, description = "An OTLP ExportLogsServiceRequest, as protobuf \
                 (`application/x-protobuf`) or OTLP/JSON (`application/json`), optionally gzip-compressed",
                 content_type = "application/x-protobuf"),
    responses(
        (status = 200, description = "Stored durably: an ExportLogsServiceResponse, in the request's encoding, \
         reporting any records partially rejected"),
        (status = 400, description = "Not a decodable OTLP request, or an invalid project id"),
        (status = 404, description = "Unknown project, or one being deleted"),
        (status = 413, description = "Larger than the body limit"),
        (status = 503, description = "Ingest is holding as many bytes as it may: retry after `Retry-After`"),
        (status = 507, description = "The project's storage quota is exceeded")
    )
)]
pub async fn export(
    State(state): State<OtlpState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_valid_project_id(&project_id) {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/plain")],
            "Invalid project_id",
        )
            .into_response();
    }
    if !super::project_accepts_writes(&state, &project_id).await {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            "Unknown project, or it is being deleted",
        )
            .into_response();
    }

    let content_type = OtlpContentType::from_headers(&headers);
    let request: ExportLogsServiceRequest = match decode_request(&body, content_type) {
        Ok(request) => request,
        Err(error) => return error.into_response(content_type),
    };

    // The body as it arrived, for staging: the decoded request is about to be mutated.
    let received =
        ReceivedPayload::new(body.to_vec(), content_type.raw_content(), state.clock.now());
    match export_signal(
        state.log_signal.as_ref(),
        request,
        SignalContext {
            project_id: &project_id,
            received: &received,
            debug_path: state.debug_path.as_deref(),
            clock: state.clock.as_ref(),
            staging: state.staging.as_ref(),
            storage_governance: state.storage_governance.as_ref(),
        },
    )
    .await
    {
        Ok(response) => success_response(&response, content_type),
        Err(SignalExportError::Gone) => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            "Unknown project, or it is being deleted",
        )
            .into_response(),
        Err(SignalExportError::QuotaExceeded) => (
            StatusCode::INSUFFICIENT_STORAGE,
            [(header::CONTENT_TYPE, "text/plain")],
            "Project storage quota exceeded",
        )
            .into_response(),
        Err(SignalExportError::StoreUnavailable | SignalExportError::QueueFull) => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(
                HeaderName::from_static("retry-after"),
                BACKPRESSURE_RETRY_AFTER_SECS.to_string(),
            )],
        )
            .into_response(),
    }
}
