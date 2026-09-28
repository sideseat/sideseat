use super::*;

const TEST_NOW: i64 = 1_700_000_000;

async fn create_project(
    pool: &SqlitePool,
    organization_id: &str,
    name: &str,
) -> Result<ProjectRow, SqliteError> {
    super::create_project(pool, organization_id, name, TEST_NOW).await
}

async fn update_project(
    pool: &SqlitePool,
    id: &str,
    name: &str,
) -> Result<Option<ProjectRow>, SqliteError> {
    super::update_project(pool, id, name, TEST_NOW + 1).await
}

async fn claim_project_for_deletion(pool: &SqlitePool, id: &str) -> Result<bool, SqliteError> {
    super::claim_project_for_deletion(pool, id, TEST_NOW).await
}

async fn get_stale_claimed_projects(
    pool: &SqlitePool,
    older_than_secs: i64,
) -> Result<Vec<(String, i64)>, SqliteError> {
    super::get_stale_claimed_projects(pool, older_than_secs, TEST_NOW).await
}

async fn claim_organization_for_deletion(pool: &SqlitePool, id: &str) -> Result<bool, SqliteError> {
    super::claim_organization_for_deletion(pool, id, TEST_NOW).await
}

async fn get_stale_claimed_organizations(
    pool: &SqlitePool,
    older_than_secs: i64,
) -> Result<Vec<(String, i64)>, SqliteError> {
    super::get_stale_claimed_organizations(pool, older_than_secs, TEST_NOW).await
}

async fn reclaim_stale_project(
    pool: &SqlitePool,
    id: &str,
    observed_deleting_at: i64,
) -> Result<bool, SqliteError> {
    super::reclaim_stale_project(pool, id, observed_deleting_at, TEST_NOW + 1).await
}

async fn setup_test_pool() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::query(crate::schema::SCHEMA)
        .execute(&pool)
        .await
        .unwrap();
    pool
}

#[tokio::test]
async fn test_create_project() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Test Project")
        .await
        .unwrap();

    assert!(!project.id.is_empty());
    assert_eq!(project.organization_id, "default");
    assert_eq!(project.name, "Test Project");
    assert!(project.created_at > 0);
    assert_eq!(project.created_at, project.updated_at);
}

#[tokio::test]
async fn test_get_project() {
    let pool = setup_test_pool().await;
    let created = create_project(&pool, "default", "Test Project")
        .await
        .unwrap();

    let fetched = get_project(&pool, &created.id).await.unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.organization_id, "default");
    assert_eq!(fetched.name, "Test Project");
}

#[tokio::test]
async fn test_get_project_not_found() {
    let pool = setup_test_pool().await;
    let result = get_project(&pool, "nonexistent").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_list_projects() {
    let pool = setup_test_pool().await;

    // Default project should exist
    let (projects, total) = list_projects(&pool, 1, 10).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, "default");
    assert_eq!(projects[0].organization_id, "default");

    // Create more projects
    create_project(&pool, "default", "Project 1").await.unwrap();
    create_project(&pool, "default", "Project 2").await.unwrap();

    let (projects, total) = list_projects(&pool, 1, 10).await.unwrap();
    assert_eq!(total, 3);
    assert_eq!(projects.len(), 3);
}

#[tokio::test]
async fn test_list_for_user() {
    let pool = setup_test_pool().await;

    // Local user should see default project
    let (projects, total) = list_for_user(&pool, "local", 1, 10).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, "default");

    // Non-member should see nothing
    let (projects, total) = list_for_user(&pool, "nonexistent", 1, 10).await.unwrap();
    assert_eq!(total, 0);
    assert_eq!(projects.len(), 0);
}

