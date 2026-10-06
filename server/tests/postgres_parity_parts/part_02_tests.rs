/// Files, their references and their deletion fence - the machinery a dangling reference comes from.
#[tokio::test]
async fn files_and_references_behave_identically() {
    assert_parity("files", |repo, mut t| async move {
        let a = hash(0xa1);
        let b = hash(0xb2);

        // Two traces referencing one file, and one referencing another.
        repo.associate_file(
            "trace-1",
            &ProjectId::from("default"),
            &a,
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        repo.associate_file(
            "trace-2",
            &ProjectId::from("default"),
            &a,
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        repo.associate_file(
            "trace-2",
            &ProjectId::from("default"),
            &b,
            None,
            64,
            "sha256",
        )
        .await
        .unwrap();
        // Idempotent: the same trace naming the same file twice is one reference.
        repo.associate_file(
            "trace-1",
            &ProjectId::from("default"),
            &a,
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();

        let file_a = repo
            .get_file(&ProjectId::from("default"), &a)
            .await
            .unwrap();
        t.note(&format!(
            "a_ref_count={:?} size={:?} media={:?}",
            file_a.as_ref().map(|f| f.ref_count),
            file_a.as_ref().map(|f| f.size_bytes),
            file_a.as_ref().and_then(|f| f.media_type.clone())
        ));
        t.note(&format!(
            "exists_a={} exists_missing={}",
            repo.file_exists(&ProjectId::from("default"), &a)
                .await
                .unwrap(),
            repo.file_exists(&ProjectId::from("default"), &hash(0xcc))
                .await
                .unwrap()
        ));
        t.note(&format!(
            "storage_bytes={}",
            repo.get_project_storage_bytes(&ProjectId::from("default"))
                .await
                .unwrap()
        ));

        let mut hashes = repo
            .get_file_hashes_for_traces(&ProjectId::from("default"), &["trace-2".to_string()])
            .await
            .unwrap();
        hashes.sort();
        t.note(&format!("hashes_for_trace_2={}", hashes.len()));
        t.note(&format!(
            "restore_candidates_first={:?}",
            repo.restore_association_trace_ids(&ProjectId::from("default"), None, 1)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "restore_candidates_after={:?}",
            repo.restore_association_trace_ids(&ProjectId::from("default"), Some("trace-1"), 10,)
                .await
                .unwrap()
        ));
        let mut counted = repo
            .get_file_reference_counts_for_traces(
                &ProjectId::from("default"),
                &["trace-1".to_string()],
            )
            .await
            .unwrap();
        counted.sort();
        t.note(&format!(
            "counts_for_trace_1={:?}",
            counted.iter().map(|(_, n)| *n).collect::<Vec<_>>()
        ));

        // Deleting one trace releases only its own references.
        t.note(&format!(
            // Sorted: the set is what matters, and the two dialects return rows in their own order.
            "released={:?}",
            {
                let mut released = repo
                    .delete_trace_files(&ProjectId::from("default"), &["trace-1".to_string()])
                    .await
                    .unwrap();
                released.sort();
                released
            }
        ));
        t.note(&format!(
            "a_after_release={:?}",
            repo.get_file(&ProjectId::from("default"), &a)
                .await
                .unwrap()
                .map(|f| f.ref_count)
        ));

        // The fence: claiming, refusing an association through it, releasing.
        t.note(&format!(
            "claim_referenced={}",
            repo.claim_file_for_deletion(&ProjectId::from("default"), &a)
                .await
                .unwrap()
        ));
        repo.delete_trace_files(&ProjectId::from("default"), &["trace-2".to_string()])
            .await
            .unwrap();
        t.note(&format!(
            "claim_unreferenced={}",
            repo.claim_file_for_deletion(&ProjectId::from("default"), &a)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "claim_again={}",
            repo.claim_file_for_deletion(&ProjectId::from("default"), &a)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "associate_through_fence_is_err={}",
            repo.associate_file(
                "trace-3",
                &ProjectId::from("default"),
                &a,
                None,
                1024,
                "sha256"
            )
            .await
            .is_err()
        ));
        t.note(&format!(
            "stale_at_zero={}",
            repo.get_stale_claimed_files(0).await.unwrap().len()
        ));
        t.note(&format!(
            "stale_at_a_day={}",
            repo.get_stale_claimed_files(86_400).await.unwrap().len()
        ));
        // The compare-and-set the recovery path gates its byte deletion on. Both backends must answer the
        // same way: the observed claim is re-takeable exactly once, and a different value never is.
        let observed = repo
            .get_stale_claimed_files(0)
            .await
            .unwrap()
            .into_iter()
            .find(|(_, hash, _)| hash == &a)
            .map(|(_, _, deleting_at)| deleting_at)
            .expect("the claimed file is reported");
        t.note(&format!(
            "reclaim_with_a_different_value={}",
            repo.reclaim_stale_file(&ProjectId::from("default"), &a, observed + 1)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "reclaim_as_observed={}",
            repo.reclaim_stale_file(&ProjectId::from("default"), &a, observed)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "reclaim_twice_on_one_reading={}",
            repo.reclaim_stale_file(&ProjectId::from("default"), &a, observed)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "delete_if_unreferenced={}",
            repo.delete_file_if_unreferenced(&ProjectId::from("default"), &a)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "gone={}",
            repo.get_file(&ProjectId::from("default"), &a)
                .await
                .unwrap()
                .is_none()
        ));

        // The release path, on the file that is still there.
        t.note(&format!(
            "claim_b={}",
            repo.claim_file_for_deletion(&ProjectId::from("default"), &b)
                .await
                .unwrap()
        ));
        repo.release_deletion_claim(&ProjectId::from("default"), &b)
            .await
            .unwrap();
        t.note(&format!(
            "associate_after_release_ok={}",
            repo.associate_file(
                "trace-4",
                &ProjectId::from("default"),
                &b,
                None,
                64,
                "sha256"
            )
            .await
            .is_ok()
        ));

        // A count that drifted, recomputed from the associations that exist.
        repo.decrement_ref_count(&ProjectId::from("default"), &b)
            .await
            .unwrap();
        repo.decrement_ref_count(&ProjectId::from("default"), &b)
            .await
            .unwrap();
        t.note(&format!(
            "b_after_two_decrements={:?}",
            repo.get_file(&ProjectId::from("default"), &b)
                .await
                .unwrap()
                .map(|f| f.ref_count)
        ));
        t.note(&format!(
            "synced={:?}",
            repo.sync_ref_count(&ProjectId::from("default"), &b)
                .await
                .unwrap()
        ));
        t.note(&format!(
            "b_after_sync={:?}",
            repo.get_file(&ProjectId::from("default"), &b)
                .await
                .unwrap()
                .map(|f| f.ref_count)
        ));
        let mut orphans = repo.get_orphan_files().await.unwrap();
        orphans.sort();
        t.note(&format!("orphans={}", orphans.len()));
        t
    })
    .await;
}

/// An organization and its members: roles, the atomic updates, and what deleting it reaches.
#[tokio::test]
async fn organizations_and_members_behave_identically() {
    assert_parity("orgs", |repo, mut t| async move {
        let owner = repo
            .create_user("owner@example.com", Some("Owner"))
            .await
            .expect("create owner");
        let member = repo
            .create_user("member@example.com", Some("Member"))
            .await
            .expect("create member");
        t.note_id("owner", &owner.id);
        t.note_id("member", &member.id);

        let org = repo
            .create_organization_with_owner("Acme", "acme", &owner.id)
            .await
            .expect("create org");
        t.note_id("org", &org.id);
        let project = repo.create_project(&org.id, "Acme Project").await.unwrap();
        t.note_id("project", &project.id);

        repo.add_member(&org.id, &member.id, "member")
            .await
            .unwrap();
        let (mut members, total) = repo.list_members(&org.id, 1, 50).await.unwrap();
        members.sort_by(|a, b| a.role.cmp(&b.role));
        t.note(&format!("members={} total={total}", members.len()));
        for m in &members {
            t.note(&format!("member_role={}", m.role));
        }
        t.note(&format!(
            "membership_role={:?}",
            repo.get_membership(&org.id, &member.id)
                .await
                .unwrap()
                .map(|m| m.role)
        ));

        // Atomic role change and removal: the last owner must not be demoted or removed.
        fn describe<T>(outcome: &LastOwnerResult<T>) -> &'static str {
            match outcome {
                LastOwnerResult::Success(_) => "success",
                LastOwnerResult::LastOwner => "last_owner",
                LastOwnerResult::NotFound => "not_found",
            }
        }
        t.note(&format!(
            "promote={}",
            describe(
                &repo
                    .update_role_atomic(&org.id, &member.id, "admin")
                    .await
                    .unwrap()
            )
        ));
        t.note(&format!(
            "demote_last_owner={}",
            describe(
                &repo
                    .update_role_atomic(&org.id, &owner.id, "member")
                    .await
                    .unwrap()
            )
        ));
        t.note(&format!(
            "remove_last_owner={}",
            describe(&repo.remove_member_atomic(&org.id, &owner.id).await.unwrap())
        ));
        t.note(&format!(
            "remove_member={}",
            describe(
                &repo
                    .remove_member_atomic(&org.id, &member.id)
                    .await
                    .unwrap()
            )
        ));

        let mut ids = repo.list_project_ids(&org.id).await.unwrap();
        ids.sort();
        t.note(&format!("project_ids={}", ids.len()));

        let (for_user, _) = repo.list_projects_for_user(&owner.id, 1, 50).await.unwrap();
        t.note(&format!("projects_for_owner={}", for_user.len()));

        // Deleting the org must take its projects with it, in both schemas.
        t.note(&format!(
            "org_deleted={}",
            repo.delete_organization(&org.id).await.unwrap()
        ));
        t.note(&format!(
            "project_gone={}",
            repo.get_project(&project.id).await.unwrap().is_none()
        ));
        t.note(&format!(
            "membership_gone={}",
            repo.get_membership(&org.id, &owner.id)
                .await
                .unwrap()
                .is_none()
        ));
        t
    })
    .await;
}

// ============================================================================
// Concurrency: what SQLite cannot express
// ============================================================================

/// The file fence must survive two writers, at the one interleaving that breaks it.
///
/// Launching two tasks and hoping proves nothing - they serialise, and the test passes against the very
/// code it is meant to catch (verified: it did). So the interleaving is pinned by holding a transaction
/// open at the point where the other writer must be blocked.
///
/// The hazard is specific to PostgreSQL under READ COMMITTED. An `UPDATE` that blocks on a locked row
/// re-checks its qualification when the lock frees - but a subquery in that qualification is evaluated
/// against the *statement's original* snapshot. So a claim written as one `UPDATE ... WHERE NOT EXISTS
/// (SELECT 1 FROM trace_files ...)` cannot see the association that committed while it waited: it claims
/// a file that is now referenced, and cleanup deletes bytes a committed row points at. The span then
/// renders as broken content and nothing on it says why.
///
/// SQLite cannot express this at all - one writer, and a read-then-write transaction fails with a busy
/// error - which is why the defect lived in the PostgreSQL half alone.
#[tokio::test]
async fn the_file_fence_holds_against_a_concurrent_association() {
    let Some((_, postgres)) = pair().await else {
        return;
    };
    let file = hash(0x5c);

    // A file that exists and is unreferenced: claimable, and associable.
    postgres
        .associate_file(
            "old-trace",
            &ProjectId::from("default"),
            &file,
            None,
            128,
            "sha256",
        )
        .await
        .unwrap();
    postgres
        .delete_trace_files(&ProjectId::from("default"), &["old-trace".to_string()])
        .await
        .unwrap();

    // Writer one: an association, up to the point of committing. These statements mirror
    // `associate_file`'s order - lock the file row, then insert the reference - because what is being
    // tested is the fence's behaviour against *a reference committed while the claim waited*, whatever
    // the code that commits it looks like.
    let mut association = postgres.pool().begin().await.unwrap();
    let _: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT deleting_at FROM files WHERE project_id = $1 AND file_hash = $2 FOR UPDATE",
    )
    .bind("default")
    .bind(&file)
    .fetch_optional(&mut *association)
    .await
    .unwrap();
    sqlx::query("INSERT INTO trace_files (trace_id, project_id, file_hash) VALUES ($1, $2, $3)")
        .bind("new-trace")
        .bind("default")
        .bind(&file)
        .execute(&mut *association)
        .await
        .unwrap();

    // Writer two: the claim. It must block on the row lock writer one holds.
    let claim = {
        let repo = postgres.clone();
        let file = file.clone();
        tokio::spawn(async move {
            repo.claim_file_for_deletion(&ProjectId::from("default"), &file)
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !claim.is_finished(),
        "the claim answered without waiting for the row lock, so this test is not exercising the \
         interleaving it exists for"
    );

    // The reference becomes visible. The claim's wait ends here.
    association.commit().await.unwrap();
    let claimed = claim.await.unwrap().unwrap();

    assert!(
        !claimed,
        "a file with a committed reference was claimed for deletion, so cleanup is about to delete \
         bytes that a span still points at"
    );
    assert_eq!(
        postgres
            .sync_ref_count(&ProjectId::from("default"), &file)
            .await
            .unwrap(),
        Some(1),
        "and the reference that won is the one counted"
    );
}

/// A project cannot be created under an organization whose deletion has committed.
///
/// PostgreSQL-specific, and it is why the check is a locking read rather than a `WHERE` clause on the
/// insert. `INSERT ... SELECT ... WHERE deleting_at IS NULL` reads under its own snapshot, and the claim
/// updates only a non-key column - so its row lock is `FOR NO KEY UPDATE`, which is *compatible* with the
/// key-share lock a foreign-key insert takes. The insert would neither block nor see the tombstone, and a
/// brand-new live project would appear under an organization whose cleanup had already listed its
/// projects: the caller gets 201 and then 404, and an ingest that passed the project fence in between
/// leaves data with no row to find it by.
///
/// The interleaving is pinned rather than hoped for: the creation is started while a transaction holds the
/// organization locked, so it must block, and the tombstone commits before it is released.
#[tokio::test]
async fn a_project_cannot_be_created_under_a_deleting_organization() {
    let Some((_, postgres)) = pair().await else {
        return;
    };

    // Writer one: take the organization's row lock, as the claim does, and hold it.
    let mut claim = postgres.pool().begin().await.unwrap();
    let _: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM organizations WHERE id = $1 FOR UPDATE")
            .bind("default")
            .fetch_optional(&mut *claim)
            .await
            .unwrap();
    sqlx::query("UPDATE organizations SET deleting_at = $1 WHERE id = $2")
        .bind(chrono::Utc::now().timestamp())
        .bind("default")
        .execute(&mut *claim)
        .await
        .unwrap();

    // Writer two: a creation that must wait for that lock.
    let creation = {
        let repo = postgres.clone();
        tokio::spawn(async move { repo.create_project("default", "Sneaky").await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !creation.is_finished(),
        "the creation answered without waiting for the organization's row, so it is reading a snapshot \
         and this test is not exercising the interleaving it exists for"
    );

    claim.commit().await.unwrap();
    let outcome = creation.await.unwrap();
    assert!(
        outcome.is_err(),
        "a project was created under an organization whose deletion had committed; it would be live, \
         accept writes, and be cascaded away without leaving a deletion record"
    );
}

/// A member cannot be added while the organization's deletion commits underneath.
///
/// The sequential case - mutating an already-tombstoned organization - was fixed first, and it is the
/// easier half. This is the race: the mutation reads a live organization, the deletion commits, and the
/// mutation writes anyway, because the parent row still exists and the foreign key is satisfied. The caller
/// is told success for a membership in an organization no read can see and the cascade is about to remove.
///
/// The interleaving is pinned rather than hoped for, as the project-creation test does it: the mutation is
/// started while a transaction holds the organization's row locked, so it must block, and the tombstone
/// commits before the lock is released.
#[tokio::test]
async fn a_member_cannot_be_added_while_the_organization_is_being_deleted() {
    let Some((_, postgres)) = pair().await else {
        return;
    };

    let mut claim = postgres.pool().begin().await.unwrap();
    let _: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM organizations WHERE id = $1 FOR UPDATE")
            .bind("default")
            .fetch_optional(&mut *claim)
            .await
            .unwrap();
    sqlx::query("UPDATE organizations SET deleting_at = $1 WHERE id = $2")
        .bind(chrono::Utc::now().timestamp())
        .bind("default")
        .execute(&mut *claim)
        .await
        .unwrap();

    let user = postgres
        .create_user("joiner@example.com", Some("Joiner"))
        .await
        .expect("create user");
    let addition = {
        let repo = postgres.clone();
        let user_id = user.id.clone();
        tokio::spawn(async move { repo.add_member("default", &user_id, "member").await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !addition.is_finished(),
        "the mutation answered without waiting for the organization's row, so its liveness check is \
         outside its transaction and this test is not exercising the interleaving it exists for"
    );

    claim.commit().await.unwrap();
    assert!(
        addition.await.unwrap().is_err(),
        "a member was added to an organization whose deletion had committed"
    );
}

/// Two replicas sweeping at once cannot claim the same deleted project.
///
/// `WHERE project_id IN (SELECT ... LIMIT n)` is not enough on PostgreSQL, and the mechanism is the same
/// stale-subquery one as the file claim's: the subquery is evaluated against the statement's snapshot, so a
/// replica whose outer update blocks on a row another replica is updating resumes with a subquery result
/// that still lists it - and the outer condition only compares `project_id`, which has not changed. Both
/// replicas return the same id and both do the storage work, which for fifty S3 listings is exactly the
/// cost this scheduler exists to bound. `FOR UPDATE SKIP LOCKED` on the inner select is what makes the
/// claim exclusive.
///
/// The interleaving is pinned: a transaction holds the row locked while a second claim runs, so a claim
/// that respects the lock returns nothing and one that reads a stale snapshot returns the id.
#[tokio::test]
async fn two_replicas_cannot_claim_the_same_deleted_project() {
    let Some((_, postgres)) = pair().await else {
        return;
    };

    let project = postgres.create_project("default", "Swept").await.unwrap();
    postgres
        .claim_project_for_deletion(&project.id)
        .await
        .unwrap();
    postgres
        .record_project_sweep(&project.id, true, 1, 0)
        .await
        .unwrap();

    // Replica A: claim the row and hold it, as a sweep in progress does.
    let mut replica_a = postgres.pool().begin().await.unwrap();
    let held: Vec<(String,)> = sqlx::query_as(
        "UPDATE deleted_projects SET last_checked_at = extract(epoch from now())::bigint          WHERE project_id IN (              SELECT project_id FROM deleted_projects              WHERE next_check_at IS NULL OR next_check_at <= extract(epoch from now())::bigint              ORDER BY next_check_at LIMIT 10 FOR UPDATE SKIP LOCKED          ) RETURNING project_id",
    )
    .fetch_all(&mut *replica_a)
    .await
    .unwrap();
    assert_eq!(held.len(), 1, "replica A claims the only due id");

    // Replica B: the same claim, concurrently. It must find nothing rather than the same id.
    let claimed_by_b = postgres
        .claim_deleted_projects_for_check(600, 10)
        .await
        .unwrap();
    assert!(
        claimed_by_b.is_empty(),
        "two replicas claimed the same deleted project, so both will do its storage cleanup: {:?}",
        claimed_by_b
    );

    replica_a.commit().await.unwrap();
}

/// Two concurrent deletions of one project: exactly one owns it.
///
/// The claim is what makes a project's cleanup single-owner, and a cleanup that runs twice deletes
/// analytics rows and file bytes twice over while both callers report success to their user.
#[tokio::test]
async fn only_one_concurrent_project_claim_wins() {
    let Some((_, postgres)) = pair().await else {
        return;
    };
    let project = postgres
        .create_project("default", "Contested")
        .await
        .unwrap();

    let mut winners = 0;
    let mut handles = Vec::new();
    for _ in 0..4 {
        let repo = postgres.clone();
        let id = project.id.clone();
        handles.push(tokio::spawn(async move {
            repo.claim_project_for_deletion(&id).await
        }));
    }
    for handle in handles {
        if handle.await.unwrap().unwrap_or(false) {
            winners += 1;
        }
    }
    assert_eq!(
        winners, 1,
        "exactly one caller must own the deletion; {winners} did"
    );
}

/// A deleted trace stays deleted, even against an ingest that commits afterwards.
///
/// The race the tombstone closes: files and their associations are written *before* the analytics row
/// that references them, so a batch in flight when `delete_traces` runs commits after the deletion has
/// reclaimed the bytes - producing a span with a `#!B64!#` reference to nothing, for a trace the caller
/// was already told 204 for. Nothing bounds how late that commit is; a queued batch can be redelivered
/// minutes later.
///
/// Compared as a transcript because the two implementations are hand-written in different dialects (a
/// per-row loop against `UNNEST`, `IN (?, ?)` against `= ANY($2)`), and a disagreement here is either a
/// deleted trace resurrected on one backend or a live one dropped on the other.
#[tokio::test]
async fn trace_deletion_tombstones_behave_identically() {
    assert_parity("deleted_traces", |repo, mut t| async move {
        let asked = vec!["trace-a".to_string(), "trace-b".to_string()];
        // Sorted, because the answer is a set and the two dialects return rows in their own order.
        let show = |t: &mut Transcript,
                    what: &str,
                    found: Result<
            std::collections::HashSet<String>,
            sideseat_ports::error::DataError,
        >| {
            match found {
                Ok(set) => {
                    let mut ids: Vec<String> = set.into_iter().collect();
                    ids.sort();
                    t.note(&format!("{what}={ids:?}"));
                }
                Err(e) => t.note(&format!("{what}=error({e})")),
            }
        };

        // Nothing deleted yet.
        let found = repo
            .deleted_traces_among(&ProjectId::from("p1"), &asked)
            .await;
        show(&mut t, "empty", found);

        // One deleted, and only that one refused.
        let recorded = repo
            .record_deleted_traces(&ProjectId::from("p1"), &["trace-a".to_string()])
            .await;
        t.note(&format!("record a ok={}", recorded.is_ok()));
        let found = repo
            .deleted_traces_among(&ProjectId::from("p1"), &asked)
            .await;
        show(&mut t, "after a", found);

        // The deletion route may be retried, so re-recording must not conflict.
        let again = repo
            .record_deleted_traces(&ProjectId::from("p1"), &["trace-a".to_string()])
            .await;
        t.note(&format!("record a again ok={}", again.is_ok()));
        let found = repo
            .deleted_traces_among(&ProjectId::from("p1"), &asked)
            .await;
        show(&mut t, "after retry", found);

        // A trace id comes from the client, so the same id in another project is untouched.
        let found = repo
            .deleted_traces_among(&ProjectId::from("p2"), &asked)
            .await;
        show(&mut t, "other project", found);

        // A batch of several, and an empty ask.
        let both = repo
            .record_deleted_traces(&ProjectId::from("p1"), &asked)
            .await;
        t.note(&format!("record both ok={}", both.is_ok()));
        let found = repo
            .deleted_traces_among(&ProjectId::from("p1"), &asked)
            .await;
        show(&mut t, "after both", found);
        let found = repo.deleted_traces_among(&ProjectId::from("p1"), &[]).await;
        show(&mut t, "empty ask", found);
        t
    })
    .await;
}

/// Releasing an association drops its file's reference count, so the orphan sweeper can reclaim it.
///
/// The leak this closes: a file's bytes and association are written *before* the analytics row that
/// references them - deliberately, so the surviving failure is a reclaimable orphan rather than a
/// dangling reference. But an association keeps `ref_count` above zero, and the orphan sweeper selects
/// on `ref_count = 0`, so a batch whose analytics write failed left the file holding the project's quota
/// with nothing able to reclaim it.
///
/// Two properties, both compared across the dialects: releasing the association this batch created drops
/// the count to zero and makes the file an orphan, and releasing does **not** touch another trace's
/// association for the same file - which is what stops the compensation from orphaning a committed
/// batch's content.
#[tokio::test]
async fn releasing_a_created_association_behaves_identically() {
    assert_parity("release association", |repo, mut t| async move {
        let project = "default";
        let file_hash = hash(0xc3);

        // Two traces share one file, which is the case that makes precision matter.
        for trace in ["t-keep", "t-drop"] {
            let created = repo
                .associate_file(
                    trace,
                    &ProjectId::from(project),
                    &file_hash,
                    Some("image/png"),
                    10,
                    "sha256",
                )
                .await
                .expect("associate");
            t.note(&format!("associate {trace} new={created}"));
            repo.sync_ref_count(&ProjectId::from(project), &file_hash)
                .await
                .expect("sync");
        }
        let counts = repo
            .get_file(&ProjectId::from(project), &file_hash)
            .await
            .expect("file row");
        t.note(&format!(
            "refs after two associations={:?}",
            counts.map(|f| f.ref_count)
        ));

        // Release only the one the failed batch created.
        let released = repo
            .release_trace_file_association(&ProjectId::from(project), "t-drop", &file_hash)
            .await
            .expect("release");
        repo.sync_ref_count(&ProjectId::from(project), &file_hash)
            .await
            .expect("sync");
        t.note(&format!("released={released}"));
        let counts = repo
            .get_file(&ProjectId::from(project), &file_hash)
            .await
            .expect("file row");
        t.note(&format!(
            "refs after release={:?}",
            counts.map(|f| f.ref_count)
        ));

        // The other trace still holds it, so it is not an orphan yet.
        let orphans = repo.get_orphan_files().await.expect("orphans");
        t.note(&format!(
            "orphan while still referenced={}",
            orphans.iter().any(|(p, h)| p == project && h == &file_hash)
        ));

        // Release the survivor too, and now it is reclaimable.
        repo.release_trace_file_association(&ProjectId::from(project), "t-keep", &file_hash)
            .await
            .expect("release the other");
        repo.sync_ref_count(&ProjectId::from(project), &file_hash)
            .await
            .expect("sync");
        let orphans = repo.get_orphan_files().await.expect("orphans");
        t.note(&format!(
            "orphan once unreferenced={}",
            orphans.iter().any(|(p, h)| p == project && h == &file_hash)
        ));

        // Releasing something that is not there is not an error - the failure path may run twice.
        let again = repo
            .release_trace_file_association(&ProjectId::from(project), "t-drop", &file_hash)
            .await
            .expect("releasing twice must not error");
        t.note(&format!("release again={again}"));
        t
    })
    .await;
}
