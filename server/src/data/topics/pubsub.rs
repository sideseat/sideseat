//! Pub/Sub bridge management
//!
//! Used by Redis backend for distributed pub/sub. Not used by memory backend.
//!
//! Provides lifecycle management for broadcast topic subscriptions:
//! - One bridge task per topic (not per subscriber)
//! - Reference counting for automatic cleanup
//! - Graceful shutdown support
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        PubSubManager                             │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  bridges: HashMap<topic, Arc<PubSubBridge>>                      │
//! │                                                                  │
//! │  ┌─────────────────────────────────────────────────────────────┐│
//! │  │ PubSubBridge (per topic)                                    ││
//! │  │  - sender: broadcast::Sender                                ││
//! │  │  - subscriber_count: AtomicU64                              ││
//! │  │  - task_handle: Option<JoinHandle>  (Redis only)            ││
//! │  │  - stop_tx: watch::Sender<bool>                             ││
//! │  └─────────────────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────────────────┘
//!
//! Message Flow (Redis):
//!   publish() ──► Redis PUBLISH ──► Bridge Task ──► Local Broadcast ──► Subscribers
//!
//! Message Flow (Memory):
//!   publish() ──► Local Broadcast ──► Subscribers
//! ```
//!
//! ## Deduplication
//!
//! Redis backend sends ONLY to Redis in publish(), not to local broadcast.
//! All messages flow through the bridge task, eliminating duplicates.
//! Memory backend sends directly to local broadcast (no bridge needed).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

/// Default broadcast channel capacity
const DEFAULT_BROADCAST_CAPACITY: usize = 10_000;

/// Manages pub/sub bridges for all topics
///
/// Ensures one bridge per topic and handles lifecycle management.
pub struct PubSubManager {
    /// Active bridges by topic name
    bridges: RwLock<HashMap<String, Arc<PubSubBridge>>>,
    /// Global shutdown signal
    #[allow(dead_code)]
    shutdown_tx: watch::Sender<bool>,
    /// Shutdown receiver for cloning
    shutdown_rx: watch::Receiver<bool>,
    /// Broadcast channel capacity
    broadcast_capacity: usize,
}

impl Default for PubSubManager {
    fn default() -> Self {
        Self::new(DEFAULT_BROADCAST_CAPACITY)
    }
}

impl PubSubManager {
    /// Create a new pub/sub manager
    pub fn new(broadcast_capacity: usize) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            bridges: RwLock::new(HashMap::new()),
            shutdown_tx,
            shutdown_rx,
            broadcast_capacity,
        }
    }

    /// Get or create a bridge for a topic
    ///
    /// Returns (bridge, is_new) where is_new indicates if a new bridge was created.
    /// The caller should start the bridge task if is_new is true (for Redis backend).
    pub fn get_or_create_bridge(&self, topic: &str) -> (Arc<PubSubBridge>, bool) {
        // Fast path: check if bridge exists
        {
            let bridges = self.bridges.read();
            if let Some(bridge) = bridges.get(topic) {
                return (Arc::clone(bridge), false);
            }
        }

        // Slow path: create new bridge
        let mut bridges = self.bridges.write();

        // Double-check after acquiring write lock
        if let Some(bridge) = bridges.get(topic) {
            return (Arc::clone(bridge), false);
        }

        let bridge = Arc::new(PubSubBridge::new(
            topic.to_string(),
            self.broadcast_capacity,
            self.shutdown_rx.clone(),
        ));
        bridges.insert(topic.to_string(), Arc::clone(&bridge));

        (bridge, true)
    }

    /// Remove a bridge when it has no subscribers
    ///
    /// Called by ManagedSubscription on drop when subscriber_count reaches 0.
    pub fn remove_bridge(&self, topic: &str) {
        let mut bridges = self.bridges.write();
        if let Some(bridge) = bridges.get(topic) {
            // Only remove if no subscribers
            if bridge.subscriber_count() == 0 {
                // Stop the bridge task first
                bridge.stop();
                bridges.remove(topic);
                tracing::debug!(topic, "Removed idle pub/sub bridge");
            }
        }
    }

    /// Get a bridge if it exists (for publishing)
    #[allow(dead_code)]
    pub fn get_bridge(&self, topic: &str) -> Option<Arc<PubSubBridge>> {
        self.bridges.read().get(topic).cloned()
    }

    /// Shutdown all bridges gracefully
    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        // Signal all bridges to stop
        let _ = self.shutdown_tx.send(true);

        // Collect all bridge handles
        let bridges: Vec<Arc<PubSubBridge>> = {
            let guard = self.bridges.read();
            guard.values().cloned().collect()
        };

        // Wait for all bridge tasks to complete
        for bridge in bridges {
            bridge.wait_for_stop().await;
        }

        // Clear all bridges
        self.bridges.write().clear();

        tracing::debug!("PubSubManager shutdown complete");
    }

    /// Get the shutdown receiver for bridge tasks
    #[allow(dead_code)]
    pub fn shutdown_rx(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }
}

/// A pub/sub bridge for a single topic
///
/// Manages the broadcast channel and optional background task (for Redis).
pub struct PubSubBridge {
    /// Topic name
    topic: String,
    /// Local broadcast channel sender
    sender: broadcast::Sender<Vec<u8>>,
    /// Number of active subscribers (for cleanup)
    subscriber_count: AtomicU64,
    /// Background task handle (Redis bridge task)
    task_handle: Mutex<Option<JoinHandle<()>>>,
    /// Signal to stop the bridge task
    stop_tx: watch::Sender<bool>,
    /// Receiver for stop signal (cloned by bridge task)
    stop_rx: watch::Receiver<bool>,
    /// Global shutdown signal
    shutdown_rx: watch::Receiver<bool>,
}

