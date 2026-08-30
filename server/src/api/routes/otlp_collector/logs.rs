//! Logs export endpoint.
//!
//! SideSeat does not store logs: there is no `otel_logs` table, no logs domain, and nothing subscribes
//! to the logs topic. The endpoint exists so that an OTel SDK configured with one endpoint for all three
//! signals is not met with a 404 it will retry forever.
//!
//! So it says so, in the field OTLP provides for exactly this. Publishing to a topic nobody reads and
//! answering with an unqualified success was the one unacceptable option: the exporter counted the
//! records as delivered, the operator saw no error anywhere, and the records were gone.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsPartialSuccess, ExportLogsServiceRequest, ExportLogsServiceResponse,
};

use super::encoding::{OtlpContentType, decode_request, success_response};
use super::{OtlpState, inject_project_id_logs};
use crate::api::extractors::is_valid_project_id;
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

    // Still published, because a subscriber is how logs would arrive if one existed and because the
    // debug writer above is useful for capturing payloads. A failure here is not worth a 503: nothing
    // downstream is going to store them either way.
    if let Err(e) = state.logs_publisher.publish(request.clone()) {
        tracing::debug!(error = %e, "Failed to publish logs to topic");
    }

    // Rejected, with a reason, rather than silently accepted. `partial_success` with every record
    // rejected is what OTLP provides for "received but not stored", and an exporter surfaces it.
    let rejected: i64 = request
        .resource_logs
        .iter()
        .flat_map(|resource| resource.scope_logs.iter())
        .map(|scope| scope.log_records.len() as i64)
        .sum();
    let response = ExportLogsServiceResponse {
        partial_success: Some(ExportLogsPartialSuccess {
            rejected_log_records: rejected,
            error_message: "SideSeat does not store logs; send traces to /v1/traces".to_string(),
        }),
    };
    success_response(&response, content_type)
}
