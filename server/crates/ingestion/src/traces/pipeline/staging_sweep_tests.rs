//! The staging sweep takes each pending payload on its own: one that cannot be loaded does not stop the ones behind
//! it. A queue reference acknowledged while the store was unavailable leaves its payload to the sweep alone, so a
//! sweep that one older payload - its bytes gone - stopped on every pass stranded every export after it.

use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;

use super::pipeline_tests::temp_pipeline;
use super::*;
use crate::staging::StagedWrite;

/// The bytes staging holds for an export: its raw record, as the ingest path stages it.
fn staged_bytes(request: &ExportTraceServiceRequest) -> Vec<u8> {
    ReceivedPayload::new(
        request.encode_to_vec(),
        sideseat_domain::raw_payload::RawContent::Protobuf,
        chrono::Utc::now(),
    )
    .staged()
}

fn export(trace: u8) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![trace; 16],
                    span_id: vec![1; 8],
                    name: "step".into(),
                    start_time_unix_nano: 1_700_000_000_000_000_000,
                    end_time_unix_nano: 1_700_000_000_000_000_100,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_payload_that_cannot_be_loaded_does_not_stop_the_sweep() {
    let store = temp_pipeline(false).await;
    let staging = Arc::clone(&store.pipeline.staging);
    let write = || StagedWrite {
        partition_key: "p".to_string(),
        raw_id: None,
    };
    let broken = staging
        .stage(
            DEFAULT_PROJECT_ID,
            StagedSignal::Traces,
            &staged_bytes(&export(1)),
            chrono::Utc::now(),
            crate::traces::confirmation_records(&export(1)),
            write(),
        )
        .await
        .expect("older");
    let healthy = staging
        .stage(
            DEFAULT_PROJECT_ID,
            StagedSignal::Traces,
            &staged_bytes(&export(2)),
            chrono::Utc::now(),
            crate::traces::confirmation_records(&export(2)),
            write(),
        )
        .await
        .expect("newer");
    // The older payload's bytes are gone.
    let (payload, _) = staging.load(&broken.id).await.unwrap().unwrap();
    store
        .pipeline
        .file_service
        .storage()
        .delete(
            &payload.project_id,
            sideseat_ports::blobs::ObjectName::new(&payload.blob_hash, 0),
        )
        .await
        .unwrap();

    staging
        .redrive_once(&store.pipeline, 100)
        .await
        .expect("the sweep runs past a payload it cannot load");
    assert!(
        store
            .analytics
            .get_span(
                &ProjectId::from(DEFAULT_PROJECT_ID),
                &hex::encode([2u8; 16]),
                &hex::encode([1u8; 8]),
            )
            .await
            .unwrap()
            .is_some(),
        "the healthy export behind it is written"
    );
    assert!(
        store
            .database
            .get_staged_payload(&healthy.id)
            .await
            .unwrap()
            .is_none(),
        "and settled"
    );
    // And the broken one is counted against, so it is quarantined at the cap rather than blocking for ever.
    let counted = store
        .database
        .get_staged_payload(&broken.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(counted.redrive_attempts, 1);
}

/// A payload whose span a later revision superseded is settled by reading its own bytes; with those gone, the
/// failure is the payload's own, and counted - so it is quarantined at the cap instead of holding the head of every
/// pass, and the healthy export behind it is written, one payload a pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_superseded_payload_whose_bytes_are_gone_does_not_hold_the_sweep() {
    let store = temp_pipeline(false).await;
    let staging = Arc::clone(&store.pipeline.staging);
    let write = || StagedWrite {
        partition_key: "p".to_string(),
        raw_id: None,
    };
    let first = export(1);
    let broken = staging
        .stage(
            DEFAULT_PROJECT_ID,
            StagedSignal::Traces,
            &staged_bytes(&first),
            chrono::Utc::now(),
            crate::traces::confirmation_records(&first),
            write(),
        )
        .await
        .expect("older");
    // A later revision of its span is stored.
    let mut revised = export(1);
    revised.resource_spans[0].scope_spans[0].spans[0].name = "revised".into();
    let received = ReceivedPayload::new(
        revised.encode_to_vec(),
        sideseat_domain::raw_payload::RawContent::Protobuf,
        chrono::Utc::now() + chrono::Duration::seconds(1),
    );
    store.pipeline.ingest_now(&revised, &received).await;
    let healthy = staging
        .stage(
            DEFAULT_PROJECT_ID,
            StagedSignal::Traces,
            &staged_bytes(&export(2)),
            chrono::Utc::now(),
            crate::traces::confirmation_records(&export(2)),
            write(),
        )
        .await
        .expect("newer");
    let (payload, _) = staging.load(&broken.id).await.unwrap().unwrap();
    store
        .pipeline
        .file_service
        .storage()
        .delete(
            &payload.project_id,
            sideseat_ports::blobs::ObjectName::new(&payload.blob_hash, 0),
        )
        .await
        .unwrap();

    // One payload a pass, each pass after the last and wrapping at the end: the healthy one is reached at once, and
    // the broken one - visited every other pass - is quarantined at the registry's redrive cap of five.
    for _ in 0..12 {
        staging
            .redrive_once(&store.pipeline, 1)
            .await
            .expect("sweep");
    }
    assert!(
        store
            .database
            .get_staged_payload(&healthy.id)
            .await
            .unwrap()
            .is_none(),
        "the healthy export behind it is written and settled"
    );
    assert!(
        store
            .database
            .get_staged_payload(&broken.id)
            .await
            .unwrap()
            .is_some_and(|p| p.unconfirmed),
        "the broken one quarantined, not lost"
    );
}

