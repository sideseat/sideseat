//! Server-Sent Events endpoint and connection management

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{RwLock, broadcast};
use tokio::time::interval;
use tracing::{debug, info};

use super::broadcast::EventBroadcaster;
use super::events::EventPayload;
use super::matcher::EventMatcher;
use crate::otel::query::TraceFilter;

/// SSE connection manager
pub struct SseManager {
    broadcaster: EventBroadcaster,
    subscriptions: RwLock<HashMap<u64, SubscriptionInfo>>,
    next_id: AtomicU64,
    max_connections: usize,
    timeout_secs: u64,
    keepalive_secs: u64,
}

/// Information about an SSE subscription
struct SubscriptionInfo {
    last_activity: std::time::Instant,
}

/// SSE subscription handle
pub struct SseSubscription {
    pub id: u64,
    pub receiver: broadcast::Receiver<Arc<EventPayload>>,
    pub matcher: EventMatcher,
}

impl SseManager {
    /// Create a new SSE manager
    pub fn new(
        broadcaster: EventBroadcaster,
        max_connections: usize,
        timeout_secs: u64,
        keepalive_secs: u64,
    ) -> Self {
        Self {
            broadcaster,
            subscriptions: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            max_connections,
            timeout_secs,
            keepalive_secs,
        }
    }

    /// Create a new subscription
    pub async fn subscribe(&self, filter: TraceFilter) -> Result<SseSubscription, SseError> {
        // Check connection limit
        let current_count = self.subscriptions.read().await.len();
        if current_count >= self.max_connections {
            return Err(SseError::TooManyConnections);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let receiver = self.broadcaster.subscribe();
        let matcher = EventMatcher::new(filter.clone());

        // Register subscription
        let info = SubscriptionInfo { last_activity: std::time::Instant::now() };

        self.subscriptions.write().await.insert(id, info);

        debug!("Created SSE subscription {}", id);

        Ok(SseSubscription { id, receiver, matcher })
    }

    /// Unsubscribe (remove subscription)
    pub async fn unsubscribe(&self, id: u64) {
        if self.subscriptions.write().await.remove(&id).is_some() {
            debug!("Removed SSE subscription {}", id);
        }
    }

    /// Update last activity time for a subscription
    pub async fn touch(&self, id: u64) {
        if let Some(info) = self.subscriptions.write().await.get_mut(&id) {
            info.last_activity = std::time::Instant::now();
        }
    }

    /// Get current subscription count
    pub async fn subscription_count(&self) -> usize {
        self.subscriptions.read().await.len()
    }

    /// Clean up stale subscriptions (background task)
    pub async fn cleanup_stale(&self) {
        let timeout = Duration::from_secs(self.timeout_secs);
        let now = std::time::Instant::now();

        let mut subscriptions = self.subscriptions.write().await;
        let before = subscriptions.len();

        subscriptions.retain(|_, info| now.duration_since(info.last_activity) < timeout);

        let removed = before - subscriptions.len();
        if removed > 0 {
            info!("Cleaned up {} stale SSE subscriptions", removed);
        }
    }

    /// Get keepalive interval
    pub fn keepalive_interval(&self) -> Duration {
        Duration::from_secs(self.keepalive_secs)
    }

    /// Get the broadcaster
    pub fn broadcaster(&self) -> &EventBroadcaster {
        &self.broadcaster
    }

    /// Start the cleanup background task
    pub fn start_cleanup_task(self: Arc<Self>) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                manager.cleanup_stale().await;
            }
        });
    }
}

/// SSE errors
#[derive(Debug, thiserror::Error)]
pub enum SseError {
    #[error("Too many SSE connections")]
    TooManyConnections,

    #[error("Subscription not found")]
    NotFound,
}
