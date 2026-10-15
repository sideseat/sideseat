//! A rewrite prepared from an older read does not land over a newer version.

use super::raw::SpanKey;
use super::raw_lifecycle_tests::{PROJECT, export, ingest, key, only_record, picture, span, trace};
use super::*;
use sideseat_domain::raw_payload;
use sideseat_ports::types::RawRecordRow;

fn picture_hash(seed: u8) -> String {
    hex::encode(blake3::hash(picture(seed).as_bytes()).as_bytes())
}

async fn delete_span(store: &super::pipeline_tests::TempPipeline, span: SpanKey) {
    let project = ProjectId::from(PROJECT);
    store
        .database
        .record_deleted_spans_journalled(&project, std::slice::from_ref(&span))
        .await
        .unwrap();
    store
        .analytics
        .delete_spans(&project, &[span])
        .await
        .unwrap();
}

/// Two rewrites of one record prepared from the same read: the one prepared first, before a second deletion, lands
/// last. The newer rewrite removed both deleted spans and reconciled their files away; had the older one landed
/// over it, the latest version would name a picture the trace no longer holds - its bytes free to be collected,
/// the record undecodable. It is refused, and the latest version names only what the trace holds.
#[tokio::test]
async fn a_rewrite_prepared_before_a_newer_one_landed_is_refused() {
    let store = super::pipeline_tests::temp_pipeline(true).await;
    ingest(
        &store.pipeline,
        &export(vec![
            span(1, 1, Some(picture(1))),
            span(1, 2, Some(picture(2))),
            span(1, 3, None),
        ]),
    )
    .await;
    let project = ProjectId::from(PROJECT);
    let read = only_record(&store.analytics).await;

    delete_span(&store, key(1, 1)).await;
    let older = store
        .pipeline
        .prepare_rewrite(&project, &read)
        .await
        .unwrap()
        .expect("removes the first span");
    delete_span(&store, key(1, 2)).await;
    let newer = store
        .pipeline
        .prepare_rewrite(&project, &read)
        .await
        .unwrap()
        .expect("removes both");

    store.pipeline.land_rewrite(&project, newer).await.unwrap();
    store.pipeline.land_rewrite(&project, older).await.unwrap();

    let latest = only_record(&store.analytics).await;
    let named: Vec<String> = raw_payload::media_hashes(&latest.record)
        .unwrap()
        .iter()
        .map(hex::encode)
        .collect();
    let held = store
        .database
        .get_file_hashes_for_traces(&project, &[trace(1)])
        .await
        .unwrap();
    for hash in &named {
        assert!(
            held.contains(hash),
            "the latest version names {hash}, which the trace no longer holds (held: {held:?})"
        );
    }
    assert!(
        !named.contains(&picture_hash(2)),
        "the second deleted span's picture is gone from the latest version"
    );
}

/// The same, with the stale rewrite keyed above the newer one - as it is when its epoch was read after a collection
/// elsewhere advanced the project's: what decides is the version it was prepared from, not its key.
#[tokio::test]
async fn a_stale_rewrite_keyed_above_the_newer_one_is_refused_too() {
    let store = super::pipeline_tests::temp_pipeline(true).await;
    ingest(
        &store.pipeline,
        &export(vec![
            span(1, 1, Some(picture(1))),
            span(1, 2, Some(picture(2))),
            span(1, 3, None),
        ]),
    )
    .await;
    let project = ProjectId::from(PROJECT);
    let read = only_record(&store.analytics).await;
    delete_span(&store, key(1, 1)).await;
    let (mut older, older_traces) = store
        .pipeline
        .prepare_rewrite(&project, &read)
        .await
        .unwrap()
        .expect("removes the first span");
    delete_span(&store, key(1, 2)).await;
    let newer = store
        .pipeline
        .prepare_rewrite(&project, &read)
        .await
        .unwrap()
        .expect("removes both");
    let newer_version = newer.0.row.version;
    store.pipeline.land_rewrite(&project, newer).await.unwrap();

    older.row.version = sideseat_ports::types::raw_version_above(
        Some(newer_version),
        sideseat_ports::types::raw_version_epoch(newer_version) + 1,
        1,
    )
    .expect("a key above");
    store
        .pipeline
        .land_rewrite(&project, (older, older_traces))
        .await
        .unwrap();
    assert!(
        !raw_payload::media_hashes(&only_record(&store.analytics).await.record)
            .unwrap()
            .iter()
            .map(hex::encode)
            .any(|hash| hash == picture_hash(2)),
        "the stale content did not land"
    );
}

/// The version a rewrite was prepared from is named by its key and its bytes: a different version that landed at
/// the same key - a repair keyed alike - is not it, and the rewrite is refused.
#[tokio::test]
async fn a_rewrite_whose_source_was_replaced_at_the_same_key_is_refused() {
    let store = super::pipeline_tests::temp_pipeline(true).await;
    ingest(
        &store.pipeline,
        &export(vec![span(1, 1, Some(picture(1))), span(1, 3, None)]),
    )
    .await;
    let project = ProjectId::from(PROJECT);
    let read = only_record(&store.analytics).await;
    delete_span(&store, key(1, 1)).await;
    let rewrite = store
        .pipeline
        .prepare_rewrite(&project, &read)
        .await
        .unwrap()
        .expect("removes the span");
    // Another version, at the key the rewrite was prepared from, with other bytes.
    let other = RawRecordRow {
        record: [read.record.as_slice(), b"\0"].concat(),
        ..read.clone()
    };
    store
        .analytics
        .append_raw_records(std::slice::from_ref(&other))
        .await
        .unwrap();
    store
        .pipeline
        .land_rewrite(&project, rewrite)
        .await
        .unwrap();
    let latest = only_record(&store.analytics).await;
    assert_ne!(
        latest.origin,
        sideseat_ports::types::RawOrigin::Deleted,
        "refused"
    );
    assert_eq!(
        latest.record, other.record,
        "the version that landed over its source stays"
    );
}

/// Two rewrites of one record in one call, prepared from one version: the first lands and the second, judged
/// against the latest the first left, is refused.
#[tokio::test]
async fn two_rewrites_of_one_record_in_one_call_admit_only_the_first() {
    let store = super::pipeline_tests::temp_pipeline(true).await;
    ingest(
        &store.pipeline,
        &export(vec![span(1, 1, Some(picture(1))), span(1, 3, None)]),
    )
    .await;
    let project = ProjectId::from(PROJECT);
    let read = only_record(&store.analytics).await;
    delete_span(&store, key(1, 1)).await;
    let (first, _) = store
        .pipeline
        .prepare_rewrite(&project, &read)
        .await
        .unwrap()
        .expect("removes the span");
    let second = sideseat_ports::types::RawRewrite {
        row: RawRecordRow {
            version: first.row.version + 1,
            record: [first.row.record.as_slice(), b"\0"].concat(),
            ..first.row.clone()
        },
        based_on: read.clone(),
    };
    let appended = store
        .analytics
        .append_raw_rewrites(&[first.clone(), second])
        .await
        .unwrap();
    assert_eq!(appended.len(), 1);
    assert_eq!(
        only_record(&store.analytics).await.record,
        first.row.record,
        "the first is the latest"
    );
}
