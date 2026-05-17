//! Per-process state for the SDK WebSocket subsystem.

use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::data::registrations::RegistrationStore;
use crate::data::topics::TopicService;

use super::chunks::Reassembler;

/// One per server process. Lives inside `WsState`.
pub struct ConnectionHandle {
    pub connection_id: String,
    pub project_id: String,
    /// Set after the SDK sends `hello`.
    pub client_id: Mutex<Option<String>>,
    /// Outbound queue for serialized frames (JSON strings).
    pub outbound: mpsc::Sender<String>,
}

#[derive(Clone)]
pub struct WsState {
    pub instance_id: Arc<String>,
    pub topics: Arc<TopicService>,
    pub registrations: Arc<dyn RegistrationStore>,
    pub connections: Arc<DashMap<String, Arc<ConnectionHandle>>>,
    pub shutdown_rx: tokio::sync::watch::Receiver<bool>,
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
    ) -> Self {
        Self {
            instance_id: Arc::new(Uuid::new_v4().to_string()),
            topics,
            registrations,
            connections: Arc::new(DashMap::new()),
            shutdown_rx,
            reassembler: Reassembler::new(),
            invoke_timeout_override: None,
        }
    }

    pub fn make_connection_id(&self) -> String {
        format!("{}:{}", self.instance_id, Uuid::new_v4())
    }
}