impl PubSubBridge {
    /// Create a new bridge for a topic
    fn new(topic: String, capacity: usize, shutdown_rx: watch::Receiver<bool>) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        let (stop_tx, stop_rx) = watch::channel(false);

        Self {
            topic,
            sender,
            subscriber_count: AtomicU64::new(0),
            task_handle: Mutex::new(None),
            stop_tx,
            stop_rx,
            shutdown_rx,
        }
    }

    /// Get the topic name
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Get current subscriber count
    pub fn subscriber_count(&self) -> u64 {
        self.subscriber_count.load(Ordering::SeqCst)
    }

    /// Increment subscriber count and return new value
    pub fn add_subscriber(&self) -> u64 {
        self.subscriber_count.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Decrement subscriber count and return new value
    pub fn remove_subscriber(&self) -> u64 {
        let prev = self.subscriber_count.fetch_sub(1, Ordering::SeqCst);
        prev.saturating_sub(1)
    }

    /// Get a new subscriber receiver
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.sender.subscribe()
    }

    /// Send a message to all subscribers
    pub fn send(&self, payload: Vec<u8>) -> Result<usize, broadcast::error::SendError<Vec<u8>>> {
        self.sender.send(payload)
    }

    /// Check if bridge task is running
    pub fn is_task_running(&self) -> bool {
        self.task_handle.lock().is_some()
    }

    /// Set the bridge task handle
    pub fn set_task(&self, handle: JoinHandle<()>) {
        let mut guard = self.task_handle.lock();
        if guard.is_some() {
            tracing::warn!(topic = %self.topic, "Bridge task already set, replacing");
            // Abort old task before replacing
            if let Some(old) = guard.take() {
                old.abort();
            }
        }
        *guard = Some(handle);
    }

    /// Get the stop signal receiver (for bridge task)
    pub fn stop_rx(&self) -> watch::Receiver<bool> {
        self.stop_rx.clone()
    }

    /// Get the global shutdown receiver (for bridge task)
    pub fn shutdown_rx(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    /// Signal the bridge task to stop
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }

    /// Wait for the bridge task to complete
    #[allow(dead_code)]
    pub async fn wait_for_stop(&self) {
        let handle = self.task_handle.lock().take();
        if let Some(h) = handle {
            // Give it a chance to stop gracefully
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), h).await;
        }
    }
}

/// A managed subscription that cleans up on drop
///
/// Wraps a broadcast receiver and decrements the subscriber count when dropped.
/// When the last subscriber is dropped, triggers bridge cleanup.
pub struct ManagedSubscription {
    /// The broadcast receiver
    receiver: broadcast::Receiver<Vec<u8>>,
    /// Reference to the bridge (for cleanup)
    bridge: Arc<PubSubBridge>,
    /// Reference to the manager (for bridge removal)
    manager: Arc<PubSubManager>,
}

impl ManagedSubscription {
    /// Create a new managed subscription
    pub fn new(
        receiver: broadcast::Receiver<Vec<u8>>,
        bridge: Arc<PubSubBridge>,
        manager: Arc<PubSubManager>,
    ) -> Self {
        Self {
            receiver,
            bridge,
            manager,
        }
    }

    /// Receive the next message
    pub async fn recv(&mut self) -> Result<Vec<u8>, broadcast::error::RecvError> {
        self.receiver.recv().await
    }
}

impl Drop for ManagedSubscription {
    fn drop(&mut self) {
        let remaining = self.bridge.remove_subscriber();
        let topic = self.bridge.topic().to_string();

        tracing::trace!(topic, remaining, "Subscription dropped");

        if remaining == 0 {
            // Last subscriber - schedule bridge cleanup
            // We can't do async work in Drop, so we spawn a task
            let manager = Arc::clone(&self.manager);
            tokio::spawn(async move {
                // Small delay to allow for quick re-subscribe patterns
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                manager.remove_bridge(&topic);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bridge_lifecycle() {
        let manager = Arc::new(PubSubManager::new(100));

        // Create first subscriber
        let (bridge, is_new) = manager.get_or_create_bridge("test");
        assert!(is_new);
        bridge.add_subscriber();
        assert_eq!(bridge.subscriber_count(), 1);

        // Second subscriber reuses bridge
        let (bridge2, is_new2) = manager.get_or_create_bridge("test");
        assert!(!is_new2);
        bridge2.add_subscriber();
        assert_eq!(bridge.subscriber_count(), 2);

        // Remove subscribers
        bridge.remove_subscriber();
        assert_eq!(bridge.subscriber_count(), 1);
        bridge.remove_subscriber();
        assert_eq!(bridge.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn test_managed_subscription_cleanup() {
        let manager = Arc::new(PubSubManager::new(100));

        // Create subscription
        let (bridge, _) = manager.get_or_create_bridge("test");
        bridge.add_subscriber();
        let receiver = bridge.subscribe();

        let sub = ManagedSubscription::new(receiver, bridge, Arc::clone(&manager));

        // Verify bridge exists
        assert!(manager.get_bridge("test").is_some());

        // Drop subscription
        drop(sub);

        // Wait for cleanup task
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // Bridge should be removed
        assert!(manager.get_bridge("test").is_none());
    }

    #[tokio::test]
    async fn test_shutdown() {
        let manager = Arc::new(PubSubManager::new(100));

        // Create some bridges
        let (bridge1, _) = manager.get_or_create_bridge("topic1");
        bridge1.add_subscriber();
        let (bridge2, _) = manager.get_or_create_bridge("topic2");
        bridge2.add_subscriber();

        // Shutdown
        manager.shutdown().await;

        // Bridges should be cleared
        assert!(manager.get_bridge("topic1").is_none());
        assert!(manager.get_bridge("topic2").is_none());
    }
}
