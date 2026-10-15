//! What the stamp protocol costs a survivor release against the release-then-re-check one it replaces, measured
//! as an alternating A/B in one run, so load from outside the process falls on both alike.
//!
//! `cargo test --locked -p sideseat-ingestion --release --lib survivor_release_bench -- --ignored --nocapture`
//!
//! Each trace's record names a picture until a rewrite removes the span that carried it; before every timed
//! release the association is restored, untimed, so every release has the same thing to release. A is the
//! release-then-re-check protocol as it was: scan, release, and scan again because something was released, with
//! nothing to restore. B is the stamp protocol: read the horizon, scan, release, read what is left. Neither
//! reclaims bytes, which both would do alike.

use super::raw_lifecycle_tests::{PROJECT, drain, export, ingest, key, picture, span, trace};
use super::*;
use std::time::{Duration, Instant};

const TRACES: u8 = 64;
const SPANS_PER_TRACE: u8 = 10;
const ROUNDS: usize = 20;

fn summary(samples: &mut [Duration]) -> (f64, f64, f64) {
    samples.sort();
    let micros = |d: Duration| d.as_secs_f64() * 1e6;
    let median = micros(samples[samples.len() / 2]);
    let p90 = micros(samples[samples.len() * 9 / 10]);
    let mean = samples.iter().copied().map(micros).sum::<f64>() / samples.len() as f64;
    (median, p90, mean)
}

#[tokio::test]
#[ignore = "a measurement, not a check: run it on purpose"]
async fn survivor_release_bench() {
    let store = super::pipeline_tests::temp_pipeline(true).await;
    let project = ProjectId::from(PROJECT);
    for t in 1..=TRACES {
        let mut spans = vec![span(t, 1, Some(picture(t)))];
        spans.extend((2..=SPANS_PER_TRACE).map(|id| span(t, id, Some(format!("text {t} {id}")))));
        ingest(&store.pipeline, &export(spans)).await;
    }
    let repo = store.database.as_ref();
    let hash = |t: u8| hex::encode(blake3::hash(picture(t).as_bytes()).as_bytes());
    // A second trace holds each picture, so the file row outlives the setup's own release and every timed
    // release's untimed restore has a file to name; no timed release touches the holder.
    let holder = trace(u8::MAX);
    for t in 1..=TRACES {
        repo.restore_durable_trace_file(&project, &holder, &hash(t))
            .await
            .unwrap();
    }
    let expired: Vec<_> = (1..=TRACES).map(|t| key(t, 1)).collect();
    store
        .database
        .record_deleted_spans_journalled(&project, &expired)
        .await
        .unwrap();
    store
        .analytics
        .delete_spans(&project, &expired)
        .await
        .unwrap();
    store.pipeline.reconcile_raw_records(1024).await.unwrap();
    drain(&store.pipeline).await;

    let analytics = store.analytics.as_ref();
    let mut a = Vec::new();
    let mut b = Vec::new();
    for round in 0..ROUNDS {
        for t in 1..=TRACES {
            let trace_id = trace(t);
            repo.restore_durable_trace_file(&project, &trace_id, &hash(t))
                .await
                .unwrap();
            let release_then_recheck = (usize::from(t) + round) % 2 == 0;
            let started = Instant::now();
            let released = if release_then_recheck {
                let keep = sideseat_domain::files::FileService::hashes_referenced_by_trace(
                    &project, &trace_id, analytics,
                )
                .await
                .unwrap();
                let released = repo
                    .release_trace_files_except(&project, &trace_id, &keep, i64::MAX)
                    .await
                    .unwrap();
                if !released.is_empty() {
                    sideseat_domain::files::FileService::hashes_referenced_by_trace(
                        &project, &trace_id, analytics,
                    )
                    .await
                    .unwrap();
                }
                released
            } else {
                let horizon = repo.trace_file_confirm_horizon(&project).await.unwrap();
                let keep = sideseat_domain::files::FileService::hashes_referenced_by_trace(
                    &project, &trace_id, analytics,
                )
                .await
                .unwrap();
                let released = repo
                    .release_trace_files_except(&project, &trace_id, &keep, horizon)
                    .await
                    .unwrap();
                repo.get_file_hashes_for_traces(&project, std::slice::from_ref(&trace_id))
                    .await
                    .unwrap();
                released
            };
            let elapsed = started.elapsed();
            assert_eq!(
                released,
                vec![hash(t)],
                "each release has the same thing to release"
            );
            if release_then_recheck {
                a.push(elapsed)
            } else {
                b.push(elapsed)
            }
        }
    }
    let (am, ap, amean) = summary(&mut a);
    let (bm, bp, bmean) = summary(&mut b);
    eprintln!(
        "survivor release per trace, {} samples each: release-then-re-check median {am:.0} us p90 {ap:.0} us mean \
         {amean:.0} us; stamp median {bm:.0} us p90 {bp:.0} us mean {bmean:.0} us; stamp / release-then-re-check \
         median {:.3}",
        a.len(),
        bm / am
    );
}
