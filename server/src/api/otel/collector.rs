//! OTLP collector endpoint handlers

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::post,
};
use std::sync::Arc;

use crate::otel::ingest::convert_traces_request;
use crate::otel::{OtelError, OtelManager};

/// Create collector routes
pub fn create_routes(otel: Arc<OtelManager>) -> Router {
    Router::new().route("/traces", post(handle_traces)).with_state(otel)
}

/// POST /otel/v1/traces - OTLP HTTP trace ingestion
pub async fn handle_traces(
    State(otel): State<Arc<OtelManager>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // Check disk space
    if otel.disk_monitor.should_pause_ingestion() {
        return OtelError::DiskSpaceCritical(95).into_response();
    }

    // Validate request size
    let max_size = 10 * 1024 * 1024; // 10MB max
    if body.len() > max_size {
        return OtelError::ValidationError(format!(
            "Request body too large: {} bytes (max: {})",
            body.len(),
            max_size
        ))
        .into_response();
    }

    // Determine content type
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/x-protobuf");

    // Convert to normalized spans
    let spans = match convert_traces_request(&body, content_type) {
        Ok(spans) => spans,
        Err(e) => return e.into_response(),
    };

    // Send to ingestion channel using try_send to avoid blocking
    let sender = otel.sender();
    for span in spans {
        match sender.try_send(span) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                return OtelError::BufferFull.into_response();
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return OtelError::BufferFull.into_response();
            }
        }
    }

    StatusCode::OK.into_response()
}
