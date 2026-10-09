//! An export's revision of a span is ordered by when it was received, not by when it was written.
//!
//! Nothing orders two workers' writes: a consumer, the inline requester and redrive each load a payload, write it
//! and settle it, and one holding an earlier export can write it after another wrote a later one. Stored at the
//! write's own instant, that late copy took the span back from the revision received after it - and since the
//! later revision was then not the winner, settling it failed and it was written again, over the earlier one, for
//! as long as either was retried. Stored at its receipt, the late copy is superseded at once and settles as
//! superseded (`server/specs/StagingRetirement.tla`, `LaterReceiptWins`).

use chrono::{DateTime, TimeDelta, Utc};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_ports::types::{ListSpansParams, ProjectId, StagedPayload, StagedSignal};

use super::pipeline_tests::pipeline_over_a_temp_store_with;
use super::*;
use crate::staging::StagingDisposition;

/// One export of three spans of one trace, each named `name`.
fn export(name: &str) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource::default()),
            scope_spans: vec![ScopeSpans {
                spans: (1..=3u8)
                    .map(|span| Span {
                        trace_id: vec![31; 16],
                        span_id: vec![span; 8],
                        name: name.into(),
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

/// Stage `request` as received at `received_at`, returning its body and its registration.
async fn staged(
    pipeline: &TracePipeline,
    request: &ExportTraceServiceRequest,
    received_at: DateTime<Utc>,
) -> (ReceivedPayload, StagedPayload) {
    let received = ReceivedPayload::new(
        request.encode_to_vec(),
        sideseat_domain::raw_payload::RawContent::Protobuf,
        received_at,
    );
    let reference = pipeline
        .staging
        .stage(
            "default",
            StagedSignal::Traces,
            &received.staged(),
            received.received_at,
            crate::traces::confirmation_records(request),
            "p".to_string(),
        )
        .await
        .expect("stage");
    let registration = pipeline
        .staging
        .registration(&reference.id)
        .await
        .expect("registration")
        .expect("staged");
    (received, registration)
}

async fn span_names(analytics: &(dyn AnalyticsRepository + Send + Sync)) -> Vec<String> {
    let (rows, _) = analytics
        .list_spans(&ListSpansParams {
            project_id: ProjectId::from("default"),
            page: 1,
            limit: 10,
            ..Default::default()
        })
        .await
        .expect("spans");
    rows.into_iter()
        .map(|row| row.span_name.unwrap_or_default())
        .collect()
}

/// The earlier export is written after the later one, as a worker holding its copy writes it late: the later
/// revision stays the winner, and the late copy settles as superseded instead of staying pending to be written
/// again. Before it is written it is not settled: superseded settles only an export whose own record holds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_written_after_a_later_revision_settles_as_superseded() {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let first = export("first");
    let second = export("second");
    let received_at = Utc::now();
    let (first_body, first_staged) = staged(&pipeline, &first, received_at).await;
    let (second_body, second_staged) =
        staged(&pipeline, &second, received_at + TimeDelta::seconds(1)).await;

    let answers = pipeline
        .run_waves(
            std::slice::from_ref(&second),
            std::slice::from_ref(&second_body),
            &[Some(second_staged)],
        )
        .await;
    assert!(
        answers[0].outcome == IngestOutcome::Stored && answers[0].settled,
        "{answers:?}"
    );
    assert_eq!(
        pipeline
            .staging
            .disposition(&first_staged)
            .await
            .expect("disposition"),
        StagingDisposition::Pending,
        "an export never written is not stored, whatever superseded it"
    );

    let answers = pipeline
        .run_waves(
            std::slice::from_ref(&first),
            std::slice::from_ref(&first_body),
            &[Some(first_staged)],
        )
        .await;
    assert!(
        answers[0].outcome == IngestOutcome::Stored && answers[0].settled,
        "the late copy settles as superseded: {answers:?}"
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
    assert_eq!(
        span_names(analytics.as_ref()).await,
        vec!["second"; 3],
        "the revision received later is the winner"
    );
}
