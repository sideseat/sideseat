//! Shared helpers for publishing `PresenceEvent` to the per-project
//! broadcast topic.

use crate::data::registrations::PresenceEvent;

use super::state::WsState;

/// Topic name for presence broadcasts. Single source of truth so a typo here
/// can't desync producers and consumers.
pub fn presence_topic_name(project_id: &str) -> String {
    format!("presence:{}", project_id)
}

/// Publish a `PresenceEvent` to the project's presence topic, logging at WARN
/// on failure. Best-effort — presence is fire-and-forget.
pub async fn publish(state: &WsState, event: &PresenceEvent) {
    let topic = state
        .topics
        .broadcast_topic::<PresenceEvent>(&presence_topic_name(event.project_id()));
    if let Err(e) = topic.publish(event).await {
        tracing::warn!(error = %e, "ws: presence publish failed");
    }
}
