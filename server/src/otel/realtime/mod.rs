//! Real-time updates via Server-Sent Events

mod broadcast;
mod events;
mod matcher;
mod sse;

pub use broadcast::EventBroadcaster;
pub use events::{EventPayload, SpanEvent, TraceEvent};
pub use matcher::EventMatcher;
pub use sse::{SseManager, SseSubscription};