#[tokio::test]
async fn test_list_for_org() {
    let pool = setup_test_pool().await;

    // Default org has default project
    let (projects, total) = list_for_org(&pool, "default", 1, 10).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(projects.len(), 1);

    // Create another project in default org
    create_project(&pool, "default", "Project 1").await.unwrap();

    let (projects, total) = list_for_org(&pool, "default", 1, 10).await.unwrap();
    assert_eq!(total, 2);
    assert_eq!(projects.len(), 2);

    // Non-existent org has no projects
    let (projects, total) = list_for_org(&pool, "nonexistent", 1, 10).await.unwrap();
    assert_eq!(total, 0);
    assert_eq!(projects.len(), 0);
}

#[tokio::test]
async fn test_list_projects_pagination() {
    let pool = setup_test_pool().await;

    for i in 1..=5 {
        create_project(&pool, "default", &format!("Project {}", i))
            .await
            .unwrap();
    }

    // Page 1 with limit 2
    let (projects, total) = list_projects(&pool, 1, 2).await.unwrap();
    assert_eq!(total, 6); // 5 + default
    assert_eq!(projects.len(), 2);

    // Page 2 with limit 2
    let (projects, _) = list_projects(&pool, 2, 2).await.unwrap();
    assert_eq!(projects.len(), 2);

    // Page 3 with limit 2
    let (projects, _) = list_projects(&pool, 3, 2).await.unwrap();
    assert_eq!(projects.len(), 2);

    // Page 4 with limit 2 (no more results)
    let (projects, _) = list_projects(&pool, 4, 2).await.unwrap();
    assert_eq!(projects.len(), 0);
}

#[tokio::test]
async fn test_update_project() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Original Name")
        .await
        .unwrap();

    let updated = update_project(&pool, &project.id, "Updated Name")
        .await
        .unwrap();
    assert!(updated.is_some());
    let updated = updated.unwrap();
    assert_eq!(updated.name, "Updated Name");
    assert_eq!(updated.organization_id, "default"); // org unchanged
}

#[tokio::test]
async fn test_delete_project() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "To Delete").await.unwrap();

    let deleted = delete_project(&pool, &project.id).await.unwrap();
    assert!(deleted);

    let fetched = get_project(&pool, &project.id).await.unwrap();
    assert!(fetched.is_none());
}

/// A claimed project is not a project any more, as far as every read is concerned.
///
/// This is what makes the fence work at all: deletion spans four stores with no transaction over
/// them, so the claim is the only interval in which "this project is going away" is a fact anyone
/// can observe. If reads still returned it, a user could open a project whose spans were already
/// deleted and whose files were already gone.
#[tokio::test]
async fn a_project_claimed_for_deletion_reads_as_absent() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Going Away")
        .await
        .unwrap();

    assert!(
        claim_project_for_deletion(&pool, &project.id)
            .await
            .unwrap()
    );

    assert!(
        get_project(&pool, &project.id).await.unwrap().is_none(),
        "a claimed project is reported absent"
    );
    let (listed, total) = list_projects(&pool, 1, 100).await.unwrap();
    assert!(
        !listed.iter().any(|p| p.id == project.id),
        "and is not listed"
    );
    assert_eq!(
        total as usize,
        listed.len(),
        "the total must count what the page can show, or pagination reports a project nothing returns"
    );
    let (for_org, org_total) = list_for_org(&pool, "default", 1, 100).await.unwrap();
    assert!(!for_org.iter().any(|p| p.id == project.id));
    assert_eq!(org_total as usize, for_org.len());

    // Renaming it is refused for the same reason: it is not there to rename.
    assert!(
        update_project(&pool, &project.id, "New Name")
            .await
            .unwrap()
            .is_none()
    );

    // But deletion itself still reaches the row - it is the one thing that must.
    assert!(delete_project(&pool, &project.id).await.unwrap());
}

/// Two deletions of one project: exactly one owns it.
#[tokio::test]
async fn only_one_claim_on_a_project_can_win() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Contested").await.unwrap();

    assert!(
        claim_project_for_deletion(&pool, &project.id)
            .await
            .unwrap()
    );
    assert!(
        !claim_project_for_deletion(&pool, &project.id)
            .await
            .unwrap(),
        "the second caller must learn it does not own this deletion"
    );
    assert!(
        !claim_project_for_deletion(&pool, "no-such-project")
            .await
            .unwrap(),
        "and a project that does not exist cannot be claimed"
    );
    assert!(
        !project_accepts_writes(&pool, &project.id).await.unwrap(),
        "and a claimed project accepts no writes"
    );
}

