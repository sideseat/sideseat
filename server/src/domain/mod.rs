//! Domain logic for LLM observability
//!
//! - `metrics` - OpenTelemetry metrics processing pipeline
//! - `pricing` - LLM cost calculation and model pricing
//! - `rules` - framework knowledge as data, interpreted generically
//! - `sideml` - Universal AI message format normalization
//! - `traces` - OpenTelemetry trace processing pipeline

pub mod metrics;
pub mod pricing;
pub mod providers;
pub mod rules;
pub mod sideml;
pub mod traces;

pub use metrics::Stored as MetricsStored;
pub use metrics::ingest as ingest_metrics;
pub use traces::{MessageSource, RawMessage, SseSpanEvent, TracePipeline};

use crate::data::topics::TopicMessage;
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};

impl TopicMessage for ExportTraceServiceRequest {
    /// The **trace id of the first span**, and nothing else.
    ///
    /// Trace id rather than session id, because a session id is not a property of every span: it lives on the
    /// span that knows it, usually the root. A rule of "session when the batch carries one, else trace" therefore
    /// keys a child-only batch on the trace and a later batch carrying the root on the session - the same trace in
    /// two partitions, mid conversation, which is precisely the split a key exists to prevent. A trace id is
    /// total, immutable, and readable from the span alone.
    ///
    /// **One export can carry many traces, and this returns one key**, which is the stated limit of keying at this
    /// level: the plan splits a request into keyed sub-batches before publishing, and until that lands a
    /// multi-trace export is keyed by its first trace. On the in-process backend that costs nothing - one
    /// partition - and it is why this is safe to introduce ahead of the split.
    fn partition_key(&self) -> String {
        self.resource_spans
            .iter()
            .flat_map(|rs| &rs.scope_spans)
            .flat_map(|ss| &ss.spans)
            .next()
            .map(|span| hex::encode(&span.trace_id))
            .unwrap_or_default()
    }

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
    /// `(resource, instrument)` - the metric's own name, qualified by the scope that emitted it.
    ///
    /// Not the project: a busy project's every instrument would then share one partition, which is the
    /// serialisation bottleneck rather than the ordering guarantee. What has to stay in order is one instrument's
    /// datapoints, because that is what a replacing engine deduplicates by.
    fn partition_key(&self) -> String {
        self.resource_metrics
            .iter()
            .flat_map(|rm| &rm.scope_metrics)
            .flat_map(|sm| sm.metrics.iter().map(move |m| (sm.scope.as_ref(), m)))
            .next()
            .map(|(scope, metric)| {
                let scope_name = scope.map(|s| s.name.as_str()).unwrap_or("");
                format!("{scope_name}/{}", metric.name)
            })
            .unwrap_or_default()
    }

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
    /// The trace id when the record carries one, else the scope that emitted it.
    ///
    /// A log record need not belong to a trace at all, so unlike spans there is no total key available. Where
    /// there is a trace, keying on it puts a request's logs beside its spans' ordering; where there is not, the
    /// scope is the narrowest thing that still groups a producer's own stream.
    fn partition_key(&self) -> String {
        for rl in &self.resource_logs {
            for sl in &rl.scope_logs {
                for record in &sl.log_records {
                    if !record.trace_id.is_empty() {
                        return hex::encode(&record.trace_id);
                    }
                }
                if let Some(scope) = sl.scope.as_ref()
                    && !scope.name.is_empty()
                {
                    return scope.name.clone();
                }
            }
        }
        String::new()
    }

    fn size_bytes(&self) -> usize {
        self.resource_logs
            .iter()
            .flat_map(|rl| &rl.scope_logs)
            .map(|sl| sl.log_records.len() * 300)
            .sum::<usize>()
            .max(100)
    }
}

#[cfg(test)]
mod partition_key_tests {
    use super::*;
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;

    /// Every queued signal declares a key, and the three rules differ.
    ///
    /// The default is `""` - no ordering requirement - which is right for a broadcast-shaped message and wrong for
    /// a signal a partitioned broker has to serialise. A new signal that forgets to override it would silently get
    /// the default, and on the in-process backend nothing would ever notice: it has one partition, so the key is
    /// unobservable until the broker changes. This is what notices.
    #[test]
    fn every_queued_signal_declares_a_partition_key() {
        use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
        use opentelemetry_proto::tonic::metrics::v1::{Metric, ResourceMetrics, ScopeMetrics};
        use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

        let spans = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: vec![Span {
                        trace_id: vec![0xab; 16],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        assert_eq!(
            spans.partition_key(),
            "ab".repeat(16),
            "a span batch must key on its trace id, or a partitioned broker splits one conversation"
        );

        let metrics = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "requests".to_string(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        assert!(
            metrics.partition_key().ends_with("requests"),
            "a metric batch keys on its instrument, got {:?}",
            metrics.partition_key()
        );

        let logs = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                scope_logs: vec![ScopeLogs {
                    log_records: vec![LogRecord {
                        trace_id: vec![0xcd; 16],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        assert_eq!(
            logs.partition_key(),
            "cd".repeat(16),
            "a traced log record keys on its trace, so it orders beside that trace's spans"
        );

        // An empty export has no key, which is the honest answer rather than a fabricated one.
        assert!(
            ExportTraceServiceRequest::default()
                .partition_key()
                .is_empty()
        );
    }
}
