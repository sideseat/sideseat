//! Domain logic for LLM observability
//!
//! - `metrics` - OpenTelemetry metrics processing pipeline
//! - `pricing` - LLM cost calculation and model pricing
//! - `sideml` - Universal AI message format normalization
//! - `traces` - OpenTelemetry trace processing pipeline

pub mod metrics;
pub mod pricing;
pub mod sideml;
pub mod traces;

pub use metrics::MetricsPipeline;
pub use traces::{MessageSource, RawMessage, SseSpanEvent, TracePipeline};

use crate::core::TopicMessage;
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};

impl TopicMessage for ExportTraceServiceRequest {
    fn size_bytes(&self) -> usize {
        self.resource_spans
            .iter()
            .flat_map(|rs| &rs.scope_spans)
            .map(|ss| ss.spans.len() * 500)
            .sum::<usize>()
            .max(100)
    }
}

impl TopicMessage for ExportMetricsServiceRequest {
    fn size_bytes(&self) -> usize {
        self.resource_metrics
            .iter()
            .flat_map(|rm| &rm.scope_metrics)
            .map(|sm| sm.metrics.len() * 200)
            .sum::<usize>()
            .max(100)
    }
}

impl TopicMessage for ExportLogsServiceRequest {
    fn size_bytes(&self) -> usize {
        self.resource_logs
            .iter()
            .flat_map(|rl| &rl.scope_logs)
            .map(|sl| sl.log_records.len() * 300)
            .sum::<usize>()
            .max(100)
    }
}
