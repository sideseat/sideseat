use base64::Engine;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_domain::raw_payload::{self, RawContent};

use super::*;

fn picture(seed: u8, len: usize) -> String {
    base64::engine::general_purpose::STANDARD.encode(
        (0..len)
            .map(|i| (i as u8).wrapping_mul(13).wrapping_add(seed))
            .collect::<Vec<_>>(),
    )
}

/// `resources` resources of `scopes` scopes of `spans` spans, every span carrying a medium of `media` bytes.
fn export(resources: u8, scopes: u8, spans: u8, media: usize) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: (0..resources)
            .map(|r| ResourceSpans {
                scope_spans: (0..scopes)
                    .map(|s| ScopeSpans {
                        spans: (0..spans)
                            .map(|n| Span {
                                trace_id: vec![r.wrapping_mul(7).wrapping_add(s); 16],
                                span_id: vec![r, s, n, 1, 2, 3, 4, 5],
                                name: format!("span {r}.{s}.{n}"),
                                attributes: vec![KeyValue {
                                    key: "input.value".into(),
                                    value: Some(AnyValue {
                                        value: Some(any_value::Value::StringValue(format!(
                                            "data:image/png;base64,{}",
                                            picture(n, media)
                                        ))),
                                    }),
                                }],
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })
            .collect(),
    }
}

/// The reading this replaces, kept as the oracle: the whole shape rebuilt and the whole export decoded.
fn rebuilt(record: &[u8]) -> Result<HashSet<SpanIdentity>, String> {
    let (content, shape) = raw_payload::decode_shape(record).map_err(|error| error.to_string())?;
    let request: ExportTraceServiceRequest = match content {
        RawContent::Protobuf => ExportTraceServiceRequest::decode(shape.as_slice())
            .map_err(|error| error.to_string())?,
        RawContent::Json => serde_json::from_slice(&shape).map_err(|error| error.to_string())?,
    };
    Ok(request
        .resource_spans
        .iter()
        .flat_map(|resource| &resource.scope_spans)
        .flat_map(|scope| &scope.spans)
        .map(|span| (hex::encode(&span.trace_id), hex::encode(&span.span_id)))
        .collect())
}

fn records(request: &ExportTraceServiceRequest) -> [Vec<u8>; 2] {
    [
        raw_payload::encode(&request.encode_to_vec(), RawContent::Protobuf).record,
        raw_payload::encode(
            &serde_json::to_vec(request).expect("json"),
            RawContent::Json,
        )
        .record,
    ]
}

#[test]
fn the_identities_are_the_ones_the_whole_export_holds() {
    for request in [
        export(1, 1, 1, 0),
        export(1, 1, 3, 2000),
        export(3, 2, 4, 700),
        export(2, 1, 1, 30_000),
        ExportTraceServiceRequest::default(),
    ] {
        for record in records(&request) {
            let held = record_identities(&record).expect("readable");
            assert_eq!(held, rebuilt(&record).expect("readable"));
            assert_eq!(
                held.len(),
                request
                    .resource_spans
                    .iter()
                    .flat_map(|r| &r.scope_spans)
                    .map(|s| s.spans.len())
                    .sum::<usize>()
            );
        }
    }
}

#[test]
fn json_identities_are_normalised_as_the_export_decoding_reads_them() {
    let body = br#"{"resourceSpans":[{"scopeSpans":[{"spans":[
        {"traceId":"ABCDEF0123456789abcdef0123456789","spanId":"0011223344556677","name":"a"},
        {"spanId":"8899aabbccddeeff"}]}]}]}"#;
    let record = raw_payload::wrap(body, RawContent::Json);
    assert_eq!(record_identities(&record), rebuilt(&record));
    assert!(record_identities(&record).unwrap().contains(&(
        "abcdef0123456789abcdef0123456789".into(),
        "0011223344556677".into()
    )));
    // A prefix the export's decoding refuses is refused here too.
    let prefixed = raw_payload::wrap(
        br#"{"resourceSpans":[{"scopeSpans":[{"spans":[{"traceId":"0x00"}]}]}]}"#,
        RawContent::Json,
    );
    assert!(record_identities(&prefixed).is_err());
    assert!(rebuilt(&prefixed).is_err());
}

#[test]
fn what_is_not_a_record_holds_nothing_readable() {
    for record in [
        b"not a raw record".to_vec(),
        raw_payload::wrap(b"\xff\xff\xff", RawContent::Protobuf),
        raw_payload::wrap(b"{\"resourceSpans\": [", RawContent::Json),
    ] {
        assert!(record_identities(&record).is_err());
        assert!(rebuilt(&record).is_err());
    }
}

/// `cargo test -p sideseat-ingestion --lib raw_identities -- --ignored --nocapture`: the two readings' cost on
/// a record of 64 spans, each with a 64 KB medium.
#[test]
#[ignore = "a measurement, not a check"]
fn measure_the_two_readings() {
    let request = export(1, 1, 64, 64 * 1024);
    for (name, record) in ["protobuf", "json"].into_iter().zip(records(&request)) {
        let rounds = 50;
        let started = std::time::Instant::now();
        for _ in 0..rounds {
            std::hint::black_box(rebuilt(&record).unwrap());
        }
        let before = started.elapsed() / rounds;
        let started = std::time::Instant::now();
        for _ in 0..rounds {
            std::hint::black_box(record_identities(&record).unwrap());
        }
        let after = started.elapsed() / rounds;
        println!(
            "{name}: record {} B, shape {} B; rebuilt {before:?}, segments {after:?}",
            record.len(),
            raw_payload::decode_shape(&record).unwrap().1.len()
        );
    }
}
