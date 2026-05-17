//! Wire-level frame types for the SDK WebSocket channel (v1).
//!
//! See `server/protocol/ws-v1/schema.json` and `codes.md` for the source of
//! truth.

use serde::{Deserialize, Serialize};

use crate::data::registrations::{RegistrationKind, RegistrationManifest};

pub const PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unsupported,
    BadPayload,
    TooLarge,
    InvalidProjectId,
    HelloRequired,
    Replaced,
    RateLimited,
    Internal,
    AgentNotRegistered,
    AgentBusy,
    InvokeTimeout,
    Cancelled,
    AguiExtraMissing,
    BadRunInput,
    UnsupportedRuntime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    #[serde(rename = "type")]
    pub r#type: String,
    pub id: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

// ============================================================================
// Inbound payloads
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct HelloPayload {
    pub client_id: String,
    #[allow(dead_code)] // forwarded into logs only
    pub sdk_version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UnregisterPayload {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PongPayload {
    #[allow(dead_code)] // logged only
    pub id: String,
}

/// SDK→server: an AG-UI event flowing back from a running invoke.
/// `event` is the alias-serialised AG-UI event JSON; the server forwards
/// it verbatim onto the per-request topic for the SSE handler.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentEventPayload {
    pub request_id: String,
    pub event: serde_json::Value,
}

/// SDK→server: one slice of an AG-UI event that was too big to fit in a
/// single WS frame. See `server/protocol/ws-v1/chunking.md`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentEventChunkPayload {
    pub request_id: String,
    pub group_id: String,
    pub idx: usize,
    pub total: usize,
    pub data_b64: String,
}

/// SDK→server: terminal marker after the AG-UI stream ends naturally.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentCompletePayload {
    pub request_id: String,
}

/// SDK→server: terminal error for an in-flight invoke.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentErrorPayload {
    pub request_id: String,
    pub code: ErrorCode,
    #[serde(default)]
    pub message: String,
}

// ============================================================================
// Outbound payloads
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct WelcomePayload {
    pub connection_id: String,
    pub server_version: String,
    pub max_message_bytes: usize,
    pub heartbeat_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AckPayload {
    pub ref_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PingPayload {
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplacedPayload {
    pub kind: RegistrationKind,
    pub name: String,
}

/// Server→SDK: kick off an invocation against the named local agent.
#[derive(Debug, Clone, Serialize)]
pub struct AgentInvokePayload {
    pub request_id: String,
    pub agent_name: String,
    pub run_input: serde_json::Value,
}

/// Server→SDK: ask the SDK to abort an in-flight invoke.
#[derive(Debug, Clone, Serialize)]
pub struct AgentCancelPayload {
    pub request_id: String,
}

// ============================================================================
// Helpers
// ============================================================================

/// Build a wire-ready frame as a JSON string. Using `serde_json::to_string`
/// directly avoids the intermediate `serde_json::Value` allocation that an
/// `serde_json::json!` macro would force.
pub fn frame_string<S: serde::Serialize>(r#type: &str, id: &str, payload: &S) -> String {
    #[derive(Serialize)]
    struct Frame<'a, P: Serialize> {
        v: u8,
        #[serde(rename = "type")]
        ty: &'a str,
        id: &'a str,
        payload: &'a P,
    }
    serde_json::to_string(&Frame {
        v: PROTOCOL_VERSION,
        ty: r#type,
        id,
        payload,
    })
    .unwrap_or_else(|e| {
        // Should never happen for our typed payloads; emit a minimal safe fallback.
        tracing::error!(error = %e, "frame_string serialize failed");
        format!("{{\"v\":1,\"type\":\"error\",\"id\":\"{}\",\"payload\":{{\"code\":\"internal\",\"message\":\"frame serialize failed\"}}}}", id)
    })
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // bounded by WS frame cap
pub enum InboundFrame {
    Hello(HelloPayload),
    Register(RegistrationKind, RegistrationManifest),
    Unregister(RegistrationKind, UnregisterPayload),
    Pong(PongPayload),
    AgentEvent(AgentEventPayload),
    AgentEventChunk(AgentEventChunkPayload),
    AgentComplete(AgentCompletePayload),
    AgentError(AgentErrorPayload),
    Unknown(String),
}

pub fn parse_inbound(env: &Envelope) -> Result<InboundFrame, FrameParseError> {
    if env.v != PROTOCOL_VERSION {
        return Err(FrameParseError::BadPayload(format!(
            "unsupported protocol version: {}",
            env.v
        )));
    }
    match env.r#type.as_str() {
        "hello" => parse_payload::<HelloPayload>(env).map(InboundFrame::Hello),
        "agent.register" => parse_payload::<RegistrationManifest>(env)
            .map(|m| InboundFrame::Register(RegistrationKind::Agent, m)),
        "agent.unregister" => parse_payload::<UnregisterPayload>(env)
            .map(|p| InboundFrame::Unregister(RegistrationKind::Agent, p)),
        "mcp.register" => parse_payload::<RegistrationManifest>(env)
            .map(|m| InboundFrame::Register(RegistrationKind::Mcp, m)),
        "mcp.unregister" => parse_payload::<UnregisterPayload>(env)
            .map(|p| InboundFrame::Unregister(RegistrationKind::Mcp, p)),
        "swarm.register" => parse_payload::<RegistrationManifest>(env)
            .map(|m| InboundFrame::Register(RegistrationKind::Swarm, m)),
        "swarm.unregister" => parse_payload::<UnregisterPayload>(env)
            .map(|p| InboundFrame::Unregister(RegistrationKind::Swarm, p)),
        "graph.register" => parse_payload::<RegistrationManifest>(env)
            .map(|m| InboundFrame::Register(RegistrationKind::Graph, m)),
        "graph.unregister" => parse_payload::<UnregisterPayload>(env)
            .map(|p| InboundFrame::Unregister(RegistrationKind::Graph, p)),
        "pong" => parse_payload::<PongPayload>(env).map(InboundFrame::Pong),
        "agent.event" => parse_payload::<AgentEventPayload>(env).map(InboundFrame::AgentEvent),
        "agent.event.chunk" => {
            parse_payload::<AgentEventChunkPayload>(env).map(InboundFrame::AgentEventChunk)
        }
        "agent.complete" => {
            parse_payload::<AgentCompletePayload>(env).map(InboundFrame::AgentComplete)
        }
        "agent.error" => parse_payload::<AgentErrorPayload>(env).map(InboundFrame::AgentError),
        other => Ok(InboundFrame::Unknown(other.to_string())),
    }
}

fn parse_payload<T: serde::de::DeserializeOwned>(env: &Envelope) -> Result<T, FrameParseError> {
    // Avoid a deep clone of the `Value`: borrow it. `from_value` requires
    // owned, but `Value::take`-style move is not safe for `&Envelope`. The
    // borrow form via `Deserialize::deserialize(&value)` keeps the input
    // intact and avoids cloning sub-trees of `tools` / `metadata`.
    T::deserialize(&env.payload).map_err(|e| FrameParseError::BadPayload(e.to_string()))
}

#[derive(Debug, thiserror::Error)]
pub enum FrameParseError {
    #[error("bad payload: {0}")]
    BadPayload(String),
}
