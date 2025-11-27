//! Real-time event types

use serde::{Deserialize, Serialize};

/// Real-time trace events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TraceEvent {
    /// New span received
    NewSpan(SpanEvent),

    /// Span updated (ended)
    SpanUpdated(SpanEvent),

    /// Trace completed (all spans ended)
    TraceCompleted(TraceCompletedEvent),

    /// System health update
    HealthUpdate(HealthEvent),
}

/// Span event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanEvent {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub span_name: String,
    pub service_name: String,
    pub detected_framework: String,
    pub detected_category: Option<String>,
    pub start_time_ns: i64,
    pub end_time_ns: Option<i64>,
    pub duration_ns: Option<i64>,
    pub status_code: i8,
    pub gen_ai_agent_name: Option<String>,
    pub gen_ai_tool_name: Option<String>,
    pub gen_ai_request_model: Option<String>,
    pub usage_input_tokens: Option<i64>,
    pub usage_output_tokens: Option<i64>,
}

/// Trace completed event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceCompletedEvent {
    pub trace_id: String,
    pub service_name: String,
    pub span_count: i32,
    pub total_duration_ns: Option<i64>,
    pub total_input_tokens: Option<i64>,
    pub total_output_tokens: Option<i64>,
    pub has_errors: bool,
}

/// Health event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthEvent {
    pub status: String,
    pub disk_usage_percent: u8,
    pub pending_spans: usize,
    pub total_traces: i64,
}

/// Generic event payload wrapper
#[derive(Debug, Clone, Serialize)]
pub struct EventPayload {
    pub event: TraceEvent,
    pub timestamp: i64,
}

impl EventPayload {
    pub fn new(event: TraceEvent) -> Self {
        Self { event, timestamp: chrono::Utc::now().timestamp_millis() }
    }
}
