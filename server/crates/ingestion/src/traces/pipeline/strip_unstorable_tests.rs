use super::*;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

fn request(nanos: &[u64]) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: None,
            scope_spans: vec![ScopeSpans {
                scope: None,
                spans: nanos
                    .iter()
                    .map(|n| Span {
                        start_time_unix_nano: *n,
                        ..Default::default()
                    })
                    .collect(),
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

/// An unstorable span is removed before the request is queued, and counted so the route can report it.
///
/// A durable queue answers 200 as soon as the payload is published; the consumer discards the unstorable
/// span later, and nothing can report back to a request that has already returned. So the span was
/// acknowledged and then simply absent. Settling it here is possible because storability depends on the
/// payload alone.
#[test]
fn an_unstorable_span_is_stripped_before_queueing() {
    // 2024, and a year-2300 timestamp (past the DateTime64(6) ceiling).
    let year_2024 = 1_704_067_200u64 * 1_000_000_000;
    let year_2300 = 10_413_792_000u64 * 1_000_000_000;
    let mut req = request(&[year_2024, year_2300]);
    assert_eq!(strip_unstorable_spans(&mut req), 1);
    let left: usize = req
        .resource_spans
        .iter()
        .flat_map(|r| r.scope_spans.iter())
        .map(|s| s.spans.len())
        .sum();
    assert_eq!(left, 1, "the storable span must be kept");
}

/// An ordinary request is untouched, so the check costs nothing in the common case.
#[test]
fn a_storable_request_is_untouched() {
    let year_2024 = 1_704_067_200u64 * 1_000_000_000;
    let mut req = request(&[year_2024, year_2024]);
    assert_eq!(strip_unstorable_spans(&mut req), 0);
}

/// An unstorable *end* time is caught too - it is stored in its own column, under the same TTL.
#[test]
fn an_unstorable_end_time_is_stripped() {
    let year_2024 = 1_704_067_200u64 * 1_000_000_000;
    let year_2300 = 10_413_792_000u64 * 1_000_000_000;
    let mut req = request(&[year_2024]);
    req.resource_spans[0].scope_spans[0].spans[0].end_time_unix_nano = year_2300;
    assert_eq!(strip_unstorable_spans(&mut req), 1);
}
