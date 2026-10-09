//! An export is stored only when the raw authority holds it, not merely when matching rows are in place.
//!
//! The rows are a cache of the raw records. Settling an export by its rows alone acknowledged exports whose
//! record a failed repair left without them, and skipping a redelivery by its rows alone meant such an export
//! could never be repaired: every retry was an exact redelivery and was dropped before the record was touched.

use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_ports::types::{ProjectId, RawRecordRow, StagedSignal};

use super::pipeline_tests::pipeline_over_a_temp_store_with;
use super::*;
use crate::staging::StagingDisposition;

fn export() -> ExportTraceServiceRequest {
    export_of(21)
}

fn export_of(trace: u8) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource::default()),
            scope_spans: vec![ScopeSpans {
                spans: (1..=3u8)
                    .map(|span| Span {
                        trace_id: vec![trace; 16],
                        span_id: vec![span; 8],
                        name: "step".into(),
                        start_time_unix_nano: 1_700_000_000_000_000_000 + u64::from(span),
                        end_time_unix_nano: 1_700_000_000_000_000_100 + u64::from(span),
                        attributes: vec![KeyValue {
                            key: "input.value".into(),
                            value: Some(AnyValue {
                                value: Some(any_value::Value::StringValue(format!("step {span}"))),
                            }),
                        }],
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn protobuf(request: &ExportTraceServiceRequest) -> ReceivedPayload {
    ReceivedPayload::new(
        request.encode_to_vec(),
        sideseat_domain::raw_payload::RawContent::Protobuf,
        chrono::Utc::now(),
    )
}

fn json(request: &ExportTraceServiceRequest) -> ReceivedPayload {
    ReceivedPayload::new(
        serde_json::to_vec(request).expect("json"),
        sideseat_domain::raw_payload::RawContent::Json,
        chrono::Utc::now(),
    )
}

fn identities(request: &ExportTraceServiceRequest) -> Vec<(String, String)> {
    request
        .resource_spans
        .iter()
        .flat_map(|rs| &rs.scope_spans)
        .flat_map(|ss| &ss.spans)
        .map(|span| (hex::encode(&span.trace_id), hex::encode(&span.span_id)))
        .collect()
}

async fn covered(
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    request: &ExportTraceServiceRequest,
) -> usize {
    crate::raw_coverage::covered(analytics, &ProjectId::from("default"), &identities(request))
        .await
        .expect("coverage")
        .len()
}

async fn delete_records(analytics: &(dyn AnalyticsRepository + Send + Sync)) {
    let project = ProjectId::from("default");
    let records = analytics
        .raw_records_page(&project, None, 64)
        .await
        .expect("records");
    let ids: Vec<String> = records.into_iter().map(|record| record.raw_id).collect();
    analytics
        .delete_raw_records(&project, &ids)
        .await
        .expect("delete records");
}

/// The same export again, encoded as JSON: its rows already name a record that holds it, so it is stored, and
/// no second copy of the same telemetry is kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reencoded_exact_redelivery_is_covered_by_the_record_it_matches() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let request = export();
    assert_eq!(
        pipeline.ingest_now(&request, &protobuf(&request)).await,
        IngestOutcome::Stored
    );
    assert_eq!(
        pipeline.ingest_now(&request, &json(&request)).await,
        IngestOutcome::Stored
    );

    assert_eq!(covered(analytics.as_ref(), &request).await, 3);
    let records = analytics
        .raw_records_page(&ProjectId::from("default"), None, 64)
        .await
        .expect("records");
    assert_eq!(records.len(), 1, "an exact redelivery keeps no second copy");
}

/// The same re-encoded redelivery, batched with a fresh export, keeps no second copy either: an export left with
/// nothing to write stores no record, whether or not another export in its batch writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_batched_reencoded_exact_redelivery_keeps_no_second_copy() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let stored = export();
    assert_eq!(
        pipeline.ingest_now(&stored, &protobuf(&stored)).await,
        IngestOutcome::Stored
    );
    let fresh = export_of(22);
    assert_eq!(
        pipeline
            .run_batch(
                &[stored.clone(), fresh.clone()],
                &[json(&stored), protobuf(&fresh)]
            )
            .await,
        vec![IngestOutcome::Stored, IngestOutcome::Stored]
    );

    let records = analytics
        .raw_records_page(&ProjectId::from("default"), None, 64)
        .await
        .expect("records");
    assert_eq!(
        records.len(),
        2,
        "the first export's record and the fresh one's, and nothing for the redelivery"
    );
    assert_eq!(covered(analytics.as_ref(), &stored).await, 3);
    assert_eq!(covered(analytics.as_ref(), &fresh).await, 3);
}

/// A record that no longer decodes is superseded by the next delivery of the same body, instead of standing
/// for ever: every retry presents the same raw id, so its insert is skipped, and the repair used to trust a
/// received body without reading it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_record_is_superseded_by_a_redelivery() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let request = export();
    let received = protobuf(&request);
    pipeline.ingest_now(&request, &received).await;
    let project = ProjectId::from("default");
    let mut corrupt = analytics
        .raw_records_page(&project, None, 64)
        .await
        .expect("records")
        .remove(0);
    corrupt.version += 1;
    corrupt.record = b"not a raw record".to_vec();
    analytics
        .append_raw_records(std::slice::from_ref(&corrupt))
        .await
        .expect("corrupt the latest version");
    assert_eq!(covered(analytics.as_ref(), &request).await, 0);

    assert_eq!(
        pipeline.ingest_now(&request, &received).await,
        IngestOutcome::Stored
    );
    assert_eq!(
        covered(analytics.as_ref(), &request).await,
        3,
        "a readable version holds the export again"
    );
}

/// Rows whose record was lost are written again by the next delivery, which re-creates the record: the export
/// is not skipped as a redelivery while its authority is missing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rows_without_their_record_are_written_again() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let request = export();
    pipeline.ingest_now(&request, &protobuf(&request)).await;
    delete_records(analytics.as_ref()).await;
    assert_eq!(covered(analytics.as_ref(), &request).await, 0);

    assert_eq!(
        pipeline.ingest_now(&request, &protobuf(&request)).await,
        IngestOutcome::Stored
    );
    assert_eq!(
        covered(analytics.as_ref(), &request).await,
        3,
        "the record is back"
    );
}

