use axum::{Json, extract::State, response::IntoResponse};
use serde::Serialize;
use std::sync::Arc;

use crate::otel::{OtelManager, health::OtelHealthStatus};

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel: Option<OtelHealthStatus>,
}

/// Simple health check (no state)
pub async fn health_check() -> &'static str {
    "OK"
}

/// Health check with OTel status
pub async fn health_check_with_otel(
    State(otel): State<Option<Arc<OtelManager>>>,
) -> impl IntoResponse {
    let otel_status = match otel {
        Some(manager) => Some(manager.health_status().await),
        None => None,
    };

    Json(HealthResponse { status: "OK", otel: otel_status })
}
