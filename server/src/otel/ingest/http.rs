//! HTTP OTLP handlers

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use std::sync::Arc;
use tokio::sync::mpsc;

use super::converter::convert_traces_request;
use super::validator::validate_request;
use crate::otel::normalize::NormalizedSpan;

/// Shared state for OTLP handlers
#[derive(Clone)]
pub struct OtlpState {
    pub sender: mpsc::Sender<NormalizedSpan>,
    pub max_request_size: usize,
    pub max_attribute_count: usize,
    pub max_attribute_value_len: usize,
}

/// POST /v1/traces - OTLP HTTP trace ingestion
pub async fn handle_traces(
    State(state): State<Arc<OtlpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // Validate request size
    if let Err(e) = validate_request(&body, state.max_request_size) {
        return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }

    // Determine content type (protobuf or JSON)
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/x-protobuf");

    // Convert to normalized spans
    let spans = match convert_traces_request(&body, content_type) {
        Ok(spans) => spans,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    // Send to ingestion channel
    for span in spans {
        if state.sender.send(span).await.is_err() {
            return (StatusCode::SERVICE_UNAVAILABLE, "Ingestion channel full").into_response();
        }
    }

    StatusCode::OK.into_response()
}
