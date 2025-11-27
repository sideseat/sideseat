//! Broadcast channel for real-time events

use std::sync::Arc;
use tokio::sync::broadcast;

use super::events::EventPayload;

/// Broadcaster for trace events
pub struct EventBroadcaster {
    sender: broadcast::Sender<Arc<EventPayload>>,
}

impl EventBroadcaster {
    /// Create a new broadcaster with the given capacity
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Broadcast an event to all subscribers
    pub fn broadcast(&self, event: EventPayload) -> usize {
        // Returns number of receivers that got the message
        self.sender.send(Arc::new(event)).unwrap_or(0)
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<EventPayload>> {
        self.sender.subscribe()
    }

    /// Get the current number of subscribers
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventBroadcaster {
    fn default() -> Self {
        Self::new(1024) // Default capacity
    }
}

impl Clone for EventBroadcaster {
    fn clone(&self) -> Self {
        Self { sender: self.sender.clone() }
    }
}
