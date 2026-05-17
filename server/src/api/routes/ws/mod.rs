//! SDK WebSocket routes (registration + introspection).

pub(crate) mod chunks;
mod expiry;
mod handler;
pub(crate) mod invoke;
mod listing;
mod presence;
pub(crate) mod protocol;
mod rate_limit;
pub(crate) mod state;

use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use tokio::sync::watch;

use crate::data::registrations::RegistrationStore;
use crate::data::topics::TopicService;

pub use state::WsState;

/// Build routes and spawn the background TTL sweeper.
///
///   GET /api/v1/project/{project_id}/ws            (handler::ws_upgrade)
///   GET /api/v1/project/{project_id}/registrations (listing::list_registrations)
pub fn routes(
    topics: Arc<TopicService>,
    registrations: Arc<dyn RegistrationStore>,
    shutdown_rx: watch::Receiver<bool>,
) -> (Router<()>, WsState) {
    let state = WsState::new(topics, registrations, shutdown_rx);
    expiry::spawn_sweeper(state.clone());
    let router = Router::new()
        .route("/project/{project_id}/ws", get(handler::ws_upgrade))
        .route(
            "/project/{project_id}/registrations",
            get(listing::list_registrations),
        )
        .with_state(state.clone());
    (router, state)
}
