//! A raw rewrite's file cleanup, derived: the reconciler pass a rewrite's append queues reconciles the files of
//! the rewritten record's traces before it clears its entries (`server/specs/RawRewriteFiles.tla`).

use super::raw_lifecycle_tests::{
    PROJECT, drain, export, ingest, key, only_record, picture, span, trace,
};
use super::*;
use sideseat_ports::types::{RawOrigin, RawRecordRow};

fn picture_hash(seed: u8) -> String {
    hex::encode(blake3::hash(picture(seed).as_bytes()).as_bytes())
}

async fn files_of(store: &super::pipeline_tests::TempPipeline, trace_id: &str) -> Vec<String> {
    let mut files = store
        .database
        .get_file_hashes_for_traces(&ProjectId::from(PROJECT), &[trace_id.to_string()])
        .await
        .unwrap();
    files.sort();
    files
}

/// A trace that keeps one span after another, which carried a picture, is deleted: the record still names the
/// deleted span, so the picture's association stands until a rewrite removes it.
async fn a_span_deleted_from_a_live_trace() -> super::pipeline_tests::TempPipeline {
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
    assert_eq!(files_of(&store, &trace(1)).await, vec![picture_hash(1)]);
    store
}

/// The rewrite reconciles nothing itself on a store that queues with its append. The pass that append queued
/// releases the removed span's file - so a rewrite that dies after its append leaves nothing owed but a queued
/// entry, which a later pass takes.
#[tokio::test]
async fn the_pass_a_rewrite_queued_releases_the_removed_spans_file() {
    let store = a_span_deleted_from_a_live_trace().await;
    store.pipeline.reconcile_raw_records(64).await.unwrap();
    assert_eq!(
        only_record(&store.analytics).await.origin,
        RawOrigin::Deleted,
        "rewritten"
    );
    assert_eq!(
        files_of(&store, &trace(1)).await,
        vec![picture_hash(1)],
        "not reconciled by the rewrite itself: its pass is queued"
    );
    drain(&store.pipeline).await;
    assert!(
        files_of(&store, &trace(1)).await.is_empty(),
        "the queued pass released it"
    );
}

/// A pass whose reconciliation fails leaves its entries queued, and a later pass releases the file.
#[tokio::test]
async fn a_failed_reconciliation_leaves_the_entries_queued() {
    let store = a_span_deleted_from_a_live_trace().await;
    store.pipeline.reconcile_raw_records(64).await.unwrap();
    sideseat_adapter_sqlite::test_support::inject_trace_file_release_fault(&store.sqlite)
        .await
        .unwrap();
    store.pipeline.reconcile_raw_records(64).await.unwrap();
    assert!(
        !store
            .analytics
            .pending_raw_records(64, None)
            .await
            .unwrap()
            .is_empty(),
        "the entry stays queued"
    );
    sideseat_adapter_sqlite::test_support::clear_trace_file_release_fault(&store.sqlite)
        .await
        .unwrap();
    drain(&store.pipeline).await;
    assert!(
        files_of(&store, &trace(1)).await.is_empty(),
        "released on the retry"
    );
}

/// A span of the trace ingested again with the same picture before the pass keeps it: the pass reconciles
/// against what references the file then, not against what the rewrite removed.
#[tokio::test]
async fn a_span_ingested_again_before_the_pass_keeps_its_file() {
    let store = a_span_deleted_from_a_live_trace().await;
    store.pipeline.reconcile_raw_records(64).await.unwrap();
    ingest(&store.pipeline, &export(vec![span(1, 4, Some(picture(1)))])).await;
    drain(&store.pipeline).await;
    assert_eq!(
        files_of(&store, &trace(1)).await,
        vec![picture_hash(1)],
        "the new span's file survives"
    );
}

