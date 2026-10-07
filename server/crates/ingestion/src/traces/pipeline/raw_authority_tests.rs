//! An export is stored only when the raw authority holds it, not merely when matching rows are in place.
//!
//! The rows are a cache of the raw records. Settling an export by its rows alone acknowledged exports whose
//! record a failed repair left without them, and skipping a redelivery by its rows alone meant such an export
//! could never be repaired: every retry was an exact redelivery and was dropped before the record was touched.

use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_ports::types::{ProjectId, StagedSignal};

use super::pipeline_tests::pipeline_over_a_temp_store_with;
use super::*;
use crate::staging::StagingDisposition;

fn export() -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource::default()),
            scope_spans: vec![ScopeSpans {
                spans: (1..=3u8)
                    .map(|span| Span {
                        trace_id: vec![21; 16],
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
    )
}

fn json(request: &ExportTraceServiceRequest) -> ReceivedPayload {
    ReceivedPayload::new(
        serde_json::to_vec(request).expect("json"),
        sideseat_domain::raw_payload::RawContent::Json,
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
