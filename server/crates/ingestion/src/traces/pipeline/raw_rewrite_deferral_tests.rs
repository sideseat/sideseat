//! A rewritten record's pass that skips an association for a pending writer keeps that trace's cleanup owed
//! (`server/specs/SurvivorRelease.tla`): the writer can still abort, which leaves the durable association
//! referenced by nothing, and the pass has cleared its entries.

use super::raw_lifecycle_tests::{PROJECT, drain, export, ingest, key, picture, span, trace};
use super::*;

/// The deletion records its own retention cleanup; completed here, so what remains owed is the pass's.
async fn complete_owed_cleanups(store: &super::pipeline_tests::TempPipeline) {
    let project = ProjectId::from(PROJECT);
    let claimed: Vec<(String, i64)> = store
        .database
        .claim_retention_cleanup(16, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|(_, trace, token)| (trace, token))
        .collect();
    store
        .database
        .complete_retention_cleanup(&project, &claimed)
        .await
        .unwrap();
}

/// A redelivery associates the picture a rewrite has just stopped referencing, and is pending through the pass;
/// its span is then dropped. The pass recorded the trace's retention cleanup rather than clearing it as done.
#[tokio::test]
async fn a_pass_that_skips_for_a_pending_writer_leaves_the_cleanup_owed() {
    let store = super::pipeline_tests::temp_pipeline(true).await;
    ingest(
        &store.pipeline,
        &export(vec![span(1, 1, Some(picture(1))), span(1, 2, None)]),
    )
    .await;
    let project = ProjectId::from(PROJECT);
    store
        .database
        .record_deleted_spans_journalled(&project, &[key(1, 1)])
        .await
        .unwrap();
    store
        .analytics
        .delete_spans(&project, &[key(1, 1)])
        .await
        .unwrap();
    complete_owed_cleanups(&store).await;
    store.pipeline.reconcile_raw_records(64).await.unwrap();

    let hash = hex::encode(blake3::hash(picture(1).as_bytes()).as_bytes());
    store
        .database
        .associate_file(&trace(1), &project, &hash, Some("image/png"), 16, "blake3")
        .await
        .unwrap();
    drain(&store.pipeline).await;
    store
        .database
        .release_trace_file_association(&project, &trace(1), &hash)
        .await
        .unwrap();

    assert_eq!(
        store
            .database
            .get_file_hashes_for_traces(&project, &[trace(1)])
            .await
            .unwrap(),
        vec![hash],
        "the abort keeps the durable association, which nothing references now"
    );
    assert_eq!(
        sideseat_adapter_sqlite::test_support::owed_retention_cleanups(&store.sqlite)
            .await
            .unwrap(),
        vec![(PROJECT.to_string(), trace(1))],
        "the skipped association's cleanup is owed"
    );
}
