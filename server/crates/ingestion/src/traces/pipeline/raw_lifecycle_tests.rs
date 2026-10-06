//! The raw lifecycle end to end over DuckDB and SQLite: ingest, delete, reconcile, hold, repair.

use std::borrow::Cow;

use base64::Engine;
use chrono::{TimeZone, Utc};
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use sideseat_domain::raw_payload::{self, RawContent};
use sideseat_ports::types::{RawOrigin, RawRecordRow};

use super::pipeline_tests::pipeline_over_a_temp_store_with;
use super::raw::{SpanKey, record_identities};
use super::*;

const PROJECT: &str = "default";

fn picture(seed: u8) -> String {
    base64::engine::general_purpose::STANDARD.encode(
        (0..1500)
            .map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed))
            .collect::<Vec<_>>(),
    )
}

fn span(trace: u8, id: u8, text: Option<String>) -> Span {
    Span {
        trace_id: vec![trace; 16],
        span_id: vec![id; 8],
        name: format!("span-{trace}-{id}"),
        start_time_unix_nano: 1_700_000_000_000_000_000 + u64::from(id) * 1_000_000,
        end_time_unix_nano: 1_700_000_001_000_000_000,
        attributes: text
            .map(|text| {
                vec![KeyValue {
                    key: "input.value".into(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(text)),
                    }),
                }]
            })
            .unwrap_or_default(),
        ..Default::default()
    }
}

