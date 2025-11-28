//! Server-Sent Events endpoint

use axum::{
    Router,
    extract::{Query, State},
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
    routing::get,
};
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;

use crate::otel::query::{AttributeFilter, TraceFilter};
use crate::otel::{OtelError, OtelManager};

/// Create SSE routes
pub fn create_routes(otel: Arc<OtelManager>) -> Router {
    Router::new().route("/", get(handle_sse)).with_state(otel)
}

/// Query params for SSE subscription
#[derive(Debug, Deserialize)]
pub struct SseQueryParams {
    pub service: Option<String>,
    pub framework: Option<String>,
    pub agent: Option<String>,
    pub errors_only: Option<bool>,
    /// Text search in span/service names
    pub search: Option<String>,
    /// Attribute filters as JSON string (same format as trace list API).
    /// Only filters on attributes present in SpanEvent (not full span data).
    pub attributes: Option<String>,
}

/// Guard to ensure SSE cleanup on drop (including panic)
struct SseCleanupGuard {
    sub_id: u64,
    sse_manager: Arc<crate::otel::realtime::SseManager>,
    cleaned_up: bool,
}

impl SseCleanupGuard {
    fn new(sub_id: u64, sse_manager: Arc<crate::otel::realtime::SseManager>) -> Self {
        Self { sub_id, sse_manager, cleaned_up: false }
    }

    async fn cleanup(&mut self) {
        if !self.cleaned_up {
            self.cleaned_up = true;
            self.sse_manager.unsubscribe(self.sub_id).await;
        }
    }
}

impl Drop for SseCleanupGuard {
    fn drop(&mut self) {
        if !self.cleaned_up {
            // Spawn cleanup task if we're being dropped without explicit cleanup
            let sub_id = self.sub_id;
            let sse_manager = self.sse_manager.clone();
            tokio::spawn(async move {
                sse_manager.unsubscribe(sub_id).await;
            });
        }
    }
}

/// GET /otel/sse/traces - SSE stream of trace events
pub async fn handle_sse(
    State(otel): State<Arc<OtelManager>>,
    Query(params): Query<SseQueryParams>,
) -> impl IntoResponse {
    // Parse attribute filters from JSON string
    let attributes = params
        .attributes
        .as_ref()
        .and_then(|s| serde_json::from_str::<Vec<AttributeFilter>>(s).ok())
        .unwrap_or_default();

    let filter = TraceFilter {
        service_name: params.service,
        framework: params.framework,
        agent_name: params.agent,
        has_errors: params.errors_only,
        search: params.search,
        attributes,
        ..Default::default()
    };

    // Subscribe to SSE
    match otel.sse.subscribe(filter.clone()).await {
        Ok(subscription) => {
            let matcher = subscription.matcher;
            let mut receiver = subscription.receiver;
            let sub_id = subscription.id;
            let sse_manager = otel.sse.clone();
            let keepalive = otel.sse.keepalive_interval();

            // Create the SSE stream with cleanup guard
            let stream = async_stream::stream! {
                let mut guard = SseCleanupGuard::new(sub_id, sse_manager.clone());
                let mut keepalive_interval = tokio::time::interval(keepalive);

                loop {
                    tokio::select! {
                        // Handle incoming events
                        result = receiver.recv() => {
                            match result {
                                Ok(payload) => {
                                    // Check if event matches filter
                                    if matcher.matches(&payload.event) {
                                        let json = serde_json::to_string(&payload).unwrap_or_default();
                                        yield Ok::<_, Infallible>(Event::default().data(json));
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                    // Missed some messages, continue
                                    continue;
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    break;
                                }
                            }
                        }
                        // Send keepalive
                        _ = keepalive_interval.tick() => {
                            yield Ok(Event::default().comment("keepalive"));
                            sse_manager.touch(sub_id).await;
                        }
                    }
                }

                // Explicit cleanup on normal disconnect
                guard.cleanup().await;
            };

            Sse::new(stream).into_response()
        }
        Err(_) => OtelError::TooManyConnections.into_response(),
    }
}