/// An abandoned project deletion is findable by age, so it can be finished.
///
/// Nothing releases a claim - that is what lets it survive a restart - so without this a project
/// whose cleanup died is fenced forever: hidden from every read while its data is still on disk.
#[tokio::test]
async fn an_abandoned_project_claim_is_found_by_age() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Half Deleted")
        .await
        .unwrap();
    assert!(
        claim_project_for_deletion(&pool, &project.id)
            .await
            .unwrap()
    );

    assert!(
        get_stale_claimed_projects(&pool, 60)
            .await
            .unwrap()
            .is_empty(),
        "a claim taken a moment ago is a deletion in progress"
    );

    sqlx::query("UPDATE projects SET deleting_at = deleting_at - 1000 WHERE id = ?")
        .bind(&project.id)
        .execute(&pool)
        .await
        .unwrap();
    let stale = get_stale_claimed_projects(&pool, 60).await.unwrap();
    assert_eq!(
        stale.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
        vec![project.id.clone()]
    );

    // Leasing it needs the tombstone value that was read, and refreshes it - so a second replica
    // holding the same reading is refused and the fence stays set throughout.
    let observed = stale[0].1;
    assert!(
        !reclaim_stale_project(&pool, &project.id, observed + 1)
            .await
            .unwrap(),
        "a tombstone that is not the one observed is not leasable"
    );
    assert!(
        reclaim_stale_project(&pool, &project.id, observed)
            .await
            .unwrap(),
        "the tombstone as observed is leased"
    );
    assert!(
        !reclaim_stale_project(&pool, &project.id, observed)
            .await
            .unwrap(),
        "and the lease moved it on, so the same reading cannot be taken twice"
    );
    assert!(
        !project_accepts_writes(&pool, &project.id).await.unwrap(),
        "leasing must not lift the fence"
    );

    delete_project(&pool, &project.id).await.unwrap();
    assert!(
        get_stale_claimed_projects(&pool, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A tombstone is removed on repeated evidence, and a late write starts that evidence over.
///
/// This is the barrier, expressed as the thing it has to do. A writer that read the fence before the
/// tombstone can commit arbitrarily later, so a sweep that finds data must not be treated as a
/// failure - it is the case the tombstone exists for. It deletes what appeared and the count restarts.
#[tokio::test]
async fn a_tombstone_is_removed_by_repeated_evidence_and_a_late_write_resets_it() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Going Away")
        .await
        .unwrap();
    assert!(
        claim_project_for_deletion(&pool, &project.id)
            .await
            .unwrap()
    );

    // Four quiet sweeps are not enough when five are required.
    for pass in 1..=4 {
        assert!(
            !record_project_sweep(&pool, &project.id, true, 5, 0)
                .await
                .unwrap(),
            "pass {pass} must not be enough on its own"
        );
    }
    // A late writer's spans show up. The count starts over - that is the whole point.
    assert!(
        !record_project_sweep(&pool, &project.id, false, 5, 0)
            .await
            .unwrap(),
        "a sweep that found data cannot also authorise removing the row"
    );
    for pass in 1..=4 {
        assert!(
            !record_project_sweep(&pool, &project.id, true, 5, 0)
                .await
                .unwrap(),
            "the count restarted, so pass {pass} is not enough again"
        );
    }
    // Through all of it the project accepted no writes.
    assert!(!project_accepts_writes(&pool, &project.id).await.unwrap());

    // The fifth removes the row, in the same statement that decides it may go - so no other sweep can
    // reset the count in between and leave this one acting on a decision that is no longer true.
    assert!(
        record_project_sweep(&pool, &project.id, true, 5, 0)
            .await
            .unwrap(),
        "five consecutive quiet sweeps is the evidence the row waits for"
    );
    assert!(
        get_project(&pool, &project.id).await.unwrap().is_none(),
        "and the row is gone with it"
    );
    assert!(
        !record_project_sweep(&pool, &project.id, true, 5, 0)
            .await
            .unwrap(),
        "a project with no row has no tombstone to advance"
    );
}

/// A removed tombstone leaves a record, so a write that arrives afterwards is still collected.
///
/// This is what makes the guarantee hold for an *arbitrarily* delayed writer rather than one that
/// finishes within a few sweeps. The tombstone goes on finite evidence; a writer that read the fence
/// before it can commit after the row is gone, and without a record nothing would know the project had
/// existed. The record is written in the same transaction as the removal, because a crash in between is
/// the one moment it matters.
#[tokio::test]
async fn removing_a_tombstone_records_that_the_project_existed() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Remembered")
        .await
        .unwrap();
    claim_project_for_deletion(&pool, &project.id)
        .await
        .unwrap();

    assert!(
        claim_deleted_projects_for_check(&pool, 0, 100)
            .await
            .unwrap()
            .is_empty(),
        "nothing is remembered while the tombstone is still there"
    );
    assert!(
        record_project_sweep(&pool, &project.id, true, 1, 0)
            .await
            .unwrap(),
        "one clean sweep is enough when one is required"
    );
    assert_eq!(
        claim_deleted_projects_for_check(&pool, 0, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|(id, _token)| id)
            .collect::<Vec<_>>(),
        vec![project.id.clone()],
        "and the id is remembered, so the sweep keeps collecting for it"
    );

    // Still refused for writes: an absent project is refused as firmly as a claimed one.
    assert!(!project_accepts_writes(&pool, &project.id).await.unwrap());

    // Forgotten only past the retention, which is where the residual is stated.
    assert_eq!(
        forget_deleted_projects(&pool, 3600).await.unwrap(),
        0,
        "a deletion from a moment ago is inside any retention"
    );
    assert_eq!(
        forget_deleted_projects(&pool, 0).await.unwrap(),
        1,
        "and past it the id is forgotten"
    );
    assert!(
        claim_deleted_projects_for_check(&pool, 0, 100)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A deleted id is claimed once per window, however many instances are sweeping.
///
/// The records are permanent - any retention would bound how late a stalled writer may commit and still
/// be collected - so a bare list would have every instance re-check every deletion ever made on every
/// sweep: work proportional to instances times lifetime deletions. Claiming in the statement that
/// returns the ids makes concurrent instances race for each one.
#[tokio::test]
async fn a_deleted_project_is_claimed_for_checking_once_per_window() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Remembered")
        .await
        .unwrap();
    claim_project_for_deletion(&pool, &project.id)
        .await
        .unwrap();
    record_project_sweep(&pool, &project.id, true, 1, 0)
        .await
        .unwrap();

    // Five instances, one window: exactly one gets the id, and it comes with the token that owns it.
    let mut claims = Vec::new();
    for _ in 0..5 {
        claims.extend(
            claim_deleted_projects_for_check(&pool, 600, 10)
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        claims.len(),
        1,
        "an id claimed inside a window must not be handed out again, or the work grows with the \
             instance count"
    );
    let (id, token) = claims.pop().expect("the one claim");
    assert_eq!(id, project.id);

    // The claim *leased* it, so nothing else is due until the holder reports.
    assert!(
        claim_deleted_projects_for_check(&pool, 600, 10)
            .await
            .unwrap()
            .is_empty(),
        "a leased id is not re-claimable while its holder is still working"
    );

    // Reporting with no gap makes it due again, which is what the next sweep sees.
    record_deleted_project_check(&pool, &id, token, false, 0, 0)
        .await
        .unwrap();
    assert_eq!(
        claim_deleted_projects_for_check(&pool, 0, 10)
            .await
            .unwrap()
            .len(),
        1,
        "once the holder has reported, the next sweep claims it again"
    );
}

/// A worker whose lease expired mid-batch cannot overwrite the worker that took the id after it.
///
/// Reporting was an unconditional update by project id, so a slow sweep could come back after its lease
/// had gone, replace the *new* holder's lease with its own schedule, and let a third sweep claim an id
/// that was still being processed - duplicating the storage work this scheduler exists to bound. The
/// token the claim handed out is what a report must present.
#[tokio::test]
async fn a_stale_claim_cannot_overwrite_its_successor() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Slowly Swept")
        .await
        .unwrap();
    claim_project_for_deletion(&pool, &project.id)
        .await
        .unwrap();
    record_project_sweep(&pool, &project.id, true, 1, 0)
        .await
        .unwrap();

    // Worker A claims it with a lease that we then treat as expired.
    let (id, stale_token) = claim_deleted_projects_for_check(&pool, 0, 1)
        .await
        .unwrap()
        .pop()
        .expect("due immediately");

    // Worker B claims the same id after A's lease lapsed, and holds it.
    let (_, fresh_token) = claim_deleted_projects_for_check(&pool, 3600, 1)
        .await
        .unwrap()
        .pop()
        .expect("A's lease has lapsed, so B can claim it");
    assert_ne!(
        stale_token, fresh_token,
        "each claim owns the id under its own token"
    );

    // A finally reports. It must change nothing: B's lease stands.
    record_deleted_project_check(&pool, &id, stale_token, true, 0, 0)
        .await
        .unwrap();
    assert!(
        claim_deleted_projects_for_check(&pool, 0, 10)
            .await
            .unwrap()
            .is_empty(),
        "a stale report reinstated the base interval, so a third sweep can claim an id that worker B \
             is still processing"
    );

    // B's own report does apply.
    record_deleted_project_check(&pool, &id, fresh_token, false, 0, 0)
        .await
        .unwrap();
    assert_eq!(
        claim_deleted_projects_for_check(&pool, 0, 10)
            .await
            .unwrap()
            .len(),
        1,
        "the holder's report is the one that reschedules"
    );
}

/// Discovery is bounded per sweep and backs off per quiet check.
///
/// The records are permanent - any retention would bound how late a stalled writer may commit and still
/// be collected - so what has to be bounded is the *rate*. Without the backoff a hundred thousand
/// historical deletions meant a hundred thousand storage listings every sweep, forever; without the
/// batch cap one sweep could outlive its own window and overlap the next.
#[tokio::test]
async fn deleted_project_checks_are_batched_and_back_off() {
    let pool = setup_test_pool().await;
    for n in 0..5 {
        let project = create_project(&pool, "default", &format!("Gone {n}"))
            .await
            .unwrap();
        claim_project_for_deletion(&pool, &project.id)
            .await
            .unwrap();
        record_project_sweep(&pool, &project.id, true, 1, 0)
            .await
            .unwrap();
    }

    // The batch caps the work regardless of how many are due.
    assert_eq!(
        claim_deleted_projects_for_check(&pool, 0, 2)
            .await
            .unwrap()
            .len(),
        2,
        "a sweep must not take on more than its batch, or it can outlive its own window"
    );

    // A quiet check pushes the next one out; a check that found something brings it back.
    let ids = claim_deleted_projects_for_check(&pool, 0, 10)
        .await
        .unwrap();
    let (subject, token) = ids.first().expect("some id is due").clone();
    record_deleted_project_check(&pool, &subject, token, true, 60, 86_400)
        .await
        .unwrap();
    record_deleted_project_check(&pool, &subject, token, true, 60, 86_400)
        .await
        .unwrap();
    let quiet: (i64,) =
        sqlx::query_as("SELECT quiet_checks FROM deleted_projects WHERE project_id = ?")
            .bind(&subject)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(quiet.0, 2, "consecutive quiet checks accumulate");

    // With a base interval of 60s and two quiet checks, this id is not due for 240s.
    let due = claim_deleted_projects_for_check(&pool, 0, 10)
        .await
        .unwrap();
    assert!(
        !due.iter().any(|(id, _)| *id == subject),
        "a project checked twice with nothing found must not be re-checked at the base rate"
    );

    record_deleted_project_check(&pool, &subject, token, false, 60, 86_400)
        .await
        .unwrap();
    let quiet: (i64,) =
        sqlx::query_as("SELECT quiet_checks FROM deleted_projects WHERE project_id = ?")
            .bind(&subject)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        quiet.0, 0,
        "finding something brings it back to the base interval"
    );
}

