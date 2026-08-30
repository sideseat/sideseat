//! Metrics export endpoint

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsPartialSuccess, ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};

use super::encoding::{OtlpContentType, decode_request, success_response};
use super::{OtlpState, inject_project_id_metrics};
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

    // Say so now, rather than accepting the request and dropping its spans at the write. The write path
    // is where the fence is authoritative; this is where the exporter can still be told.
    if !super::project_accepts_writes(&state, &project_id).await {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            "Unknown project, or it is being deleted",
        )
            .into_response();
    }

    let content_type = OtlpContentType::from_headers(&headers);

    // Parse request (protobuf or JSON based on content type)
    let mut request: ExportMetricsServiceRequest = match decode_request(&body, content_type) {
        Ok(req) => req,
        Err(e) => return e.into_response(content_type),
    };

    // Inject project_id into resource attributes
    inject_project_id_metrics(&mut request, &project_id);

    // Write to debug file if debug mode is enabled
    if let Some(ref debug_path) = state.debug_path {
        write_debug(debug_path, "metrics.jsonl", &project_id, &request).await;
    }

    // Written before the answer, not queued behind it. A 200 used to mean "in an in-process buffer", so
    // a crash or a database that stayed down through its retries lost records the exporter had counted as
    // delivered - and nothing surfaced it. A failure is now a 503 the exporter retries.
    let stored =
        match crate::domain::ingest_metrics(&request, &state.analytics, &state.database).await {
            Ok(stored) => stored,
            Err(e) => {
                tracing::error!(error = %e, %project_id, "Failed to store metrics");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(
                        HeaderName::from_static("retry-after"),
                        BACKPRESSURE_RETRY_AFTER_SECS.to_string(),
                    )],
                )
                    .into_response();
            }
        };

    // Anything dropped is *reported* dropped. A project that stopped accepting writes between the check
    // above and the write leaves records unstored, and an unqualified success would have the exporter
    // count them as delivered.
    let rejected = stored.total.saturating_sub(stored.stored);
    let response = ExportMetricsServiceResponse {
        partial_success: (rejected > 0).then(|| ExportMetricsPartialSuccess {
            rejected_data_points: rejected as i64,
            error_message: "the project is unknown or is being deleted".to_string(),
        }),
    };
    success_response(&response, content_type)
}
