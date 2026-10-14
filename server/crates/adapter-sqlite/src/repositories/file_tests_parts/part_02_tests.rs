/// Deleting several traces that share a file must release every reference they held.
///
/// The loop decremented once per distinct hash, so deleting three traces that referenced one file
/// removed three associations and one reference - leaving a file nothing points at and nothing will
/// ever collect.
#[tokio::test]
async fn deleting_traces_releases_every_reference_they_held() {
    let pool = setup_test_pool().await;
    let traces = ["trace-a", "trace-b", "trace-c"];
    for trace in traces {
        associate_file(
            &pool,
            trace,
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
    }

    let counts = get_file_reference_counts_for_traces(
        &pool,
        "default",
        &traces.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    assert_eq!(counts, vec![(test_hash().to_string(), 3)]);

    delete_trace_files(
        &pool,
        "default",
        &traces.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    let released = sync_ref_count(&pool, "default", test_hash()).await.unwrap();
    assert_eq!(
        released,
        Some(0),
        "all three references released at once, so the file is collectable"
    );
}

/// `ref_count` must equal the number of trace associations, or deleting one trace deletes a file
/// another trace still shows.
///
/// The defect this pins: ingestion incremented once per batch-unique hash while associating per
/// trace, so two traces of one batch sharing a file held two associations and one count. Deleting
/// either took the count to zero and removed the bytes from under the other.
#[tokio::test]
async fn ref_count_matches_the_number_of_trace_associations() {
    let pool = setup_test_pool().await;

    // Two traces of one batch reference the same content-addressed file. Ingestion increments
    // once per association, which is what the loop in `write_and_record_files` now does.
    for trace in ["trace-a", "trace-b"] {
        upsert_file(
            &pool,
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        insert_trace_file(&pool, trace, "default", test_hash())
            .await
            .unwrap();
    }

    // Deleting the first trace must leave the file alive for the second.
    delete_trace_files(&pool, "default", &["trace-a".to_string()])
        .await
        .unwrap();
    let after_first = decrement_ref_count(&pool, "default", test_hash())
        .await
        .unwrap();
    assert_eq!(
        after_first,
        Some(1),
        "the file is still referenced by trace-b, so it must not be collectable"
    );

    // Deleting the second releases it.
    delete_trace_files(&pool, "default", &["trace-b".to_string()])
        .await
        .unwrap();
    let after_second = decrement_ref_count(&pool, "default", test_hash())
        .await
        .unwrap();
    assert_eq!(
        after_second,
        Some(0),
        "with no trace referencing it the file is collectable"
    );
}

/// A retention cleanup that keeps failing stays owed and shows it: the backlog counts it, dates it from its
/// recording, and its attempts grow with every pass that claims it without completing it.
#[tokio::test]
async fn an_owed_retention_cleanup_shows_its_age_and_failed_passes() {
    let pool = setup_test_pool().await;
    let project = sideseat_ports::types::ProjectId::from("default");
    let empty = crate::repositories::governance::retention_cleanup_backlog(&pool, &project)
        .await
        .unwrap();
    assert_eq!(
        empty,
        sideseat_ports::types::RetentionCleanupBacklog::default()
    );

    use crate::repositories::retention_cleanup::{
        claim_retention_cleanup, complete_retention_cleanup, record_retention_cleanup,
    };
    let tokens = record_retention_cleanup(&pool, "default", &["t1".to_string()], 1_000)
        .await
        .unwrap();
    for pass in 0..3 {
        let claimed = claim_retention_cleanup(&pool, 16, 0, 2_000 + pass)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1, "the failed cleanup is claimed again");
    }
    let stalled = crate::repositories::governance::retention_cleanup_backlog(&pool, &project)
        .await
        .unwrap();
    assert_eq!(stalled.pending, 1);
    assert_eq!(
        stalled.oldest_pending_at,
        chrono::DateTime::from_timestamp(1_000, 0)
    );
    assert_eq!(stalled.most_attempts, 3);

    complete_retention_cleanup(&pool, "default", &tokens)
        .await
        .unwrap();
    let done = crate::repositories::governance::retention_cleanup_backlog(&pool, &project)
        .await
        .unwrap();
    assert_eq!(done.pending, 0);
}

/// A claim that is no longer the row's - taken over by another collector, or held on an incarnation that was
/// collected and created again - neither releases nor deletes the row, so a collector that paused past its
/// claim cannot clear another's claim or take a row created after its own.
#[tokio::test]
async fn a_superseded_claim_neither_releases_nor_deletes_the_row() {
    let pool = setup_test_pool().await;
    upsert_file(&pool, "default", test_hash(), None, 1, "sha256")
        .await
        .unwrap();
    let first = super::claim_file_for_deletion(&pool, "default", test_hash(), TEST_NOW)
        .await
        .unwrap()
        .expect("an unreferenced row is claimable");
    let taken_over =
        super::reclaim_stale_file(&pool, "default", test_hash(), TEST_NOW, TEST_NOW + 5)
            .await
            .unwrap()
            .expect("the claim as observed is taken over");

    release_deletion_claim(&pool, "default", test_hash(), &first)
        .await
        .unwrap();
    assert!(
        !delete_file_if_unreferenced(&pool, "default", test_hash(), &first)
            .await
            .unwrap(),
        "the taken-over claim must not delete the row"
    );
    assert_eq!(
        held_claim(&pool, test_hash()).await,
        taken_over,
        "nor release the claim that replaced it"
    );

    assert!(
        delete_file_if_unreferenced(&pool, "default", test_hash(), &taken_over)
            .await
            .unwrap()
    );
    upsert_file(&pool, "default", test_hash(), None, 1, "sha256")
        .await
        .unwrap();
    let again = super::claim_file_for_deletion(&pool, "default", test_hash(), TEST_NOW + 5)
        .await
        .unwrap()
        .expect("the row created again is claimable");
    assert_ne!(again.incarnation, taken_over.incarnation);
    assert_eq!(
        again.claimed_at, taken_over.claimed_at,
        "claimed in the same second, so only the incarnation tells the claims apart"
    );
    release_deletion_claim(&pool, "default", test_hash(), &taken_over)
        .await
        .unwrap();
    assert_eq!(
        held_claim(&pool, test_hash()).await,
        again,
        "a claim on the collected incarnation must not release the row created after it"
    );
    assert!(
        !delete_file_if_unreferenced(&pool, "default", test_hash(), &taken_over)
            .await
            .unwrap(),
        "nor delete it"
    );
}

/// The store the server runs: file-backed, pooled, WAL.
async fn served_store() -> (tempfile::TempDir, crate::SqliteService) {
    let root = tempfile::TempDir::new().expect("temp dir");
    let storage = sideseat_core::storage::AppStorage::init_for_test(root.path().to_path_buf());
    let service = crate::SqliteService::init(&storage, std::sync::Arc::new(crate::TestClock))
        .await
        .expect("sqlite");
    (root, service)
}

/// A file association whose fence read meets a writer committing before the association writes is not refused.
/// It read the file's deletion claim and then wrote, in a transaction that took the write lock only at its first
/// write, so the commit in between made SQLite answer `database is locked` at once, busy timeout or not - and the
/// ingest batch carrying the association was refused and redelivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_association_is_not_refused_by_a_commit_after_its_fence_read() {
    let (_root, service) = served_store().await;
    let pool = service.pool().clone();
    for existing in [false, true] {
        let hash = format!("{:064x}", u64::from(existing));
        if existing {
            super::associate_file(
                &pool, "first", "default", &hash, None, 128, "sha256", TEST_NOW,
            )
            .await
            .expect("the file exists");
        }
        // Another writer holds the lock, then commits while the association waits.
        let mut writer = pool.begin().await.expect("writer");
        sqlx::query("UPDATE files SET updated_at = updated_at")
            .execute(&mut *writer)
            .await
            .expect("lock");
        let association = {
            let pool = pool.clone();
            let hash = hash.clone();
            tokio::spawn(async move {
                if existing {
                    super::associate_existing_file(&pool, "second", "default", &hash, TEST_NOW)
                        .await
                } else {
                    super::associate_file(
                        &pool, "second", "default", &hash, None, 128, "sha256", TEST_NOW,
                    )
                    .await
                }
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        sqlx::query("INSERT INTO retention_cleanup_tokens (id, last) VALUES (1, 1) ON CONFLICT(id) DO UPDATE SET last = last + 1")
            .execute(&mut *writer)
            .await
            .expect("a write");
        writer.commit().await.expect("commit");
        assert!(
            association.await.expect("task").expect("not refused"),
            "associated (existing file: {existing})"
        );
    }
}

/// File associations racing other writers: none is refused, and each records its reference.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn associations_racing_other_writers_are_never_refused_as_locked() {
    let (_root, service) = served_store().await;
    let pool = service.pool().clone();
    for n in 0..4u64 {
        super::associate_file(
            &pool,
            "seed",
            "default",
            &format!("{n:064x}"),
            None,
            128,
            "sha256",
            TEST_NOW,
        )
        .await
        .expect("seed");
    }
    let mut tasks = Vec::new();
    for worker in 0..8u8 {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            let mut refused = Vec::new();
            for round in 0..20u8 {
                let hash = format!("{:064x}", u64::from(round % 4));
                let trace = format!("trace-{worker}-{round}");
                let result = if worker % 2 == 0 {
                    super::associate_file(
                        &pool, &trace, "default", &hash, None, 128, "sha256", TEST_NOW,
                    )
                    .await
                } else {
                    super::associate_existing_file(&pool, &trace, "default", &hash, TEST_NOW).await
                };
                match result {
                    Ok(true) => {}
                    Ok(false) => refused.push("not associated".to_string()),
                    Err(error) => refused.push(error.to_string()),
                }
                // Another writer, as staging's registrations and the confirms are.
                crate::repositories::retention_cleanup::record_retention_cleanup(
                    &pool,
                    "default",
                    &[trace],
                    TEST_NOW,
                )
                .await
                .expect("a plain writer");
            }
            refused
        }));
    }
    let mut refused = Vec::new();
    for task in tasks {
        refused.extend(task.await.expect("worker"));
    }
    assert!(
        refused.is_empty(),
        "{} refused: {:?}",
        refused.len(),
        refused.first()
    );
}
