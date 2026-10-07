//! A batch answers each export for its own spans, and keeps each export's raw record whole.
//!
//! The batch path used to answer a whole batch with one boolean and to key every span's export by its
//! identity. The boolean told a live project's exporter and a deleted project's exporter the same thing; the
//! identity key, when two exports carried the same span, gave that span to the later export alone - so the
//! earlier export's raw record was written without a span its fences kept, and the raw authority lost it.

use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_ports::types::ProjectId;

use super::pipeline_tests::pipeline_over_a_temp_store_with;
use super::*;

fn span(trace: u8, span: u8, text: &str) -> Span {
    Span {
        trace_id: vec![trace; 16],
        span_id: vec![span; 8],
        name: "generation".into(),
        start_time_unix_nano: 1_700_000_000_000_000_000 + u64::from(span),
        end_time_unix_nano: 1_700_000_000_000_000_100 + u64::from(span),
        attributes: vec![KeyValue {
            key: "input.value".into(),
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue(text.into())),
            }),
        }],
        ..Default::default()
    }
}

fn export(project: &str, spans: Vec<Span>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: crate::otlp::PROJECT_ID_ATTR.into(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(project.into())),
                    }),
                }],
                ..Default::default()
            }),
            scope_spans: vec![ScopeSpans {
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// An export to a project with no row is dropped as gone; the live export beside it is stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_export_in_a_batch_is_answered_for_its_own_spans() {
    let (_temp, _analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let live = export("default", vec![span(1, 1, "kept"), span(1, 2, "kept too")]);
    let gone = export("no-such-project", vec![span(2, 1, "refused")]);

    let outcomes = pipeline
        .run_batch_outcomes_for_test(&[live.clone(), gone.clone()])
        .await;

    assert_eq!(
        outcomes,
        vec![
            IngestOutcome::Stored,
            IngestOutcome::Dropped {
                spans: 1,
                reason: DropReason::Gone,
            },
        ]
    );
    // And as each would have been answered alone.
    let received = ReceivedPayload::new(
        gone.encode_to_vec(),
        sideseat_domain::raw_payload::RawContent::Protobuf,
    );
    assert_eq!(pipeline.ingest_now(&gone, &received).await, outcomes[1]);
}

/// Two exports carrying the same span each keep it in their own raw record, and each row names its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_exports_of_one_span_each_keep_it_in_their_record() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let first = export(
        "default",
        vec![span(3, 1, "first"), span(3, 2, "only in the first")],
    );
    let second = export("default", vec![span(3, 1, "second")]);

    let outcomes = pipeline
        .run_batch_outcomes_for_test(&[first.clone(), second.clone()])
        .await;
    assert!(
        outcomes
            .iter()
            .all(|outcome| *outcome == IngestOutcome::Stored)
    );

    let records = analytics
        .raw_records_page(&ProjectId::from("default"), None, 16)
        .await
        .expect("records");
    assert_eq!(records.len(), 2, "one record per export");
    for record in &records {
        let (_, bytes) =
            sideseat_domain::raw_payload::decode_shape(&record.record).expect("decode");
        let request = ExportTraceServiceRequest::decode(bytes.as_slice()).expect("protobuf");
        let ids: Vec<Vec<u8>> = request
            .resource_spans
            .iter()
            .flat_map(|rs| &rs.scope_spans)
            .flat_map(|ss| &ss.spans)
            .map(|span| span.span_id.clone())
            .collect();
        assert!(
            ids.contains(&vec![1; 8]),
            "each export's record keeps the span both carried: {ids:?}"
        );
    }
}
