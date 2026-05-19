//! SSE endpoint streaming registration presence for a project.
//!
//! `GET /api/v1/project/{project_id}/presence`
//!
//! First frame is `event: snapshot` with the same payload shape as
//! `listing::ListingResponse` so the UI gets initial state in the same
//! round-trip. Subsequent frames are `event: presence` carrying the
//! published `PresenceEvent` discriminated union.
//!
//! Subscription happens BEFORE the snapshot is built so a register/
//! unregister between `list()` and `subscribe()` cannot be dropped.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;

use crate::api::extractors::is_valid_project_id;
use crate::api::types::ApiError;
use crate::core::TopicError;
use crate::data::registrations::{PresenceEvent, RegistrationKind};

use super::listing::{ListingResponse, ProjectPath};
use super::presence::presence_topic_name;
use super::state::WsState;

pub async fn stream_presence(
    State(state): State<WsState>,
    Path(ProjectPath { project_id }): Path<ProjectPath>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    if !is_valid_project_id(&project_id) {
        return Err(ApiError::bad_request(
            "invalid_project_id",
            "project_id has invalid characters or length",
        ));
    }

    let topic = state
        .topics
        .broadcast_topic::<PresenceEvent>(&presence_topic_name(&project_id));
    let mut subscriber = topic
        .subscribe()
        .await
        .map_err(|e| ApiError::internal(format!("subscribe failed: {e}")))?;

    let snapshot = build_snapshot(&state, &project_id).await?;
    let snapshot_json = serde_json::to_string(&snapshot)
        .map_err(|e| ApiError::internal(format!("snapshot serialise failed: {e}")))?;

    let mut shutdown_rx = state.shutdown_rx.clone();

    let stream = async_stream::stream! {
        yield Ok::<Event, Infallible>(Event::default().event("snapshot").data(snapshot_json));

        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        yield Ok::<Event, Infallible>(
                            Event::default().event("terminate").data("shutdown"),
                        );
                        break;
                    }
                }
                result = subscriber.recv() => {
                    match result {
                        Ok(event) => {
                            match serde_json::to_string(&event) {
                                Ok(data) => yield Ok::<Event, Infallible>(
                                    Event::default().event("presence").data(data),
                                ),
                                Err(e) => tracing::warn!(error = %e, "presence: serialise failed"),
                            }
                        }
                        Err(TopicError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "presence: subscriber lagged");
                        }
                        Err(TopicError::ChannelClosed) => break,
                        Err(_) => break,
                    }
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keep-alive"),
    ))
}

async fn build_snapshot(state: &WsState, project_id: &str) -> Result<ListingResponse, ApiError> {
    let entries = state
        .registrations
        .list(project_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut agents = Vec::new();
    let mut mcps = Vec::new();
    let mut swarms = Vec::new();
    let mut graphs = Vec::new();
    for e in entries {
        match e.kind {
            RegistrationKind::Agent => agents.push(e),
            RegistrationKind::Mcp => mcps.push(e),
            RegistrationKind::Swarm => swarms.push(e),
            RegistrationKind::Graph => graphs.push(e),
        }
    }
    Ok(ListingResponse {
        agents,
        mcps,
        swarms,
        graphs,
    })
}
