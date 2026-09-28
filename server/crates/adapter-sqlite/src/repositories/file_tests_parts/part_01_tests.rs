use super::*;

const TEST_NOW: i64 = 1_700_000_000;

async fn upsert_file(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
    hash_algo: &str,
) -> Result<i64, SqliteError> {
    super::upsert_file(
        pool, project_id, file_hash, media_type, size_bytes, hash_algo, TEST_NOW,
    )
    .await
}

async fn decrement_ref_count(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<Option<i64>, SqliteError> {
    super::decrement_ref_count(pool, project_id, file_hash, TEST_NOW + 1).await
}

async fn get_stale_claimed_files(
    pool: &SqlitePool,
    older_than_secs: i64,
) -> Result<Vec<(String, String, i64)>, SqliteError> {
    super::get_stale_claimed_files(pool, older_than_secs, TEST_NOW).await
}

async fn reclaim_stale_file(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
    observed_deleting_at: i64,
) -> Result<bool, SqliteError> {
    super::reclaim_stale_file(pool, project_id, file_hash, observed_deleting_at, TEST_NOW).await
}

async fn claim_file_for_deletion(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, SqliteError> {
    super::claim_file_for_deletion(pool, project_id, file_hash, TEST_NOW).await
}

async fn sync_ref_count(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<Option<i64>, SqliteError> {
    super::sync_ref_count(pool, project_id, file_hash, TEST_NOW + 1).await
}

#[allow(clippy::too_many_arguments)]
async fn associate_file(
    pool: &SqlitePool,
    trace_id: &str,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
    hash_algo: &str,
) -> Result<bool, SqliteError> {
    super::associate_file(
        pool, trace_id, project_id, file_hash, media_type, size_bytes, hash_algo, TEST_NOW,
    )
    .await
}

fn test_hash() -> &'static str {
    "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
}

/// A file's association survives a peer batch failing, once any referencing batch has committed.
///
/// Two batches reference the same `(project, trace, hash)`, one commits, and one fails. The commit makes the
/// association durable; the failed batch only decrements its writer count and cannot remove the durable row.
#[tokio::test]
async fn a_committed_association_survives_a_peer_batch_releasing_it() {
    let pool = setup_test_pool().await;
    let (project, trace, hash) = ("default", "trace-1", test_hash());

    // Two batches reference the same association: pending_writers = 2, not yet durable.
    for _ in 0..2 {
        assert!(
            associate_file(
                &pool,
                trace,
                project,
                hash,
                Some("image/png"),
                1024,
                "sha256"
            )
            .await
            .unwrap(),
            "every reference must be recorded and tracked"
        );
    }
    let pending: i64 =
            sqlx::query_scalar("SELECT pending_writers FROM trace_files WHERE project_id = ? AND trace_id = ? AND file_hash = ?")
                .bind(project).bind(trace).bind(hash)
                .fetch_one(&pool).await.unwrap();
    assert_eq!(
        pending, 2,
        "two referencing batches must count as two in-flight writers"
    );

    // Batch A commits while batch B releases its provisional writer.
    confirm_trace_file_associations(
        &pool,
        &[(project.to_string(), trace.to_string(), hash.to_string())],
    )
    .await
    .unwrap();
    let deleted = release_trace_file_association(&pool, project, trace, hash)
        .await
        .unwrap();
    assert!(
        !deleted,
        "a durable association must survive a peer batch's release"
    );

    // The association and its file reference are intact.
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trace_files WHERE project_id = ? AND trace_id = ? AND file_hash = ?",
    )
    .bind(project)
    .bind(trace)
    .bind(hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows, 1,
        "the committed batch's association must still exist"
    );
    let ref_count: i64 =
        sqlx::query_scalar("SELECT ref_count FROM files WHERE project_id = ? AND file_hash = ?")
            .bind(project)
            .bind(hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        ref_count, 1,
        "the file must still be referenced, not orphaned"
    );
}

/// When every referencing batch fails, the association is removed and the file reclaimed.
#[tokio::test]
async fn an_association_no_batch_commits_is_released() {
    let pool = setup_test_pool().await;
    let (project, trace, hash) = ("default", "trace-1", test_hash());

    for _ in 0..2 {
        associate_file(
            &pool,
            trace,
            project,
            hash,
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
    }
    // Both batches fail. Only the last release, which brings the count to zero on a non-durable row,
    // deletes it - so nothing is orphaned and nothing is deleted prematurely.
    assert!(
        !release_trace_file_association(&pool, project, trace, hash)
            .await
            .unwrap(),
        "the first release must not delete while a peer is still in flight"
    );
    assert!(
        release_trace_file_association(&pool, project, trace, hash)
            .await
            .unwrap(),
        "the last release must delete the association no batch committed"
    );
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trace_files WHERE project_id = ? AND trace_id = ? AND file_hash = ?",
    )
    .bind(project)
    .bind(trace)
    .bind(hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 0, "an association no batch committed must be gone");
}

#[tokio::test]
async fn test_upsert_file_new() {
    let pool = setup_test_pool().await;

    let ref_count = upsert_file(
        &pool,
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await
    .unwrap();

    assert_eq!(ref_count, 1);

    let file = get_file(&pool, "default", test_hash()).await.unwrap();
    assert!(file.is_some());
    let file = file.unwrap();
    assert_eq!(file.project_id, "default");
    assert_eq!(file.file_hash, test_hash());
    assert_eq!(file.media_type, Some("image/png".to_string()));
    assert_eq!(file.size_bytes, 1024);
    assert_eq!(file.ref_count, 1);
}

#[tokio::test]
async fn test_upsert_file_increments_ref_count() {
    let pool = setup_test_pool().await;

    let ref1 = upsert_file(
        &pool,
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await
    .unwrap();
    assert_eq!(ref1, 1);

    let ref2 = upsert_file(
        &pool,
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await
    .unwrap();
    assert_eq!(ref2, 2);

    let ref3 = upsert_file(
        &pool,
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await
    .unwrap();
    assert_eq!(ref3, 3);
}

#[tokio::test]
async fn test_decrement_ref_count() {
    let pool = setup_test_pool().await;

    upsert_file(&pool, "default", test_hash(), None, 1024, "sha256")
        .await
        .unwrap();
    upsert_file(&pool, "default", test_hash(), None, 1024, "sha256")
        .await
        .unwrap();

    let new_count = decrement_ref_count(&pool, "default", test_hash())
        .await
        .unwrap();
    assert_eq!(new_count, Some(1));

    let new_count = decrement_ref_count(&pool, "default", test_hash())
        .await
        .unwrap();
    assert_eq!(new_count, Some(0));
}

#[tokio::test]
async fn test_decrement_ref_count_not_found() {
    let pool = setup_test_pool().await;

    let result = decrement_ref_count(&pool, "default", test_hash())
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_file_exists() {
    let pool = setup_test_pool().await;

    assert!(!file_exists(&pool, "default", test_hash()).await.unwrap());

    upsert_file(&pool, "default", test_hash(), None, 1024, "sha256")
        .await
        .unwrap();

    assert!(file_exists(&pool, "default", test_hash()).await.unwrap());
}

#[tokio::test]
async fn test_delete_file() {
    let pool = setup_test_pool().await;

    upsert_file(&pool, "default", test_hash(), None, 1024, "sha256")
        .await
        .unwrap();

    let deleted = delete_file(&pool, "default", test_hash()).await.unwrap();
    assert!(deleted);

    assert!(!file_exists(&pool, "default", test_hash()).await.unwrap());
}

#[tokio::test]
async fn test_trace_file_associations() {
    let pool = setup_test_pool().await;
    let hash1 = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
    let hash2 = "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3";

    // Insert file records first
    upsert_file(&pool, "default", hash1, None, 1024, "sha256")
        .await
        .unwrap();
    upsert_file(&pool, "default", hash2, None, 2048, "sha256")
        .await
        .unwrap();

    // Associate with trace
    insert_trace_file(&pool, "trace1", "default", hash1)
        .await
        .unwrap();
    insert_trace_file(&pool, "trace1", "default", hash2)
        .await
        .unwrap();
    insert_trace_file(&pool, "trace2", "default", hash1)
        .await
        .unwrap();

    // Get hashes for trace1
    let hashes = get_file_hashes_for_traces(&pool, "default", &["trace1".to_string()])
        .await
        .unwrap();
    assert_eq!(hashes.len(), 2);

    // Get hashes for both traces
    let hashes = get_file_hashes_for_traces(
        &pool,
        "default",
        &["trace1".to_string(), "trace2".to_string()],
    )
    .await
    .unwrap();
    // hash1 appears in both, hash2 only in trace1 - should deduplicate
    assert_eq!(hashes.len(), 2);
}

#[tokio::test]
async fn test_delete_trace_files() {
    let pool = setup_test_pool().await;
    let hash = test_hash();

    upsert_file(&pool, "default", hash, None, 1024, "sha256")
        .await
        .unwrap();
    insert_trace_file(&pool, "trace1", "default", hash)
        .await
        .unwrap();

    let deleted = delete_trace_files(&pool, "default", &["trace1".to_string()])
        .await
        .unwrap();
    assert_eq!(
        deleted.len(),
        1,
        "the delete reports the hashes it removed, which is the set the caller must reconcile"
    );

    let hashes = get_file_hashes_for_traces(&pool, "default", &["trace1".to_string()])
        .await
        .unwrap();
    assert!(hashes.is_empty());
}

#[tokio::test]
async fn test_get_project_storage_bytes() {
    let pool = setup_test_pool().await;

    let bytes = get_project_storage_bytes(&pool, "default").await.unwrap();
    assert_eq!(bytes, 0);

    upsert_file(
        &pool,
        "default",
        "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        None,
        1024,
        "sha256",
    )
    .await
    .unwrap();
    upsert_file(
        &pool,
        "default",
        "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3",
        None,
        2048,
        "sha256",
    )
    .await
    .unwrap();

    let bytes = get_project_storage_bytes(&pool, "default").await.unwrap();
    assert_eq!(bytes, 3072);
}

#[tokio::test]
async fn test_project_isolation() {
    let pool = setup_test_pool().await;
    let hash = test_hash();

    upsert_file(&pool, "project1", hash, None, 1024, "sha256")
        .await
        .unwrap();
    upsert_file(&pool, "project2", hash, None, 2048, "sha256")
        .await
        .unwrap();

    let file1 = get_file(&pool, "project1", hash).await.unwrap().unwrap();
    let file2 = get_file(&pool, "project2", hash).await.unwrap().unwrap();

    assert_eq!(file1.size_bytes, 1024);
    assert_eq!(file2.size_bytes, 2048);

    // Storage should be separate
    assert_eq!(
        get_project_storage_bytes(&pool, "project1").await.unwrap(),
        1024
    );
    assert_eq!(
        get_project_storage_bytes(&pool, "project2").await.unwrap(),
        2048
    );
}

/// A claimed file cannot be associated, so its bytes cannot be deleted out from under a new row.
///
/// The interleaving a conditional delete alone leaves open: the row is deleted, ingestion recreates
/// it, associates and finalises the bytes, and cleanup then deletes those bytes - leaving a
/// committed reference to nothing. Refusing association through the claim is what closes it; the
/// batch fails and its retry finds the file either gone, and writes it again, or released.
#[tokio::test]
async fn a_claimed_file_refuses_association() {
    let pool = setup_test_pool().await;
    associate_file(
        &pool,
        "old-trace",
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await
    .unwrap();
    delete_trace_files(&pool, "default", &["old-trace".to_string()])
        .await
        .unwrap();

    assert!(
        claim_file_for_deletion(&pool, "default", test_hash())
            .await
            .unwrap(),
        "nothing references it, so it can be claimed"
    );
    assert!(
        !claim_file_for_deletion(&pool, "default", test_hash())
            .await
            .unwrap(),
        "and a second cleanup cannot claim it as well"
    );

    // Ingestion must not be able to reference bytes that are being deleted.
    let associated = associate_file(
        &pool,
        "new-trace",
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await;
    assert!(
        associated.is_err(),
        "associating with a claimed file must fail so the batch is refused"
    );

    // Released, it is available again.
    release_deletion_claim(&pool, "default", test_hash())
        .await
        .unwrap();
    assert!(
        associate_file(
            &pool,
            "new-trace",
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .is_ok(),
        "once the claim is released the file can be referenced again"
    );
}

/// An abandoned claim is findable, so a crash mid-deletion does not strand the file forever.
///
/// The claim is durable by design - that is the whole point of a fence - so nothing releases it if the
/// process dies holding it. Then every sweep sees a zero-reference row it cannot claim and skips it,
/// and every ingestion naming that file fails its batch. It has to be discoverable by age.
#[tokio::test]
async fn an_abandoned_claim_is_found_by_age_and_a_fresh_one_is_not() {
    let pool = setup_test_pool().await;
    associate_file(
        &pool,
        "old-trace",
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await
    .unwrap();
    delete_trace_files(&pool, "default", &["old-trace".to_string()])
        .await
        .unwrap();
    assert!(
        claim_file_for_deletion(&pool, "default", test_hash())
            .await
            .unwrap()
    );

    assert!(
        get_stale_claimed_files(&pool, 900)
            .await
            .unwrap()
            .is_empty(),
        "a claim taken a moment ago is a deletion in progress, not an abandoned one"
    );

    // Age it past the threshold rather than sleeping.
    sqlx::query("UPDATE files SET deleting_at = deleting_at - 1000 WHERE file_hash = ?")
        .bind(test_hash())
        .execute(&pool)
        .await
        .unwrap();
    let stale = get_stale_claimed_files(&pool, 900).await.unwrap();
    assert_eq!(
        stale
            .iter()
            .map(|(p, h, _)| (p.clone(), h.clone()))
            .collect::<Vec<_>>(),
        vec![("default".to_string(), test_hash().to_string())],
        "an old claim is reported so the sweep can finish what the crash left"
    );
    let observed = stale[0].2;

    // Re-taking it needs the value that was read. A claim that moved on - because another worker took
    // it over, or because the row was released and re-associated - refuses, which is what stops a stale
    // reading from deleting bytes a committed span references.
    assert!(
        !reclaim_stale_file(&pool, "default", test_hash(), observed + 1)
            .await
            .unwrap(),
        "a claim that is not the one observed must not be re-taken"
    );
    assert!(
        reclaim_stale_file(&pool, "default", test_hash(), observed)
            .await
            .unwrap(),
        "the claim as observed is re-taken"
    );
    assert!(
        !reclaim_stale_file(&pool, "default", test_hash(), observed)
            .await
            .unwrap(),
        "and the reclaim refreshed it, so the same reading cannot be acted on twice"
    );

    // A stale snapshot cannot survive another worker releasing the claim and ingestion re-associating the
    // same content hash.
    {
        let stale = get_stale_claimed_files(&pool, 0).await.unwrap();
        let observed = stale[0].2;

        release_deletion_claim(&pool, "default", test_hash())
            .await
            .expect("release the claim");
        associate_file(
            &pool,
            "trace-again",
            "default",
            test_hash(),
            Some("image/png"),
            1,
            "sha256",
        )
        .await
        .expect("ingestion re-associates the same content");

        assert!(
            !reclaim_stale_file(&pool, "default", test_hash(), observed)
                .await
                .unwrap(),
            "the claim is gone and the file is referenced again, so its bytes must not be deleted"
        );

        // Cleaned up so the rest of the test sees the state it expects, and re-claimed the ordinary way.
        delete_trace_files(&pool, "default", &["trace-again".to_string()])
            .await
            .expect("release the association");
        assert!(
            claim_file_for_deletion(&pool, "default", test_hash())
                .await
                .unwrap(),
            "with nothing referencing it the file is claimable again"
        );
    }

    // And finishing it is exactly the normal path: the row goes, nothing is left claimed.
    assert!(
        delete_file_if_unreferenced(&pool, "default", test_hash())
            .await
            .unwrap()
    );
    assert!(
        get_stale_claimed_files(&pool, 900)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A file re-associated between the count and the delete must survive.
///
/// The interleaving a count-then-delete cannot survive: cleanup recomputes zero, ingestion
/// associates a new trace, and the delete fires anyway - taking the bytes from under a span that was
/// just committed. With the condition inside the delete, the association makes it match nothing.
#[tokio::test]
async fn a_file_referenced_again_before_deletion_survives() {
    let pool = setup_test_pool().await;
    associate_file(
        &pool,
        "old-trace",
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await
    .unwrap();

    // Cleanup: associations for the doomed trace are gone, and the count reads zero.
    delete_trace_files(&pool, "default", &["old-trace".to_string()])
        .await
        .unwrap();
    assert_eq!(
        sync_ref_count(&pool, "default", test_hash()).await.unwrap(),
        Some(0)
    );

    // Ingestion gets in first, associating a new trace with the same content.
    associate_file(
        &pool,
        "new-trace",
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await
    .unwrap();

    // The delete must now refuse.
    let deleted = delete_file_if_unreferenced(&pool, "default", test_hash())
        .await
        .unwrap();
    assert!(
        !deleted,
        "the file is referenced by new-trace, so it must not be deleted"
    );
    assert!(
        get_file(&pool, "default", test_hash())
            .await
            .unwrap()
            .is_some(),
        "and its metadata must still be there"
    );
}

/// Two cleanups deleting overlapping trace sets must not release a reference twice.
///
/// The case a maintained counter cannot survive: four traces reference one file, two cleanups both
/// read a count of three for the same three traces, and both subtract three - taking the count to
/// zero and deleting a file the fourth trace still shows. Derived from the associations, the count
/// is simply what remains, however many times it is recomputed.
#[tokio::test]
async fn recomputing_a_reference_count_is_idempotent() {
    let pool = setup_test_pool().await;
    let all = ["t1", "t2", "t3", "t4"];
    for trace in all {
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

    // Both cleanups target the same three traces; only one deletion actually removes rows.
    let doomed: Vec<String> = ["t1", "t2", "t3"].iter().map(|t| t.to_string()).collect();
    delete_trace_files(&pool, "default", &doomed).await.unwrap();

    // Two recomputations, as two concurrent cleanups would each do.
    let first = sync_ref_count(&pool, "default", test_hash()).await.unwrap();
    let second = sync_ref_count(&pool, "default", test_hash()).await.unwrap();

    assert_eq!(first, Some(1), "t4 still references the file");
    assert_eq!(
        second, first,
        "recomputing again must not release t4's reference a second time"
    );
}

/// Two projects can present the same trace id, and their associations must not collide.
///
/// A trace id comes from the client. Keyed without the project, the first project's association
/// satisfied `INSERT OR IGNORE` for the second - so the second got no association, `associate_file`
/// reported "not new" and skipped the increment, and the second project's file was left with a
/// reference count nothing would ever release.
#[tokio::test]
async fn two_projects_sharing_a_trace_id_each_get_their_own_association() {
    let pool = setup_test_pool().await;

    for project in ["project-a", "project-b"] {
        let inserted = associate_file(
            &pool,
            "same-trace-id",
            project,
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        assert!(
            inserted,
            "{project} must get its own association for a trace id it happens to share"
        );
        let file = get_file(&pool, project, test_hash())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(file.ref_count, 1, "{project} holds exactly one reference");
    }
}

/// A retry must not count the same reference twice.
///
/// With the increment separate from the association, `INSERT OR IGNORE` kept the existing
/// association while the increment ran again - so a redelivered batch inflated the count and the
/// file became uncollectable after its traces were gone.
#[tokio::test]
async fn associating_the_same_file_twice_counts_it_once() {
    let pool = setup_test_pool().await;

    let first = associate_file(
        &pool,
        "trace-a",
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await
    .unwrap();
    let second = associate_file(
        &pool,
        "trace-a",
        "default",
        test_hash(),
        Some("image/png"),
        1024,
        "sha256",
    )
    .await
    .unwrap();

    // Both record a reference now, because each is a distinct in-flight writer that must be resolved by
    // its own confirm or release. What must *not* change is the reference count: the same trace naming
    // the same file is one reference however many batches carry it.
    assert!(first, "the first reference is recorded");
    assert!(
        second,
        "the second reference is recorded too - it is a separate writer to resolve"
    );

    let file = get_file(&pool, "default", test_hash())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        file.ref_count, 1,
        "one association means one reference, however many batches carry it"
    );
    let pending: i64 = sqlx::query_scalar(
            "SELECT pending_writers FROM trace_files WHERE project_id = ? AND trace_id = ? AND file_hash = ?",
        )
        .bind("default")
        .bind("trace-a")
        .bind(test_hash())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        pending, 2,
        "each carrying batch is one in-flight writer to resolve"
    );
}
