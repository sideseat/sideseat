//! Per-invoke pub/sub plumbing.
//!
//! The WS handler receives `agent.event / agent.complete / agent.error`
//! frames from the SDK and republishes them onto a per-request topic so the
//! HTTP `/agents/{name}/runs` SSE handler — which may live on a different
//! server instance — can fan them out to its client.
//!
//! Topic name: `agent_request:{request_id}`.

use serde::{Deserialize, Serialize};

use crate::data::topics::TopicMessage;

use super::protocol::ErrorCode;
use super::state::WsState;

/// Single message carried over `agent_request:{request_id}` topic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvokeReply {
    /// AG-UI event payload, alias-serialised. Forwarded verbatim to SSE.
    Event(serde_json::Value),
    /// Natural end of the stream from the SDK. SSE handler closes the pipe.
    Complete,
    /// SDK-level failure for the invoke. SSE handler synthesises a
    /// terminal AG-UI `RUN_ERROR` if no real terminal event flowed yet,
    /// then closes.
    Error {
        code: ErrorCode,
        #[serde(default)]
        message: String,
    },
}

impl TopicMessage for InvokeReply {
    fn size_bytes(&self) -> usize {
        // Event payloads are bounded by the WS frame cap. Cheap upper-bound
        // is fine — the topic backend uses this only for backpressure.
        match self {
            Self::Event(v) => v.to_string().len(),
            Self::Complete => 32,
            Self::Error { message, .. } => 64 + message.len(),
        }
    }
}

pub fn invoke_topic_name(request_id: &str) -> String {
    format!("agent_request:{}", request_id)
}

pub async fn publish_invoke_reply(state: &WsState, request_id: &str, reply: InvokeReply) {
    let topic = state
        .topics
        .broadcast_topic::<InvokeReply>(&invoke_topic_name(request_id));
    if let Err(e) = topic.publish(&reply).await {
        tracing::warn!(error = %e, request_id = %request_id, "ws: invoke reply publish failed");
    }
}
