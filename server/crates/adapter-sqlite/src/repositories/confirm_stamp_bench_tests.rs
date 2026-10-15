//! What the confirm stamp costs a batch's confirm, measured as an alternating A/B in one run over the service's own
//! file-backed store, so load from outside the process falls on both alike.
//!
//! `cargo test -p sideseat-adapter-sqlite --release --lib confirm_stamp_bench -- --ignored --nocapture`
//!
//! A is the confirm as it was before stamps, reproduced exactly; B is `confirm_trace_file_associations`, which draws
//! the batch's stamp first and applies the larger of each row's and its own.

use super::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

const BATCHES: usize = 400;
const PER_BATCH: usize = 8;

#[derive(Debug)]
struct Clock;

impl sideseat_ports::clock::Clock for Clock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }
}

/// The confirm before stamps, statement for statement: deduplicated, then one `UPDATE` per association in one
/// transaction.
async fn unstamped_confirm(pool: &SqlitePool, associations: &[(String, String, String)]) -> u64 {
    if associations.is_empty() {
        return 0;
    }
    let mut unique: Vec<&(String, String, String)> = associations.iter().collect();
    unique.sort_unstable();
    unique.dedup();
    let mut tx = pool.begin().await.unwrap();
    let mut confirmed = 0u64;
    for (project_id, trace_id, file_hash) in unique {
        let result = sqlx::query(
            "UPDATE trace_files SET durable = 1, pending_writers = MAX(pending_writers - 1, 0) \
             WHERE project_id = ? AND trace_id = ? AND file_hash = ?",
        )
        .bind(project_id)
        .bind(trace_id)
        .bind(file_hash)
        .execute(&mut *tx)
        .await
        .unwrap();
        confirmed += result.rows_affected();
    }
    tx.commit().await.unwrap();
    confirmed
}

#[tokio::test]
#[ignore = "a measurement, not a check: run it on purpose"]
async fn confirm_stamp_bench() {
    let temp = tempfile::TempDir::new().unwrap();
    let service = crate::SqliteService::init(
        &sideseat_core::storage::AppStorage::init_for_test(temp.path().to_path_buf()),
        Arc::new(Clock),
    )
    .await
    .unwrap();
    let pool = service.pool();
    let mut a = Vec::new();
    let mut b = Vec::new();
    for batch in 0..BATCHES {
        let associations: Vec<(String, String, String)> = (0..PER_BATCH)
            .map(|i| {
                (
                    "default".to_string(),
                    format!("trace-{batch}"),
                    format!("{:064x}", batch * PER_BATCH + i),
                )
            })
            .collect();
        for (project_id, trace_id, file_hash) in &associations {
            associate_file(
                pool,
                trace_id,
                project_id,
                file_hash,
                None,
                5,
                "sha256",
                1_700_000_000,
            )
            .await
            .unwrap();
        }
        let stamped = batch % 2 == 1;
        let started = Instant::now();
        let confirmed = if stamped {
            confirm_trace_file_associations(pool, &associations)
                .await
                .unwrap()
        } else {
            unstamped_confirm(pool, &associations).await
        };
        let elapsed = started.elapsed();
        assert_eq!(
            confirmed, PER_BATCH as u64,
            "both arms confirm the whole batch"
        );
        if stamped {
            b.push(elapsed)
        } else {
            a.push(elapsed)
        }
    }
    let summary = |samples: &mut Vec<Duration>| {
        samples.sort();
        let micros = |d: Duration| d.as_secs_f64() * 1e6;
        (
            micros(samples[samples.len() / 2]),
            micros(samples[samples.len() * 9 / 10]),
        )
    };
    let (am, ap) = summary(&mut a);
    let (bm, bp) = summary(&mut b);
    eprintln!(
        "confirm of {PER_BATCH} associations, {} batches each: unstamped median {am:.0} us p90 {ap:.0} us; stamped \
         median {bm:.0} us p90 {bp:.0} us; stamped / unstamped median {:.3}",
        a.len(),
        bm / am
    );
}