fn export(spans: Vec<Span>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource::default()),
            scope_spans: vec![ScopeSpans {
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn trace(id: u8) -> String {
    hex::encode([id; 16])
}

fn key(trace_id: u8, id: u8) -> SpanKey {
    (trace(trace_id), hex::encode([id; 8]))
}

/// Ingest a body exactly as the request path does: staged, prepared, written.
async fn ingest(pipeline: &TracePipeline, body: &ExportTraceServiceRequest) -> ReceivedPayload {
    let received = ReceivedPayload::new(body.encode_to_vec(), RawContent::Protobuf);
    let (request, received) =
        crate::received::staged_traces(&received.staged(), PROJECT).expect("prepare");
    let outcome = pipeline.ingest_now(&request, &received).await;
    assert!(
        matches!(
            outcome,
            IngestOutcome::Stored | IngestOutcome::PartlyDropped { .. }
        ),
        "{outcome:?}"
    );
    received
}

async fn only_record(analytics: &Arc<dyn AnalyticsRepository + Send + Sync>) -> RawRecordRow {
    let page = analytics
        .raw_records_page(&ProjectId::from(PROJECT), None, 10)
        .await
        .unwrap();
    assert_eq!(page.len(), 1, "one record");
    page.into_iter().next().unwrap()
}

async fn decode_from_files(pipeline: &TracePipeline, record: &[u8]) -> Vec<u8> {
    let mut objects = HashMap::new();
    for hash in raw_payload::media_hashes(record).unwrap() {
        let file = pipeline
            .file_service
            .get_file(&ProjectId::from(PROJECT), &hex::encode(hash))
            .await
            .expect("the medium is stored");
        objects.insert(hash, file.data.to_vec());
    }
    raw_payload::decode(record, |hash| {
        objects
            .get(hash)
            .map(|bytes| Cow::Borrowed(bytes.as_slice()))
    })
    .unwrap()
    .1
}

async fn drain(pipeline: &TracePipeline) {
    for _ in 0..8 {
        pipeline.reconcile_raw_records(64).await.unwrap();
    }
    let left = pipeline.analytics.pending_raw_records(64).await.unwrap();
    assert!(left.is_empty(), "the queue settles: {left:?}");
}

/// A deleted trace leaves the raw record - its spans and its media - while the export's other trace stays,
/// byte for byte as the received export filtered to it; the record is flagged as rewritten after a deletion.
/// The deleted trace's files are cleaned up before the reconciler runs, as the API does, and the rewrite still
/// succeeds: what it needs is only the surviving spans' media.
#[tokio::test]
async fn a_deleted_trace_leaves_the_raw_record() {
    let (_temp, analytics, database, pipeline) = pipeline_over_a_temp_store_with(true).await;
    let body = export(vec![
        span(1, 1, Some(picture(1))),
        span(1, 2, None),
        span(2, 3, Some(picture(2))),
    ]);
    let received = ingest(&pipeline, &body).await;
    let first = only_record(&analytics).await;
    assert_eq!(first.origin, RawOrigin::Received);
    assert_eq!(
        decode_from_files(&pipeline, &first.record).await,
        received.bytes
    );
    assert_eq!(raw_payload::media_hashes(&first.record).unwrap().len(), 2);

    let project = ProjectId::from(PROJECT);
    database
        .record_deleted_traces_journalled(&project, &[trace(1)])
        .await
        .unwrap();
    analytics
        .delete_traces(&project, &[trace(1)])
        .await
        .unwrap();
    pipeline
        .file_service
        .cleanup_traces(&project, &[trace(1)])
        .await
        .unwrap();
    drain(&pipeline).await;

    let rewritten = only_record(&analytics).await;
    assert_eq!(rewritten.origin, RawOrigin::Deleted);
    assert_eq!(rewritten.raw_id, first.raw_id);
    assert!(rewritten.version > first.version);
    assert_eq!(
        record_identities(&rewritten.record).unwrap(),
        HashSet::from([key(2, 3)])
    );
    let picture_two = hex::encode(blake3::hash(picture(2).as_bytes()).as_bytes());
    assert_eq!(
        raw_payload::media_hashes(&rewritten.record)
            .unwrap()
            .iter()
            .map(hex::encode)
            .collect::<Vec<_>>(),
        vec![picture_two]
    );
    let kept = ExportTraceServiceRequest::decode(
        decode_from_files(&pipeline, &rewritten.record)
            .await
            .as_slice(),
    )
    .unwrap();
    assert_eq!(kept, export(vec![span(2, 3, Some(picture(2)))]));
}

/// A record no row names is deleted with its trace index once its last trace goes.
#[tokio::test]
async fn a_record_no_row_names_is_collected() {
    let (_temp, analytics, database, pipeline) = pipeline_over_a_temp_store_with(true).await;
    ingest(&pipeline, &export(vec![span(1, 1, Some(picture(3)))])).await;
    let project = ProjectId::from(PROJECT);
    database
        .record_deleted_traces_journalled(&project, &[trace(1)])
        .await
        .unwrap();
    analytics
        .delete_traces(&project, &[trace(1)])
        .await
        .unwrap();
    drain(&pipeline).await;
    assert!(
        analytics
            .raw_records_page(&project, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Under a legal hold the reconciler neither deletes nor rewrites, and the entries wait for the hold to end.
#[tokio::test]
async fn a_held_record_is_left_alone() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(true).await;
    ingest(&pipeline, &export(vec![span(1, 1, None), span(2, 2, None)])).await;
    let project = ProjectId::from(PROJECT);
    let before = only_record(&analytics).await;
    analytics
        .patch_project_hold(&project, Utc.timestamp_opt(4_000_000_000, 0).unwrap())
        .await
        .unwrap();
    analytics
        .delete_traces(&project, &[trace(1)])
        .await
        .unwrap();
    analytics
        .delete_traces(&project, &[trace(2)])
        .await
        .unwrap();
    pipeline.reconcile_raw_records(64).await.unwrap();

    let after = only_record(&analytics).await;
    assert_eq!(
        after.record, before.record,
        "nothing rewritten under a hold"
    );
    assert!(!analytics.pending_raw_records(64).await.unwrap().is_empty());
}

/// An ingest whose rows the latest record does not hold appends the union, two versions up, and enqueues it.
#[tokio::test]
async fn an_ingest_repairs_a_record_that_lost_its_spans() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let body = export(vec![span(1, 1, None), span(1, 2, None)]);
    let received = ingest(&pipeline, &body).await;
    let first = only_record(&analytics).await;
    let project = ProjectId::from(PROJECT);

    // A version without span 2, as a rewrite would leave it, and span 2's row gone without a fence - so the
    // redelivery writes span 2 again and the latest record does not hold it.
    let lacking = RawRecordRow {
        origin: RawOrigin::Deleted,
        version: first.version + 10,
        record: raw_payload::wrap(
            &export(vec![span(1, 1, None)]).encode_to_vec(),
            RawContent::Protobuf,
        ),
        ..first.clone()
    };
    analytics
        .append_raw_records(std::slice::from_ref(&lacking))
        .await
        .unwrap();
    analytics
        .delete_spans(&project, &[key(1, 2)])
        .await
        .unwrap();
    for entry in analytics.pending_raw_records(64).await.unwrap() {
        analytics.clear_raw_pending(&[entry]).await.unwrap();
    }

    ingest(&pipeline, &body).await;
    let repaired = only_record(&analytics).await;
    assert!(repaired.version >= lacking.version + 2);
    assert_eq!(
        repaired.origin,
        RawOrigin::Received,
        "the union is the whole export"
    );
    assert_eq!(
        raw_payload::decode(&repaired.record, |_| None).unwrap().1,
        received.bytes
    );
    assert!(
        !analytics.pending_raw_records(64).await.unwrap().is_empty(),
        "a repair is reconciled like any other change"
    );
    drain(&pipeline).await;
}