/// The trigger is a latest version that is not the received body, not only a rewrite. A record ingested again
/// after its rewrite, and repaired, has a fenced version on top - written without the spans the deletion fences
/// refuse - and the pass that follows still owes, and does, the file reconciliation the rewrite left.
#[tokio::test]
async fn a_record_repaired_after_its_rewrite_still_has_its_files_reconciled() {
    let store = a_span_deleted_from_a_live_trace().await;
    store.pipeline.reconcile_raw_records(64).await.unwrap();
    let rewritten = only_record(&store.analytics).await;
    assert_eq!(rewritten.origin, RawOrigin::Deleted);
    // What a repair after the rewrite leaves: the same content, keyed above it, as a fenced version.
    let repaired = RawRecordRow {
        origin: RawOrigin::Fenced,
        version: rewritten.version + 2,
        ..rewritten
    };
    store
        .analytics
        .append_raw_records(std::slice::from_ref(&repaired))
        .await
        .unwrap();
    assert_eq!(
        only_record(&store.analytics).await.origin,
        RawOrigin::Fenced
    );
    drain(&store.pipeline).await;
    assert!(
        files_of(&store, &trace(1)).await.is_empty(),
        "the removed span's file is released"
    );
}

/// With file storage disabled the pass cannot reconcile, but associations recorded while it was enabled are still
/// owed: the pass records the traces' retention cleanup, which the sweep keeps until files are enabled again,
/// rather than clearing its entries with the reconciliation lost.
#[tokio::test]
async fn with_files_disabled_the_pass_leaves_the_cleanup_owed() {
    let store = super::pipeline_tests::temp_pipeline(false).await;
    ingest(
        &store.pipeline,
        &export(vec![span(1, 1, None), span(1, 2, None)]),
    )
    .await;
    let project = ProjectId::from(PROJECT);
    // An association recorded while files were enabled, durable.
    let hash = picture_hash(9);
    store
        .database
        .associate_file(&trace(1), &project, &hash, Some("image/png"), 16, "blake3")
        .await
        .unwrap();
    store
        .database
        .confirm_trace_file_associations(&[(PROJECT.to_string(), trace(1), hash)])
        .await
        .unwrap();
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
    // The deletion recorded its own cleanup; this test is about the rewrite's.
    let before = sideseat_adapter_sqlite::test_support::owed_retention_cleanups(&store.sqlite)
        .await
        .unwrap();
    for (_, trace_id) in &before {
        store
            .database
            .complete_retention_cleanup(
                &project,
                &store
                    .database
                    .claim_retention_cleanup(16, 0)
                    .await
                    .unwrap()
                    .into_iter()
                    .filter(|(_, claimed, _)| claimed == trace_id)
                    .map(|(_, trace, token)| (trace, token))
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap();
    }
    drain(&store.pipeline).await;
    assert_eq!(
        sideseat_adapter_sqlite::test_support::owed_retention_cleanups(&store.sqlite)
            .await
            .unwrap(),
        vec![(PROJECT.to_string(), trace(1))],
        "the rewrite's file reconciliation is owed"
    );
}

/// A rewritten record shared by many traces, one of which carried a picture: the pass reconciles only the traces
/// associated with a file. Each trace a survivor scan reads costs a read of the record's raw content, so handing
/// it every indexed trace read the same body once per trace, for nothing to release.
#[tokio::test]
async fn the_pass_reconciles_only_traces_with_files() {
    let store = super::pipeline_tests::temp_pipeline(true).await;
    let mut spans = vec![span(1, 1, Some(picture(1))), span(1, 2, None)];
    spans.extend((2..=40u8).map(|t| span(t, t, None)));
    ingest(&store.pipeline, &export(spans)).await;
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
    let before = super::raw_lifecycle::RECONCILED_TRACES.with(std::cell::Cell::get);
    drain(&store.pipeline).await;
    let reconciled = super::raw_lifecycle::RECONCILED_TRACES.with(std::cell::Cell::get) - before;
    assert!(files_of(&store, &trace(1)).await.is_empty(), "released");
    eprintln!("traces handed to a survivor scan: {reconciled}");
    assert_eq!(
        reconciled, 1,
        "traces scanned for a record of 40, one of them with a file"
    );
}
