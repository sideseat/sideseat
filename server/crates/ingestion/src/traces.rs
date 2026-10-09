pub mod enrich;
pub mod extract;
mod persist;
mod pipeline;
pub mod raw_views;

pub use extract::SpanData;
pub use persist::SseSpanEvent;
pub(crate) use pipeline::raw_id;
pub use pipeline::{
    DropReason, IngestOutcome, InlineBatcher, Reconciled, TracePipeline, strip_unstorable_spans,
};
#[cfg(any(test, feature = "test-support"))]
pub use pipeline::{process_request_for_test_with_files, process_request_for_test_with_mode};
pub use sideseat_domain::observations::{MessageSource, RawMessage};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use sideseat_core::utils::time::nanos_to_datetime;
use sideseat_ports::types::StagedRecord;

/// Producer-owned span identities and content digests used by durable staging.
pub fn confirmation_records(request: &ExportTraceServiceRequest) -> Vec<StagedRecord> {
    request
        .resource_spans
        .iter()
        .flat_map(|resource| {
            resource.scope_spans.iter().flat_map(move |scope| {
                scope.spans.iter().map(move |span| StagedRecord::Span {
                    trace_id: hex::encode(&span.trace_id),
                    span_id: hex::encode(&span.span_id),
                    content_digest: persist::span_content_digest(resource, scope, span),
                    timestamp: nanos_to_datetime(span.start_time_unix_nano),
                })
            })
        })
        .collect()
}