/// Concurrent instances cannot inflate the count: one window, one increment.
///
/// Every instance of a horizontally scaled deployment runs the sweep. With a bare increment, five
/// instances reached five "consecutive clean sweeps" inside a single interval - the barrier getting
/// *weaker* the more instances you run, which is the opposite of what scaling out should do. The
/// increment is gated on a window having passed, and it is one atomic UPDATE, so the instances race
/// for the row and one wins.
#[tokio::test]
async fn concurrent_sweeps_within_one_window_count_once() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Swept By Many")
        .await
        .unwrap();
    claim_project_for_deletion(&pool, &project.id)
        .await
        .unwrap();

    // Five instances, one window: the count must advance by one, so five is never reached.
    for _ in 0..5 {
        assert!(
            !record_project_sweep(&pool, &project.id, true, 5, 600)
                .await
                .unwrap(),
            "sweeps inside one window must not stack up into the evidence the row waits for"
        );
    }
    let count: (i64,) = sqlx::query_as("SELECT clean_sweeps FROM projects WHERE id = ?")
        .bind(&project.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count.0, 1,
        "five concurrent sweeps in one window are one observation"
    );

    // A finding of data resets unconditionally - the safe direction, and not gated on the window.
    record_project_sweep(&pool, &project.id, false, 5, 600)
        .await
        .unwrap();
    let count: (i64,) = sqlx::query_as("SELECT clean_sweeps FROM projects WHERE id = ?")
        .bind(&project.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count.0, 0,
        "a late writer's spans reset the evidence whatever the window"
    );
}

