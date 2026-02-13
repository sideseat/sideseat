//! SSE endpoint for real-time span streaming
//!
//! Uses BroadcastTopic for distributed pub/sub. In Redis mode, events are
//! received via Redis Pub/Sub so all SSE endpoints get events from any worker.

use std::convert::Infallible;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::response::sse::{Event, Sse};
use futures::stream::Stream;
use serde::Deserialize;

use super::OtelApiState;
use crate::api::auth::ProjectRead;
use crate::api::types::ApiError;
use crate::core::TopicError;
use crate::domain::SseSpanEvent;

/// Maximum events per second per SSE connection (backpressure)
const MAX_EVENTS_PER_SECOND: u32 = 10;

#[derive(Debug, Deserialize)]
pub struct SseQuery {
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub session_id: Option<String>,
}

pub async fn sse(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    Query(query): Query<SseQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let project_id = auth.project_id.clone();
    let topic_name = format!("sse_spans:{}", project_id);

    // Subscribe to broadcast topic for distributed SSE
    // In Redis mode, this uses Redis Pub/Sub to receive events from any worker
    let topic = state.topics.broadcast_topic::<SseSpanEvent>(&topic_name);
    let subscriber_result = topic.subscribe().await;
    let mut shutdown_rx = state.shutdown_rx.clone();

    let stream = async_stream::stream! {
        let mut subscriber = match subscriber_result {
            Ok(sub) => sub,
            Err(e) => {
                tracing::error!(error = %e, "Failed to subscribe to SSE topic");
                yield Ok(Event::default().event("error").data("subscription failed"));
                return;
            }
        };
        let mut events_this_second: u32 = 0;
        let mut second_start = Instant::now();
        let mut dropped_count: u64 = 0;

        loop {
            tokio::select! {
                biased;
                // Check for shutdown signal first
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        // Notify client before closing so it can reconnect immediately
                        yield Ok(Event::default().event("terminate").data("shutdown"));
                        break;
                    }
                }
                // Process incoming messages
                result = subscriber.recv() => {
                    match result {
                        Ok(event) => {
                            // Skip events that don't match filter (before rate limiting)
                            if !matches_filter(&event, &query) {
                                continue;
                            }

                            // Reset rate limit counter every second
                            if second_start.elapsed() >= Duration::from_secs(1) {
                                if dropped_count > 0 {
                                    tracing::debug!(dropped = dropped_count, "SSE events dropped due to rate limit");
                                }
                                events_this_second = 0;
                                dropped_count = 0;
                                second_start = Instant::now();
                            }

                            // Apply backpressure: drop events if rate limit exceeded
                            if events_this_second >= MAX_EVENTS_PER_SECOND {
                                dropped_count += 1;
                                continue;
                            }

                            match serde_json::to_string(&event) {
                                Ok(data) => {
                                    events_this_second += 1;
                                    yield Ok(Event::default()
                                        .event("span")
                                        .data(data));
                                }
                                Err(e) => {
                                    tracing::error!(error = %e, "Failed to serialize SSE event");
                                }
                            }
                        }
                        Err(TopicError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "SSE subscriber lagged behind");
                        }
                        Err(TopicError::ChannelClosed) => break,
                        Err(_) => break,
                    }
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keep-alive"),
    ))
}

fn matches_filter(event: &SseSpanEvent, query: &SseQuery) -> bool {
    // project_id filtering is handled by per-project topics
    if let Some(ref trace_id) = query.trace_id
        && &event.trace_id != trace_id
    {
        return false;
    }
    if let Some(ref span_id) = query.span_id
        && &event.span_id != span_id
    {
        return false;
    }
    if let Some(ref session_id) = query.session_id
        && event.session_id.as_ref() != Some(session_id)
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(
        project_id: Option<&str>,
        trace_id: &str,
        span_id: &str,
        session_id: Option<&str>,
    ) -> SseSpanEvent {
        SseSpanEvent {
            project_id: project_id.map(|s| s.to_string()),
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            session_id: session_id.map(|s| s.to_string()),
            user_id: None,
        }
    }

    #[test]
    fn test_matches_filter_no_filters() {
        let event = make_event(Some("proj1"), "trace1", "span1", Some("session1"));
        let query = SseQuery {
            trace_id: None,
            span_id: None,
            session_id: None,
        };
        assert!(matches_filter(&event, &query));
    }

    // project_id filtering is handled by per-project topics, not matches_filter

    #[test]
    fn test_matches_filter_trace_id_match() {
        let event = make_event(Some("proj1"), "trace1", "span1", Some("session1"));
        let query = SseQuery {
            trace_id: Some("trace1".to_string()),
            span_id: None,
            session_id: None,
        };
        assert!(matches_filter(&event, &query));
    }

    #[test]
    fn test_matches_filter_trace_id_no_match() {
        let event = make_event(Some("proj1"), "trace1", "span1", Some("session1"));
        let query = SseQuery {
            trace_id: Some("trace2".to_string()),
            span_id: None,
            session_id: None,
        };
        assert!(!matches_filter(&event, &query));
    }

    #[test]
    fn test_matches_filter_span_id_match() {
        let event = make_event(Some("proj1"), "trace1", "span1", Some("session1"));
        let query = SseQuery {
            trace_id: None,
            span_id: Some("span1".to_string()),
            session_id: None,
        };
        assert!(matches_filter(&event, &query));
    }

    #[test]
    fn test_matches_filter_span_id_no_match() {
        let event = make_event(Some("proj1"), "trace1", "span1", Some("session1"));
        let query = SseQuery {
            trace_id: None,
            span_id: Some("span2".to_string()),
            session_id: None,
        };
        assert!(!matches_filter(&event, &query));
    }

    #[test]
    fn test_matches_filter_session_id_match() {
        let event = make_event(Some("proj1"), "trace1", "span1", Some("session1"));
        let query = SseQuery {
            trace_id: None,
            span_id: None,
            session_id: Some("session1".to_string()),
        };
        assert!(matches_filter(&event, &query));
    }

    #[test]
    fn test_matches_filter_session_id_no_match() {
        let event = make_event(Some("proj1"), "trace1", "span1", Some("session1"));
        let query = SseQuery {
            trace_id: None,
            span_id: None,
            session_id: Some("session2".to_string()),
        };
        assert!(!matches_filter(&event, &query));
    }

    #[test]
    fn test_matches_filter_session_id_none_event() {
        let event = make_event(Some("proj1"), "trace1", "span1", None);
        let query = SseQuery {
            trace_id: None,
            span_id: None,
            session_id: Some("session1".to_string()),
        };
        assert!(!matches_filter(&event, &query));
    }

    #[test]
    fn test_matches_filter_multiple_filters() {
        let event = make_event(Some("proj1"), "trace1", "span1", Some("session1"));
        let query = SseQuery {
            trace_id: Some("trace1".to_string()),
            span_id: Some("span1".to_string()),
            session_id: Some("session1".to_string()),
        };
        assert!(matches_filter(&event, &query));
    }

    #[test]
    fn test_matches_filter_multiple_filters_one_no_match() {
        let event = make_event(Some("proj1"), "trace1", "span1", Some("session1"));
        let query = SseQuery {
            trace_id: Some("trace1".to_string()),
            span_id: Some("span2".to_string()),
            session_id: Some("session1".to_string()),
        };
        assert!(!matches_filter(&event, &query));
    }
}