/// A staged export whose rows are in place but whose record is not is pending, not confirmed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_settles_only_when_its_record_holds_it() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let request = export();
    let received = protobuf(&request);
    pipeline.ingest_now(&request, &received).await;
    let reference = pipeline
        .staging
        .stage(
            "default",
            StagedSignal::Traces,
            &received.staged(),
            received.received_at,
            crate::traces::confirmation_records(&request),
            "p".to_string(),
        )
        .await
        .expect("stage");
    let (payload, _) = pipeline
        .staging
        .load(&reference.id)
        .await
        .expect("load")
        .expect("staged");
    assert_eq!(
        pipeline
            .staging
            .disposition(&payload)
            .await
            .expect("disposition"),
        StagingDisposition::Confirmed
    );

    delete_records(analytics.as_ref()).await;
    assert_eq!(
        pipeline
            .staging
            .disposition(&payload)
            .await
            .expect("disposition"),
        StagingDisposition::Pending,
        "matching rows without their record are not a stored export"
    );
}

fn record_of(project: &str, raw_id: &str, request: &ExportTraceServiceRequest) -> RawRecordRow {
    RawRecordRow {
        project_id: ProjectId::from(project),
        raw_id: raw_id.to_string(),
        signal: StagedSignal::Traces,
        received_at: chrono::Utc::now(),
        origin: sideseat_ports::types::RawOrigin::Received,
        version: 1,
        signal_until: chrono::Utc::now(),
        hold_until: None,
        trace_ids: Vec::new(),
        record: sideseat_domain::raw_payload::wrap(
            &request.encode_to_vec(),
            sideseat_domain::raw_payload::RawContent::Protobuf,
        ),
    }
}

/// A failed write's records are queued for collection only for projects whose failure is settled: a project in
/// doubt may still have its rows land, and a record collected first would leave them naming nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_in_doubt_write_keeps_its_records_out_of_collection() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let request = export();
    let records = [
        record_of("settled", "settled-record", &request),
        record_of("doubt", "doubtful-record", &request),
        record_of("written", "written-record", &request),
    ];
    pipeline
        .enqueue_records_without_rows(
            &records,
            &HashSet::from(["written".to_string()]),
            &HashSet::from(["doubt".to_string()]),
        )
        .await;
    let queued: Vec<String> = analytics
        .pending_raw_records(16)
        .await
        .expect("pending")
        .into_iter()
        .map(|entry| entry.raw_id)
        .collect();
    assert_eq!(queued, vec!["settled-record".to_string()]);
}

/// A record the reconciler restores is indexed under the traces it holds again: reads return no trace ids, and
/// appended as read it was indexed under none, so a later deletion of one of its traces could not find it.
#[test]
fn a_restored_record_is_indexed_under_its_traces() {
    let mut request = export();
    request.resource_spans[0].scope_spans[0].spans[1].trace_id = vec![22; 16];
    let restored =
        super::raw_lifecycle::restored_version(&record_of("default", "restored", &request));
    assert_eq!(restored.version, 2);
    assert_eq!(
        restored.trace_ids,
        vec![hex::encode([21u8; 16]), hex::encode([22u8; 16])]
    );
}

/// Two revisions of the same spans in one batch, the earlier first: each is settled after its own wave, so the
/// earlier one is confirmed while it is the winner. Settled after both, it found the later revision the winner,
/// stayed pending, and was written again - over the revision that had arrived after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revisions_in_one_batch_are_each_settled_after_their_wave() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let first = export();
    let mut second = export();
    for span in &mut second.resource_spans[0].scope_spans[0].spans {
        span.name = "corrected".into();
    }
    let requests = [first, second];
    let received = [protobuf(&requests[0]), protobuf(&requests[1])];
    let mut staged = Vec::new();
    for (request, body) in requests.iter().zip(&received) {
        let reference = pipeline
            .staging
            .stage(
                "default",
                StagedSignal::Traces,
                &body.staged(),
                body.received_at,
                crate::traces::confirmation_records(request),
                "p".to_string(),
            )
            .await
            .expect("stage");
        staged.push(
            pipeline
                .staging
                .registration(&reference.id)
                .await
                .expect("registration"),
        );
    }

    let answers = pipeline.run_waves(&requests, &received, &staged).await;
    assert!(
        answers
            .iter()
            .all(|answer| answer.outcome == IngestOutcome::Stored && answer.settled),
        "{answers:?}"
    );
    assert!(
        pipeline
            .staging
            .pending(10)
            .await
            .expect("pending")
            .is_empty(),
        "an export stayed pending, to be written again"
    );
    let (rows, _) = analytics
        .list_spans(&sideseat_ports::types::ListSpansParams {
            project_id: ProjectId::from("default"),
            page: 1,
            limit: 10,
            ..Default::default()
        })
        .await
        .expect("spans");
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|row| row.span_name.as_deref() == Some("corrected")),
        "the later revision is the winner"
    );
}
