//! Traces export endpoint.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;

use super::OtlpState;
use super::encoding::{OtlpContentType, decode_request, success_response};
use crate::extractors::is_valid_project_id;
use sideseat_core::core::constants::BACKPRESSURE_RETRY_AFTER_SECS;
use sideseat_domain::signals::{SignalContext, SignalExportError, export_signal};

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
    let request: ExportTraceServiceRequest = match decode_request(&body, content_type) {
        Ok(request) => request,
        Err(error) => return error.into_response(content_type),
    };

    match export_signal(
        state.trace_signal.as_ref(),
        request,
        SignalContext {
            project_id: &project_id,
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
            "Unknown project, trace or session, or it is being deleted",
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