/// Rows the sweep cannot take because the blob store is unavailable for them - uncounted, so they stay pending - do
/// not hold the head of every pass: each pass continues after the last, so the healthy export behind them is
/// written.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rows_the_store_is_unavailable_for_do_not_hold_the_sweep() {
    use std::os::unix::fs::PermissionsExt as _;
    let store = temp_pipeline(false).await;
    let staging = Arc::clone(&store.pipeline.staging);
    let write = || StagedWrite {
        partition_key: "p".to_string(),
        raw_id: None,
    };
    let unreadable = staging
        .stage(
            DEFAULT_PROJECT_ID,
            StagedSignal::Traces,
            &staged_bytes(&export(1)),
            chrono::Utc::now(),
            crate::traces::confirmation_records(&export(1)),
            write(),
        )
        .await
        .expect("older");
    let healthy = staging
        .stage(
            DEFAULT_PROJECT_ID,
            StagedSignal::Traces,
            &staged_bytes(&export(2)),
            chrono::Utc::now(),
            crate::traces::confirmation_records(&export(2)),
            write(),
        )
        .await
        .expect("newer");
    // The older payload's bytes cannot be read: an I/O failure, which says nothing about them.
    let (payload, _) = staging.load(&unreadable.id).await.unwrap().unwrap();
    fn find(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find(&path, name) {
                    return Some(found);
                }
            } else if path.file_name()?.to_string_lossy().contains(name) {
                return Some(path);
            }
        }
        None
    }
    let blob = find(store.temp.path(), &payload.blob_hash).expect("the staged blob");
    std::fs::set_permissions(&blob, std::fs::Permissions::from_mode(0o000)).unwrap();

    for _ in 0..2 {
        staging
            .redrive_once(&store.pipeline, 1)
            .await
            .expect("sweep");
    }
    std::fs::set_permissions(&blob, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        store
            .database
            .get_staged_payload(&healthy.id)
            .await
            .unwrap()
            .is_none(),
        "the healthy export behind it is written and settled"
    );
    let held = store
        .database
        .get_staged_payload(&unreadable.id)
        .await
        .unwrap()
        .expect("still staged");
    assert_eq!(
        (held.redrive_attempts, held.unconfirmed),
        (0, false),
        "uncounted"
    );
}

/// A traversal ends at the receipt bound it began with, so exports arriving before every pass cannot keep the sweep
/// from coming back to an older row it skipped: once that row's bytes are readable again, a later traversal takes it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arrivals_do_not_keep_the_sweep_from_a_row_it_skipped() {
    use std::os::unix::fs::PermissionsExt as _;
    let store = temp_pipeline(false).await;
    let staging = Arc::clone(&store.pipeline.staging);
    let stage = |trace: u8| {
        let staging = Arc::clone(&staging);
        async move {
            staging
                .stage(
                    DEFAULT_PROJECT_ID,
                    StagedSignal::Traces,
                    &staged_bytes(&export(trace)),
                    chrono::Utc::now(),
                    crate::traces::confirmation_records(&export(trace)),
                    StagedWrite {
                        partition_key: "p".to_string(),
                        raw_id: None,
                    },
                )
                .await
                .expect("staged")
        }
    };
    let skipped = stage(1).await;
    let (payload, _) = staging.load(&skipped.id).await.unwrap().unwrap();
    fn find(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find(&path, name) {
                    return Some(found);
                }
            } else if path.file_name()?.to_string_lossy().contains(name) {
                return Some(path);
            }
        }
        None
    }
    let blob = find(store.temp.path(), &payload.blob_hash).expect("the staged blob");
    std::fs::set_permissions(&blob, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _first = stage(2).await;
    staging
        .redrive_once(&store.pipeline, 1)
        .await
        .expect("sweep");
    std::fs::set_permissions(&blob, std::fs::Permissions::from_mode(0o600)).unwrap();

    // A new export before every pass, one row a pass.
    for trace in 3..9 {
        let _arrival = stage(trace).await;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        staging
            .redrive_once(&store.pipeline, 1)
            .await
            .expect("sweep");
    }
    assert!(
        store
            .database
            .get_staged_payload(&skipped.id)
            .await
            .unwrap()
            .is_none(),
        "the skipped row is taken once readable"
    );
}
