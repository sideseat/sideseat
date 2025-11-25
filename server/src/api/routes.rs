use crate::api;
use crate::auth::AuthManager;
use axum::{Router, routing::get};
use std::sync::Arc;

pub fn create_routes(auth_manager: Arc<AuthManager>) -> Router {
    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/health", get(api::health::health_check))
        .nest("/auth", api::auth::create_routes(auth_manager.clone()));

    // Protected routes (auth required)
    // To add protected endpoints, use:
    //   use crate::auth::require_auth;
    //   use axum::middleware;
    //   let protected_routes = Router::new()
    //       .route("/protected", get(some_handler))
    //       .layer(middleware::from_fn_with_state(auth_manager, require_auth));
    //   Router::new().merge(public_routes).merge(protected_routes)
    let _ = auth_manager; // Will be used when protected routes are added

    Router::new().merge(public_routes)
}