/// An organization is tombstoned too, because its row is what its projects' tombstones hang from.
#[tokio::test]
async fn an_organization_tombstone_hides_it_and_its_row_waits_for_its_projects() {
    let pool = setup_test_pool().await;
    let project = create_project(&pool, "default", "Owned").await.unwrap();

    assert!(
        claim_organization_for_deletion(&pool, "default")
            .await
            .unwrap()
    );
    assert!(
        !claim_organization_for_deletion(&pool, "default")
            .await
            .unwrap(),
        "one caller owns the deletion"
    );
    assert!(
        get_stale_claimed_organizations(&pool, 0)
            .await
            .unwrap()
            .iter()
            .any(|(id, _)| id == "default")
    );
    assert!(
        count_projects_of_organization(&pool, "default")
            .await
            .unwrap()
            >= 1,
        "its row may not go while a project row still hangs from it"
    );

    // Tombstoned projects still count: their rows are what their own cleanups depend on.
    claim_project_for_deletion(&pool, &project.id)
        .await
        .unwrap();
    let before = count_projects_of_organization(&pool, "default")
        .await
        .unwrap();
    delete_project(&pool, &project.id).await.unwrap();
    assert_eq!(
        count_projects_of_organization(&pool, "default")
            .await
            .unwrap(),
        before - 1,
        "and it stops counting only when the row is really gone"
    );
}

#[tokio::test]
async fn test_delete_project_not_found() {
    let pool = setup_test_pool().await;
    let deleted = delete_project(&pool, "nonexistent").await.unwrap();
    assert!(!deleted);
}

#[tokio::test]
async fn test_default_project_exists() {
    let pool = setup_test_pool().await;
    let project = get_project(&pool, "default").await.unwrap();
    assert!(project.is_some());
    let project = project.unwrap();
    assert_eq!(project.name, "Default Project");
    assert_eq!(project.organization_id, "default");
}
