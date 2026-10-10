//! Health check endpoint

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;
use utoipa::ToSchema;

/// Whether a store has failed in a way only a restart recovers.
pub type FatalFailureCheck = dyn Fn() -> bool + Send + Sync;

#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    /// `ok`, or `failed` once a store has stopped for good and the server is going down to recover it.
    pub status: &'static str,
    pub version: &'static str,
}

/// Health check endpoint
///
/// Unhealthy once a store has failed in a way only a restart recovers, so a supervisor replaces the process even
/// while it is still draining. Why is left to the server's log: this answer is unauthenticated.
#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse),
        (status = 503, description = "A store has failed and the server must restart", body = HealthResponse)
    )
)]
pub async fn health(State(failed): State<Arc<FatalFailureCheck>>) -> impl IntoResponse {
    let (status, label) = if failed() {
        (StatusCode::SERVICE_UNAVAILABLE, "failed")
    } else {
        (StatusCode::OK, "ok")
    };
    (
        status,
        Json(HealthResponse {
            status: label,
            version: env!("CARGO_PKG_VERSION"),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn status(failed: bool) -> StatusCode {
        let check: Arc<FatalFailureCheck> = Arc::new(move || failed);
        health(State(check)).await.into_response().status()
    }

    #[tokio::test]
    async fn a_failed_store_makes_the_server_unhealthy() {
        assert_eq!(status(false).await, StatusCode::OK);
        assert_eq!(status(true).await, StatusCode::SERVICE_UNAVAILABLE);
    }
}
