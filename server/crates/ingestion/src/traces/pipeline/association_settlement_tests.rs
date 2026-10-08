//! Associations whose rows a batch did not write itself are settled by the rows that are stored.
//!
//! A failed write can still land - a ClickHouse insert can store its rows and then report failure, and one whose
//! answer was lost can land after the caller gave up - and an exact redelivery's rows are already in place.
//! Releasing either batch's associations on its own account leaves readable rows naming files the sweeper then
//! reclaims.

use base64::Engine;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_domain::raw_payload::RawContent;
use sideseat_ports::traits::TransactionalRepository;

use super::pipeline_tests::pipeline_over_a_temp_store_with;
use super::*;

const PROJECT: &str = "default";

fn trace() -> String {
    hex::encode([41u8; 16])
}

fn export() -> ExportTraceServiceRequest {
    let picture = base64::engine::general_purpose::STANDARD
        .encode((0..4096u32).map(|i| (i % 251) as u8).collect::<Vec<_>>());
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource::default()),
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![41; 16],
                    span_id: vec![1; 8],
                    name: "generation".into(),
                    start_time_unix_nano: 1_700_000_000_000_000_000,
                    end_time_unix_nano: 1_700_000_000_000_000_100,
                    attributes: vec![KeyValue {
                        key: "input.value".into(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(format!(
                                "data:image/png;base64,{picture}"
                            ))),
                        }),
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

async fn orphans(database: &(dyn TransactionalRepository + Send + Sync)) -> HashSet<String> {
    database
        .get_orphan_files()
        .await
        .expect("orphans")
        .into_iter()
        .filter(|(project, _)| project == PROJECT)
        .map(|(_, hash)| hash)
        .collect()
}

/// An exact redelivery restores an association an earlier failure released under rows that had landed, where it
/// used to release its own re-created one and leave the file to the sweeper again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exact_redelivery_restores_a_lost_association() {
    let (_temp, _analytics, database, pipeline) = pipeline_over_a_temp_store_with(true).await;
    let project = ProjectId::from(PROJECT);
    let request = export();
    let received = ReceivedPayload::new(request.encode_to_vec(), RawContent::Protobuf);
    assert_eq!(
        pipeline.ingest_now(&request, &received).await,
        IngestOutcome::Stored
    );
    let hashes = database
        .get_file_hashes_for_traces(&project, &[trace()])
        .await
        .expect("hashes");
    assert!(
        !hashes.is_empty(),
        "the attachment is a file the trace holds"
    );

    // What a release on the strength of a failure that had in fact landed leaves: rows naming a file that
    // nothing holds.
    for hash in database
        .delete_trace_files(&project, &[trace()])
        .await
        .expect("drop the associations")
    {
        database
            .sync_ref_count(&project, &hash)
            .await
            .expect("recount");
    }
    let lost = orphans(database.as_ref()).await;
    assert!(hashes.iter().all(|hash| lost.contains(hash)), "{lost:?}");

    assert_eq!(
        pipeline.ingest_now(&request, &received).await,
        IngestOutcome::Stored,
        "an exact redelivery"
    );
    let left = orphans(database.as_ref()).await;
    assert!(
        hashes.iter().all(|hash| !left.contains(hash)),
        "the redelivery's rows are stored, so the file is held again: {left:?}"
    );

    // Held durably: a later batch that shares the association and fails releases only its own writer.
    for hash in &hashes {
        assert!(
            database
                .associate_existing_file(&trace(), &project, hash)
                .await
                .expect("share")
        );
        database
            .release_trace_file_association(&project, &trace(), hash)
            .await
            .expect("release");
        database
            .sync_ref_count(&project, hash)
            .await
            .expect("recount");
    }
    let left = orphans(database.as_ref()).await;
    assert!(hashes.iter().all(|hash| !left.contains(hash)), "{left:?}");
}

/// A write that may still land keeps the associations no stored row names yet; once settled, it releases them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_that_may_still_land_keeps_its_associations() {
    let (_temp, _analytics, database, pipeline) = pipeline_over_a_temp_store_with(true).await;
    let project = ProjectId::from(PROJECT);
    let hash = hex::encode(blake3::hash(b"an attachment").as_bytes());
    database
        .associate_file(&trace(), &project, &hash, Some("image/png"), 13, "blake3")
        .await
        .expect("associate");
    let associations = vec![(PROJECT.to_string(), trace(), hash.clone())];

    pipeline
        .settle_associations_by_stored_rows(&associations, &HashSet::from([PROJECT.to_string()]))
        .await;
    assert!(
        !orphans(database.as_ref()).await.contains(&hash),
        "no stored row names the file yet, but the write that would may still land"
    );

    pipeline
        .settle_associations_by_stored_rows(&associations, &HashSet::new())
        .await;
    assert!(
        orphans(database.as_ref()).await.contains(&hash),
        "a settled write that stored nothing naming the file releases it"
    );
}
