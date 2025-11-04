use crate::api;
use axum::{Router, routing::get};

pub fn create_routes() -> Router {
    Router::new().route("/health", get(api::health::health_check))
}
