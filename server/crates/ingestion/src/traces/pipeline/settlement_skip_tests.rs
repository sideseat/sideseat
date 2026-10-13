//! The write path settles the exports it stored whole without reading them back, and settles a wave's other
//! payloads together; neither may settle anything the reads would not (`server/specs/StagingRetirement.tla`,
//! `Skips`).

use chrono::Utc;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_ports::types::{StagedPayload, StagedSignal};

use super::batch::BatchAnswer;
use super::pipeline_tests::pipeline_over_a_temp_store_with;
use super::*;
use crate::staging::StagingDisposition;

/// An export of the spans `spans` of trace `[trace; 16]`.
fn export(trace: u8, spans: std::ops::RangeInclusive<u8>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource::default()),
            scope_spans: vec![ScopeSpans {
                spans: spans
                    .map(|span| Span {
                        trace_id: vec![trace; 16],
                        span_id: vec![span; 8],
                        name: "step".into(),
                        start_time_unix_nano: 1_700_000_000_000_000_000 + u64::from(span),
                        end_time_unix_nano: 1_700_000_000_000_000_100 + u64::from(span),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn body(request: &ExportTraceServiceRequest) -> ReceivedPayload {
    ReceivedPayload::new(
        request.encode_to_vec(),
        sideseat_domain::raw_payload::RawContent::Protobuf,
        Utc::now(),
    )
}

async fn staged(pipeline: &TracePipeline, request: &ExportTraceServiceRequest) -> StagedPayload {
    let received = body(request);
    let reference = pipeline
        .staging
        .stage(
            "default",
            StagedSignal::Traces,
            &received.staged(),
            received.received_at,
            crate::traces::confirmation_records(request),
            crate::staging::StagedWrite {
                partition_key: "p".to_string(),
                raw_id: None,
            },
        )
        .await
        .expect("stage");
    pipeline
        .staging
        .registration(&reference.id)
        .await
        .expect("registration")
        .expect("staged")
}

async fn written(pipeline: &TracePipeline, request: &ExportTraceServiceRequest) -> BatchAnswer {
    pipeline
        .run_batch_vouched(std::slice::from_ref(request), &[body(request)])
        .await
        .remove(0)
}

/// Vouched for only when this execution wrote every span: a first delivery is; its exact redelivery, and an
/// export some of whose spans were redeliveries, are not - each of those settles by reading.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_an_export_written_whole_is_vouched_for() {
    let (_temp, _analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let first = export(41, 1..=3);
    assert_eq!(
        written(&pipeline, &first).await,
        BatchAnswer {
            outcome: IngestOutcome::Stored,
            written: true
        }
    );
    assert_eq!(
        written(&pipeline, &first).await,
        BatchAnswer {
            outcome: IngestOutcome::Stored,
            written: false
        },
        "an exact redelivery wrote nothing"
    );
    assert_eq!(
        written(&pipeline, &export(41, 1..=4)).await,
        BatchAnswer {
            outcome: IngestOutcome::Stored,
            written: false
        },
        "three of its four spans were redeliveries"
    );
}

/// What the skip trusts: a payload settled as written is retired without any read, so it may be settled so only
/// for an export the batch vouched for. Here nothing was written, and it is retired all the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_payload_settled_as_written_is_retired_without_a_read() {
    let (_temp, _analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let payload = staged(&pipeline, &export(42, 1..=2)).await;
    assert_eq!(
        pipeline.staging.disposition(&payload).await.expect("read"),
        StagingDisposition::Pending,
        "the premise: read back, it is not stored"
    );
    assert_eq!(
        pipeline
            .staging
            .settle_written(&payload)
            .await
            .expect("settle"),
        StagingDisposition::Confirmed
    );
    assert!(
        pipeline
            .staging
            .registration(&payload.id)
            .await
            .expect("registration")
            .is_none()
    );
}

/// A wave's payloads settled together answer exactly what each would alone: stored ones are confirmed and
/// retired, one never written stays pending and registered, whatever shares its reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn payloads_settled_together_answer_as_each_alone() {
    let (_temp, _analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let stored = [export(43, 1..=3), export(44, 1..=2)];
    let unwritten = export(45, 1..=2);
    let mut payloads = Vec::new();
    for request in stored.iter().chain(std::iter::once(&unwritten)) {
        payloads.push(staged(&pipeline, request).await);
    }
    for request in &stored {
        assert_eq!(
            written(&pipeline, request).await.outcome,
            IngestOutcome::Stored
        );
    }
    let mut alone = Vec::new();
    for payload in &payloads {
        alone.push(pipeline.staging.disposition(payload).await.expect("alone"));
    }
    assert_eq!(
        alone,
        vec![
            StagingDisposition::Confirmed,
            StagingDisposition::Confirmed,
            StagingDisposition::Pending
        ]
    );
    let together: Vec<StagingDisposition> = pipeline
        .staging
        .settle_traces(&payloads.iter().collect::<Vec<_>>())
        .await
        .into_iter()
        .map(|answer| answer.expect("together"))
        .collect();
    assert_eq!(together, alone);
    for (payload, disposition) in payloads.iter().zip(&alone) {
        let registered = pipeline
            .staging
            .registration(&payload.id)
            .await
            .expect("registration")
            .is_some();
        assert_eq!(registered, *disposition == StagingDisposition::Pending);
    }
}

/// An export whose resources name two projects is not vouched for: its record is claimed in one of them, so the
/// rows in the other are held by no record there, and settling must read to find that out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_reaching_two_projects_is_not_vouched_for() {
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    let (_temp, _analytics, database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let user = database
        .create_user("projects@example.com", None)
        .await
        .expect("user");
    let org = database
        .create_organization_with_owner("Projects", "projects", &user.id)
        .await
        .expect("org");
    let first = database
        .create_project(&org.id, "First")
        .await
        .expect("first")
        .id;
    let second = database
        .create_project(&org.id, "Second")
        .await
        .expect("second")
        .id;
    let in_project = |project: &str, trace: u8| ResourceSpans {
        resource: Some(Resource {
            attributes: vec![KeyValue {
                key: crate::otlp::PROJECT_ID_ATTR.into(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::StringValue(project.into())),
                }),
            }],
            ..Default::default()
        }),
        ..export(trace, 1..=2).resource_spans.remove(0)
    };
    let request = ExportTraceServiceRequest {
        resource_spans: vec![in_project(&first, 46), in_project(&second, 47)],
    };
    let answer = written(&pipeline, &request).await;
    assert!(!answer.written, "{answer:?}");
    let alone = ExportTraceServiceRequest {
        resource_spans: vec![in_project(&first, 48)],
    };
    assert!(
        written(&pipeline, &alone).await.written,
        "the premise: an export of one named project is vouched for"
    );
}
