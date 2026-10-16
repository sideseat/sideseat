//! Shared helpers for publishing `PresenceEvent` to the per-project
//! broadcast topic.

use sideseat_ports::registrations::PresenceEvent;

use super::state::WsState;

/// Topic name for presence broadcasts. Single source of truth so a typo here
/// can't desync producers and consumers.
/// Publish a `PresenceEvent` to the project's presence topic, logging at WARN
/// on failure. Best-effort — presence is fire-and-forget.
pub async fn publish(state: &WsState, event: &PresenceEvent) {
    let topic = state.topics.broadcast_topic::<PresenceEvent>(
        sideseat_ports::queue::broadcast_kinds::PRESENCE,
        event.project_id(),
    );
    if let Err(e) = topic.publish(event).await {
        tracing::warn!(error = %e, "ws: presence publish failed");
    }
}
