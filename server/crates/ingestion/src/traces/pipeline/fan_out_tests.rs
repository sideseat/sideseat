use super::*;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

/// A request of roughly `payload_bytes`, carried as one attribute value.
///
/// One large attribute rather than many spans, because the bound is about bytes and this keeps the
/// relationship between the requested size and `encoded_len` direct enough to reason about.
fn request_of(payload_bytes: usize) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: None,
            scope_spans: vec![ScopeSpans {
                scope: None,
                spans: vec![Span {
                    trace_id: vec![1; 16],
                    span_id: vec![2; 8],
                    attributes: vec![KeyValue {
                        key: "payload".to_string(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue("x".repeat(payload_bytes))),
                        }),
                    }],
                    ..Default::default()
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

fn wave_shape(waves: &[&[ExportTraceServiceRequest]]) -> Vec<usize> {
    waves.iter().map(|w| w.len()).collect()
}

/// Small requests keep full parallelism: the byte bound does not fire, so only the worker count shapes it.
///
/// This is the common case and the one a byte bound must not make slower - the fixtures this repository
/// benchmarks are kilobytes, and a bound that halved their concurrency to protect against a 15 MB export
/// would be paying everywhere for a rare shape.
#[test]
fn small_requests_keep_full_parallelism() {
    let requests: Vec<_> = (0..8).map(|_| request_of(1_024)).collect();
    assert_eq!(wave_shape(&byte_bounded_waves(&requests, 8)), vec![8]);
    assert_eq!(wave_shape(&byte_bounded_waves(&requests, 4)), vec![4, 4]);
}

/// Large requests are split into waves regardless of how many workers are available.
///
/// Eight 12 MB requests against a 64 MB budget cannot all be in flight, regardless of host CPU count.
#[test]
fn large_requests_are_bounded_by_bytes_not_by_cores() {
    let requests: Vec<_> = (0..8).map(|_| request_of(12 * 1024 * 1024)).collect();
    let waves = byte_bounded_waves(&requests, 32);
    assert!(
        waves.len() > 1,
        "eight 12 MB requests must not all be in flight at once on a 32-core host"
    );
    for wave in &waves {
        let bytes: u64 = wave.iter().map(|r| r.encoded_len() as u64).sum();
        assert!(
            bytes <= PIPELINE_CPU_PHASE_MAX_INFLIGHT_BYTES || wave.len() == 1,
            "a wave holds {bytes} bytes, over the budget, and is not a single oversized request"
        );
    }
}

/// A request larger than the whole budget is processed alone, not refused and not skipped.
///
/// Oversized input must not create an empty wave or prevent later requests from being partitioned.
#[test]
fn a_single_oversized_request_forms_its_own_wave() {
    let oversized = (PIPELINE_CPU_PHASE_MAX_INFLIGHT_BYTES as usize) * 2;
    let requests = vec![request_of(1_024), request_of(oversized), request_of(1_024)];
    let waves = byte_bounded_waves(&requests, 8);

    assert!(
        waves.iter().all(|w| !w.is_empty()),
        "no wave may be empty: {:?}",
        wave_shape(&waves)
    );
    assert_eq!(
        waves.iter().map(|w| w.len()).sum::<usize>(),
        requests.len(),
        "every request appears in exactly one wave"
    );
}

/// Every request appears once, in order, for any batch shape and worker count.
///
/// Results are concatenated wave by wave, so partition order is part of the batch contract.
#[test]
fn the_waves_partition_the_batch_in_order() {
    for count in [0usize, 1, 3, 8, 17] {
        for workers in [1usize, 2, 8] {
            // Distinct sizes, so a reordering is detectable by the sizes alone.
            let requests: Vec<_> = (0..count).map(|n| request_of(64 + n * 7)).collect();
            let waves = byte_bounded_waves(&requests, workers);

            let flattened: Vec<usize> = waves
                .iter()
                .flat_map(|w| w.iter())
                .map(|r| r.encoded_len())
                .collect();
            let expected: Vec<usize> = requests.iter().map(|r| r.encoded_len()).collect();
            assert_eq!(
                flattened, expected,
                "count {count}, workers {workers}: the waves must be the batch, in order"
            );
            assert!(
                waves.iter().all(|w| w.len() <= workers.max(1)),
                "count {count}, workers {workers}: a wave exceeded the worker count"
            );
        }
    }
}
