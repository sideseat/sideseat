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
    pub status_code: i32,
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
    pub root_span_id: Option<String>,
    pub root_span_name: Option<String>,
    pub service_name: String,
    pub detected_framework: String,
    pub span_count: i32,
    pub start_time_ns: i64,
    pub end_time_ns: Option<i64>,
    pub duration_ns: Option<i64>,
    pub total_input_tokens: Option<i64>,
    pub total_output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_span_event() -> SpanEvent {
        SpanEvent {
            trace_id: "trace123".to_string(),
            span_id: "span456".to_string(),
            parent_span_id: None,
            span_name: "test_span".to_string(),
            service_name: "test_service".to_string(),
            detected_framework: "unknown".to_string(),
            detected_category: Some("llm".to_string()),
            start_time_ns: 1000000000,
            end_time_ns: Some(2000000000),
            duration_ns: Some(1000000000),
            status_code: 0,
            gen_ai_agent_name: Some("agent1".to_string()),
            gen_ai_tool_name: None,
            gen_ai_request_model: Some("gpt-4".to_string()),
            usage_input_tokens: Some(100),
            usage_output_tokens: Some(50),
        }
    }

    #[test]
    fn test_span_event_serialization() {
        let event = sample_span_event();
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"trace_id\":\"trace123\""));
        assert!(json.contains("\"span_name\":\"test_span\""));

        let deserialized: SpanEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.trace_id, event.trace_id);
        assert_eq!(deserialized.span_id, event.span_id);
    }

    #[test]
    fn test_trace_event_new_span() {
        let span = sample_span_event();
        let event = TraceEvent::NewSpan(span);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"NewSpan\""));
        assert!(json.contains("\"data\""));
    }

    #[test]
    fn test_trace_event_span_updated() {
        let span = sample_span_event();
        let event = TraceEvent::SpanUpdated(span);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"SpanUpdated\""));
    }

    #[test]
    fn test_trace_completed_event_serialization() {
        let event = TraceCompletedEvent {
            trace_id: "trace123".to_string(),
            root_span_id: Some("root".to_string()),
            root_span_name: Some("main".to_string()),
            service_name: "svc".to_string(),
            detected_framework: "langchain".to_string(),
            span_count: 5,
            start_time_ns: 1000,
            end_time_ns: Some(2000),
            duration_ns: Some(1000),
            total_input_tokens: Some(500),
            total_output_tokens: Some(300),
            total_tokens: Some(800),
            has_errors: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"span_count\":5"));
        assert!(json.contains("\"has_errors\":false"));

        let deserialized: TraceCompletedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.span_count, 5);
    }

    #[test]
    fn test_health_event_serialization() {
        let event = HealthEvent {
            status: "healthy".to_string(),
            disk_usage_percent: 45,
            pending_spans: 10,
            total_traces: 1000,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"status\":\"healthy\""));
        assert!(json.contains("\"disk_usage_percent\":45"));

        let deserialized: HealthEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.disk_usage_percent, 45);
    }

    #[test]
    fn test_trace_event_health_update() {
        let health = HealthEvent {
            status: "degraded".to_string(),
            disk_usage_percent: 80,
            pending_spans: 100,
            total_traces: 5000,
        };
        let event = TraceEvent::HealthUpdate(health);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"HealthUpdate\""));
    }

    #[test]
    fn test_event_payload_new() {
        let span = sample_span_event();
        let event = TraceEvent::NewSpan(span);
        let payload = EventPayload::new(event);

        assert!(payload.timestamp > 0);
        assert!(matches!(payload.event, TraceEvent::NewSpan(_)));
    }

    #[test]
    fn test_event_payload_serialization() {
        let span = sample_span_event();
        let event = TraceEvent::NewSpan(span);
        let payload = EventPayload::new(event);

        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"event\""));
        assert!(json.contains("\"timestamp\""));
    }

    #[test]
    fn test_span_event_clone() {
        let event = sample_span_event();
        let cloned = event.clone();
        assert_eq!(cloned.trace_id, event.trace_id);
        assert_eq!(cloned.span_name, event.span_name);
    }

    #[test]
    fn test_trace_event_clone() {
        let span = sample_span_event();
        let event = TraceEvent::NewSpan(span);
        let cloned = event.clone();
        assert!(matches!(cloned, TraceEvent::NewSpan(_)));
    }
}
