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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otel::realtime::events::{HealthEvent, TraceEvent};

    #[test]
    fn test_broadcaster_new() {
        let broadcaster = EventBroadcaster::new(100);
        assert_eq!(broadcaster.subscriber_count(), 0);
    }

    #[test]
    fn test_broadcaster_default() {
        let broadcaster = EventBroadcaster::default();
        assert_eq!(broadcaster.subscriber_count(), 0);
    }

    #[test]
    fn test_broadcaster_subscribe() {
        let broadcaster = EventBroadcaster::new(100);
        let _rx1 = broadcaster.subscribe();
        assert_eq!(broadcaster.subscriber_count(), 1);

        let _rx2 = broadcaster.subscribe();
        assert_eq!(broadcaster.subscriber_count(), 2);
    }

    #[test]
    fn test_broadcaster_clone() {
        let broadcaster = EventBroadcaster::new(100);
        let _rx = broadcaster.subscribe();

        let cloned = broadcaster.clone();
        assert_eq!(cloned.subscriber_count(), 1);
    }

    #[test]
    fn test_broadcaster_broadcast_no_subscribers() {
        let broadcaster = EventBroadcaster::new(100);
        let event = TraceEvent::HealthUpdate(HealthEvent {
            status: "healthy".to_string(),
            disk_usage_percent: 50,
            pending_spans: 0,
            total_traces: 100,
        });
        let payload = EventPayload::new(event);
        let count = broadcaster.broadcast(payload);
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_broadcaster_broadcast_with_subscriber() {
        let broadcaster = EventBroadcaster::new(100);
        let mut rx = broadcaster.subscribe();

        let event = TraceEvent::HealthUpdate(HealthEvent {
            status: "healthy".to_string(),
            disk_usage_percent: 50,
            pending_spans: 0,
            total_traces: 100,
        });
        let payload = EventPayload::new(event);
        let count = broadcaster.broadcast(payload);
        assert_eq!(count, 1);

        let received = rx.recv().await.unwrap();
        assert!(matches!(received.event, TraceEvent::HealthUpdate(_)));
    }
}
