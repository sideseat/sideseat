//! Bounded write buffer for batch processing

use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;

use crate::otel::normalize::NormalizedSpan;

/// Write buffer with bounded memory
pub struct WriteBuffer {
    spans: Mutex<Vec<NormalizedSpan>>,
    current_count: AtomicUsize,
    current_bytes: AtomicUsize,
    max_spans: usize,
    max_bytes: usize,
}

impl WriteBuffer {
    /// Create a new write buffer with limits
    pub fn new(max_spans: usize, max_bytes: usize) -> Self {
        Self {
            spans: Mutex::new(Vec::with_capacity(max_spans.min(10000))),
            current_count: AtomicUsize::new(0),
            current_bytes: AtomicUsize::new(0),
            max_spans,
            max_bytes,
        }
    }

    /// Add a span to the buffer
    /// Returns true if the buffer should be flushed (at capacity)
    pub async fn push(&self, span: NormalizedSpan) -> bool {
        let span_size = Self::estimate_size(&span);

        let mut spans = self.spans.lock().await;
        spans.push(span);

        let count = self.current_count.fetch_add(1, Ordering::Relaxed) + 1;
        let bytes = self.current_bytes.fetch_add(span_size, Ordering::Relaxed) + span_size;

        count >= self.max_spans || bytes >= self.max_bytes
    }

    /// Drain all spans from the buffer
    pub async fn drain(&self) -> Vec<NormalizedSpan> {
        let mut spans = self.spans.lock().await;
        let drained = std::mem::take(&mut *spans);

        self.current_count.store(0, Ordering::Relaxed);
        self.current_bytes.store(0, Ordering::Relaxed);

        drained
    }

    /// Get current span count
    pub fn count(&self) -> usize {
        self.current_count.load(Ordering::Relaxed)
    }

    /// Get current byte count
    pub fn bytes(&self) -> usize {
        self.current_bytes.load(Ordering::Relaxed)
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.current_count.load(Ordering::Relaxed) == 0
    }

    /// Check if buffer is at capacity
    pub fn is_at_capacity(&self) -> bool {
        let count = self.current_count.load(Ordering::Relaxed);
        let bytes = self.current_bytes.load(Ordering::Relaxed);
        count >= self.max_spans || bytes >= self.max_bytes
    }

    /// Estimate memory size of a span
    fn estimate_size(span: &NormalizedSpan) -> usize {
        std::mem::size_of::<NormalizedSpan>()
            + span.trace_id.len()
            + span.span_id.len()
            + span.parent_span_id.as_ref().map(|s| s.len()).unwrap_or(0)
            + span.service_name.len()
            + span.span_name.len()
            + span.attributes_json.len()
            + span.resource_attributes_json.as_ref().map(|s| s.len()).unwrap_or(0)
    }
}

impl Default for WriteBuffer {
    fn default() -> Self {
        Self::new(10000, 50 * 1024 * 1024) // 10k spans or 50MB
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_span(trace_id: &str, span_id: &str) -> NormalizedSpan {
        NormalizedSpan {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            service_name: "test-service".to_string(),
            span_name: "test-span".to_string(),
            attributes_json: "{}".to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_buffer_new() {
        let buffer = WriteBuffer::new(100, 1000);
        assert_eq!(buffer.count(), 0);
        assert_eq!(buffer.bytes(), 0);
        assert!(buffer.is_empty());
        assert!(!buffer.is_at_capacity());
    }

    #[tokio::test]
    async fn test_buffer_push_single() {
        let buffer = WriteBuffer::new(100, 100000);
        let span = create_test_span("trace1", "span1");

        let should_flush = buffer.push(span).await;

        assert!(!should_flush);
        assert_eq!(buffer.count(), 1);
        assert!(buffer.bytes() > 0);
        assert!(!buffer.is_empty());
    }

    #[tokio::test]
    async fn test_buffer_push_triggers_flush_by_count() {
        let buffer = WriteBuffer::new(2, 100000);

        let span1 = create_test_span("trace1", "span1");
        let span2 = create_test_span("trace1", "span2");

        assert!(!buffer.push(span1).await);
        assert!(buffer.push(span2).await); // Should trigger flush
        assert!(buffer.is_at_capacity());
    }

    #[tokio::test]
    async fn test_buffer_push_triggers_flush_by_bytes() {
        let buffer = WriteBuffer::new(1000, 100); // Very small byte limit

        let span = create_test_span("trace1", "span1");
        // This should exceed the 100 byte limit
        assert!(buffer.push(span).await);
    }

    #[tokio::test]
    async fn test_buffer_drain() {
        let buffer = WriteBuffer::new(100, 100000);

        buffer.push(create_test_span("trace1", "span1")).await;
        buffer.push(create_test_span("trace1", "span2")).await;

        assert_eq!(buffer.count(), 2);

        let drained = buffer.drain().await;

        assert_eq!(drained.len(), 2);
        assert_eq!(buffer.count(), 0);
        assert_eq!(buffer.bytes(), 0);
        assert!(buffer.is_empty());
    }

    #[tokio::test]
    async fn test_buffer_drain_empty() {
        let buffer = WriteBuffer::new(100, 100000);
        let drained = buffer.drain().await;
        assert!(drained.is_empty());
    }

    #[tokio::test]
    async fn test_buffer_is_at_capacity() {
        let buffer = WriteBuffer::new(2, 100000);

        assert!(!buffer.is_at_capacity());
        buffer.push(create_test_span("trace1", "span1")).await;
        assert!(!buffer.is_at_capacity());
        buffer.push(create_test_span("trace1", "span2")).await;
        assert!(buffer.is_at_capacity());
    }

    #[tokio::test]
    async fn test_buffer_default() {
        let buffer = WriteBuffer::default();
        assert_eq!(buffer.max_spans, 10000);
        assert_eq!(buffer.max_bytes, 50 * 1024 * 1024);
    }

    #[tokio::test]
    async fn test_estimate_size() {
        let span = create_test_span("trace123", "span456");
        let size = WriteBuffer::estimate_size(&span);

        // Should include struct size plus string lengths
        assert!(size >= std::mem::size_of::<NormalizedSpan>());
        assert!(size >= "trace123".len() + "span456".len());
    }
}
