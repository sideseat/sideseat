//! Per-process state for the SDK WebSocket subsystem.

use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use uuid::Uuid;

use sideseat_ingestion::topics::TopicService;
use sideseat_ports::clock::Clock;
use sideseat_ports::registrations::RegistrationStore;
use sideseat_ports::types::ProjectId;

use super::chunks::Reassembler;

/// One per server process. Lives inside `WsState`.
pub struct ConnectionHandle {
    pub connection_id: String,
    pub project_id: ProjectId,
    /// Set after the SDK sends `hello`.
    pub client_id: Mutex<Option<String>>,
    /// Outbound queue for serialized frames (JSON strings).
    pub outbound: mpsc::Sender<String>,
    /// Fires when this connection must stop, whatever the client does.
    ///
    /// The protocol says a `replaced` connection does not survive, and the server only *queued* the notice:
    /// nothing closed the socket or ended its receive loop, so the guarantee rested on the client
    /// disconnecting voluntarily. The official SDKs do; a client that ignores the frame kept registering and
    /// publishing events under a name it no longer owned.
    pub close: Arc<tokio::sync::Notify>,
}

#[derive(Clone)]
pub struct WsState {
    pub instance_id: Arc<String>,
    pub topics: Arc<TopicService>,
    pub registrations: Arc<dyn RegistrationStore>,
    pub connections: Arc<DashMap<String, Arc<ConnectionHandle>>>,
    pub shutdown_rx: tokio::sync::watch::Receiver<bool>,
    pub clock: Arc<dyn Clock>,
    /// Process-wide reassembly buffer for chunked AG-UI events.
    pub reassembler: Reassembler,
    /// Test-only: override the default invoke timeout. Production keeps
    /// `None` and the AG-UI route falls back to `INVOKE_TIMEOUT_MS`.
    pub invoke_timeout_override: Option<std::time::Duration>,
}

impl WsState {
    pub fn new(
        topics: Arc<TopicService>,
        registrations: Arc<dyn RegistrationStore>,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            instance_id: Arc::new(Uuid::new_v4().to_string()),
            topics,
            registrations,
            connections: Arc::new(DashMap::new()),
            shutdown_rx,
            clock,
            reassembler: Reassembler::new(),
            invoke_timeout_override: None,
        }
    }

    pub fn make_connection_id(&self) -> String {
        format!("{}:{}", self.instance_id, Uuid::new_v4())
    }

    pub fn now_secs(&self) -> u64 {
        self.clock.now().timestamp().try_into().unwrap_or(0)
    }
}
