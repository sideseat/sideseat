//! Bounded ingestion channel with backpressure

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

use crate::otel::normalize::NormalizedSpan;

/// Bounded ingestion channel with memory tracking
pub struct IngestionChannel {
    sender: mpsc::Sender<NormalizedSpan>,
    receiver: mpsc::Receiver<NormalizedSpan>,
    pending_count: Arc<AtomicUsize>,
    pending_bytes: Arc<AtomicUsize>,
    max_spans: usize,
    max_bytes: usize,
}

impl IngestionChannel {
    /// Create a new bounded ingestion channel
    pub fn new(capacity: usize, max_spans: usize, max_bytes: usize) -> Self {
        let (sender, receiver) = mpsc::channel(capacity);
        Self {
            sender,
            receiver,
            pending_count: Arc::new(AtomicUsize::new(0)),
            pending_bytes: Arc::new(AtomicUsize::new(0)),
            max_spans,
            max_bytes,
        }
    }

    /// Get a sender handle for the channel
    pub fn sender(&self) -> ChannelSender {
        ChannelSender {
            sender: self.sender.clone(),
            pending_count: self.pending_count.clone(),
            pending_bytes: self.pending_bytes.clone(),
            max_spans: self.max_spans,
            max_bytes: self.max_bytes,
        }
    }

    /// Get the receiver (takes ownership)
    pub fn into_receiver(self) -> ChannelReceiver {
        ChannelReceiver {
            receiver: self.receiver,
            pending_count: self.pending_count,
            pending_bytes: self.pending_bytes,
        }
    }

    /// Get current pending count
    pub fn pending_count(&self) -> usize {
        self.pending_count.load(Ordering::Relaxed)
    }

    /// Get current pending bytes
    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes.load(Ordering::Relaxed)
    }
}

/// Sender handle for the ingestion channel
#[derive(Clone)]
pub struct ChannelSender {
    sender: mpsc::Sender<NormalizedSpan>,
    pending_count: Arc<AtomicUsize>,
    pending_bytes: Arc<AtomicUsize>,
    max_spans: usize,
    max_bytes: usize,
}

impl ChannelSender {
    /// Try to send a span, returning error if backpressure is applied
    pub async fn send(&self, span: NormalizedSpan) -> Result<(), ChannelError> {
        // Check limits
        let current_count = self.pending_count.load(Ordering::Relaxed);
        if current_count >= self.max_spans {
            return Err(ChannelError::SpanLimitReached);
        }

        let span_size = estimate_span_size(&span);
        let current_bytes = self.pending_bytes.load(Ordering::Relaxed);
        if current_bytes + span_size > self.max_bytes {
            return Err(ChannelError::ByteLimitReached);
        }

        // Update counters
        self.pending_count.fetch_add(1, Ordering::Relaxed);
        self.pending_bytes.fetch_add(span_size, Ordering::Relaxed);

        // Send
        self.sender.send(span).await.map_err(|_| ChannelError::ChannelClosed)
    }

    /// Check if channel is at capacity
    pub fn is_at_capacity(&self) -> bool {
        let count = self.pending_count.load(Ordering::Relaxed);
        let bytes = self.pending_bytes.load(Ordering::Relaxed);
        count >= self.max_spans || bytes >= self.max_bytes
    }
}

/// Receiver handle for the ingestion channel
pub struct ChannelReceiver {
    receiver: mpsc::Receiver<NormalizedSpan>,
    pending_count: Arc<AtomicUsize>,
    pending_bytes: Arc<AtomicUsize>,
}

impl ChannelReceiver {
    /// Receive a span from the channel
    pub async fn recv(&mut self) -> Option<NormalizedSpan> {
        let span = self.receiver.recv().await?;

        // Update counters
        let span_size = estimate_span_size(&span);
        self.pending_count.fetch_sub(1, Ordering::Relaxed);
        self.pending_bytes.fetch_sub(span_size, Ordering::Relaxed);

        Some(span)
    }

    /// Try to receive a batch of spans
    pub async fn recv_batch(&mut self, max_size: usize) -> Vec<NormalizedSpan> {
        let mut batch = Vec::with_capacity(max_size);

        // Wait for at least one span
        if let Some(span) = self.recv().await {
            batch.push(span);
        } else {
            return batch;
        }

        // Try to receive more without waiting
        while batch.len() < max_size {
            match self.receiver.try_recv() {
                Ok(span) => {
                    let span_size = estimate_span_size(&span);
                    self.pending_count.fetch_sub(1, Ordering::Relaxed);
                    self.pending_bytes.fetch_sub(span_size, Ordering::Relaxed);
                    batch.push(span);
                }
                Err(_) => break,
            }
        }

        batch
    }
}

/// Channel errors
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("Span limit reached")]
    SpanLimitReached,
    #[error("Byte limit reached")]
    ByteLimitReached,
    #[error("Channel closed")]
    ChannelClosed,
}

/// Estimate the memory size of a span
fn estimate_span_size(span: &NormalizedSpan) -> usize {
    // Base struct size + string contents
    std::mem::size_of::<NormalizedSpan>()
        + span.trace_id.len()
        + span.span_id.len()
        + span.parent_span_id.as_ref().map(|s| s.len()).unwrap_or(0)
        + span.service_name.len()
        + span.span_name.len()
        + span.attributes_json.len()
        + span.resource_attributes_json.as_ref().map(|s| s.len()).unwrap_or(0)
}
