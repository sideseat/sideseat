use chrono::{TimeZone, Utc};
use sqlx::SqlitePool;

use super::*;
use sideseat_ports::types::{ProjectId, RawConfirmOutcome, RawOwnerClaim, StagedPayload};

fn now() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000, 0).unwrap()
}

fn project() -> ProjectId {
    ProjectId::from("p")
}

fn claim(raw_id: &str, version_epoch: i64, traces: &[&str]) -> RawOwnerClaim {
    RawOwnerClaim {
        raw_id: raw_id.to_string(),
        version_epoch,
        trace_ids: traces.iter().map(|t| t.to_string()).collect(),
    }
}

async fn setup_test_pool() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::query(crate::schema::SCHEMA)
        .execute(&pool)
        .await
        .unwrap();
    pool
}

async fn owners(pool: &SqlitePool, raw_id: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT trace_id FROM raw_owners WHERE project_id = 'p' AND raw_id = ? ORDER BY trace_id",
    )
    .bind(raw_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_project_that_never_collected_reads_epoch_zero_and_no_cutoffs() {
    let pool = setup_test_pool().await;
    assert_eq!(epoch(&pool, &project()).await.unwrap(), 0);
    assert!(
        cutoffs(&pool, &project(), &["r".to_string()])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_confirmed_claim_grants_ownership_to_its_traces() {
    let pool = setup_test_pool().await;
    let outcomes = confirm_owners(&pool, &project(), &[claim("r", 0, &["t1", "t2"])])
        .await
        .unwrap();
    assert_eq!(outcomes, vec![RawConfirmOutcome::Granted]);
    assert_eq!(owners(&pool, "r").await, vec!["t1", "t2"]);
}

#[tokio::test]
async fn the_fence_excludes_a_tombstoned_trace_from_the_grant() {
    let pool = setup_test_pool().await;
    sqlx::query(
        "INSERT INTO deleted_traces (project_id, trace_id, deleted_at) VALUES ('p', 't1', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let outcomes = confirm_owners(&pool, &project(), &[claim("r", 0, &["t1", "t2"])])
        .await
        .unwrap();
    assert_eq!(outcomes, vec![RawConfirmOutcome::Granted]);
    assert_eq!(owners(&pool, "r").await, vec!["t2"]);
}

#[tokio::test]
async fn the_fence_refuses_a_version_at_or_below_the_record_s_high_water_mark() {
    let pool = setup_test_pool().await;
    let decided = collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0].epoch, 0);
    drop_collection(&pool, &project(), "r", 0).await.unwrap();

    // A late landing keyed under the old epoch: refused, told to re-key above the fresh epoch.
    let outcomes = confirm_owners(&pool, &project(), &[claim("r", 0, &["t1"])])
        .await
        .unwrap();
    assert_eq!(
        outcomes,
        vec![RawConfirmOutcome::Refused { fresh_epoch: 1 }]
    );
    assert!(
        owners(&pool, "r").await.is_empty(),
        "a refused claim grants nothing"
    );

    // Re-keyed above the mark, the same writer is granted.
    let outcomes = confirm_owners(&pool, &project(), &[claim("r", 1, &["t1"])])
        .await
        .unwrap();
    assert_eq!(outcomes, vec![RawConfirmOutcome::Granted]);
}

#[tokio::test]
async fn the_fence_refuses_a_version_at_or_below_a_pending_decision() {
    let pool = setup_test_pool().await;
    collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    // The decision is pending - not yet dropped into the high-water mark - and still refuses the claim: a
    // takeover purge of it may run however late (`RawRecordOwnership.tla`, `Doomed`).
    let outcomes = confirm_owners(&pool, &project(), &[claim("r", 0, &["t1"])])
        .await
        .unwrap();
    assert_eq!(
        outcomes,
        vec![RawConfirmOutcome::Refused { fresh_epoch: 1 }]
    );
}

#[tokio::test]
async fn a_collection_is_refused_while_any_trace_owns_the_record() {
    let pool = setup_test_pool().await;
    confirm_owners(&pool, &project(), &[claim("r", 0, &["t1"])])
        .await
        .unwrap();
    let decided = collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    assert!(decided.is_empty());
}

#[tokio::test]
async fn a_collection_is_refused_while_a_staged_payload_writes_the_record() {
    let pool = setup_test_pool().await;
    let payload = StagedPayload {
        id: "payload".to_string(),
        project_id: project(),
        signal: sideseat_ports::types::StagedSignal::Traces,
        blob_hash: "hash".to_string(),
        byte_len: 1,
        created_at: now(),
        redrive_attempts: 0,
        unconfirmed: false,
        records: Vec::new(),
        raw_id: Some("r".to_string()),
    };
    super::super::staging::create(&pool, &payload)
        .await
        .unwrap();
    assert!(writer_pending(&pool, &project(), "r").await.unwrap());
    let decided = collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    assert!(decided.is_empty(), "a registered payload blocks collection");

    // Settlement retires the registration and the writer entry together; the record is collectable.
    super::super::staging::delete(&pool, "payload")
        .await
        .unwrap();
    assert!(!writer_pending(&pool, &project(), "r").await.unwrap());
    let decided = collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    assert_eq!(decided.len(), 1);
}

#[tokio::test]
async fn a_collection_is_refused_under_an_active_legal_hold() {
    let pool = setup_test_pool().await;
    sqlx::query(
        "INSERT INTO project_holds (project_id, hold_until, updated_at) VALUES ('p', ?, 0)",
    )
    .bind((now() + chrono::Duration::hours(1)).timestamp_micros())
    .execute(&pool)
    .await
    .unwrap();
    let decided = collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    assert!(decided.is_empty());

    // An expired hold holds nothing.
    sqlx::query("UPDATE project_holds SET hold_until = ?")
        .bind((now() - chrono::Duration::hours(1)).timestamp_micros())
        .execute(&pool)
        .await
        .unwrap();
    let decided = collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    assert_eq!(decided.len(), 1);
}

#[tokio::test]
async fn each_decision_bumps_the_epoch_and_the_drop_keeps_the_record_s_cutoff() {
    let pool = setup_test_pool().await;
    let first = collect_records(&pool, &project(), &["a".to_string()], now())
        .await
        .unwrap();
    let second = collect_records(&pool, &project(), &["b".to_string()], now())
        .await
        .unwrap();
    assert_eq!(first[0].epoch, 0);
    assert_eq!(second[0].epoch, 1);
    assert_eq!(epoch(&pool, &project()).await.unwrap(), 2);
    let ids = ["a".to_string(), "b".to_string()];
    // A pending decision already cuts its record off: a reader never sees what a purge may delete.
    let before = cutoffs(&pool, &project(), &ids).await.unwrap();
    assert_eq!((before["a"], before["b"]), (0, 1));

    drop_collection(&pool, &project(), "b", 1).await.unwrap();
    drop_collection(&pool, &project(), "a", 0).await.unwrap();
    let after = cutoffs(&pool, &project(), &ids).await.unwrap();
    assert_eq!(
        before, after,
        "the drop moves a decision into the mark, not the cutoff"
    );
    assert!(
        stale_collections(&pool, now() + chrono::Duration::seconds(1), 10)
            .await
            .unwrap()
            .is_empty()
    );
}

/// One record's collection must never hide another's versions: the cutoff is the record's own, not the
/// project's - the defect a project-wide high-water mark had.
#[tokio::test]
async fn collecting_one_record_never_cuts_off_another() {
    let pool = setup_test_pool().await;
    confirm_owners(&pool, &project(), &[claim("live", 0, &["t1"])])
        .await
        .unwrap();
    let decided = collect_records(&pool, &project(), &["dead".to_string()], now())
        .await
        .unwrap();
    drop_collection(&pool, &project(), "dead", decided[0].epoch)
        .await
        .unwrap();
    let cutoffs = cutoffs(&pool, &project(), &["live".to_string(), "dead".to_string()])
        .await
        .unwrap();
    assert_eq!(cutoffs.get("live"), None, "the live record has no cutoff");
    assert_eq!(cutoffs.get("dead"), Some(&0));
    // And a writer of the live record keyed under the epoch the other's decision consumed is still granted.
    let outcomes = confirm_owners(&pool, &project(), &[claim("live", 0, &["t2"])])
        .await
        .unwrap();
    assert_eq!(outcomes, vec![RawConfirmOutcome::Granted]);
}

/// A record collected again after a re-ingest: its second decision lands above its first, and the cutoff
/// follows it.
#[tokio::test]
async fn a_record_collected_twice_takes_the_later_cutoff() {
    let pool = setup_test_pool().await;
    let first = collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    drop_collection(&pool, &project(), "r", first[0].epoch)
        .await
        .unwrap();
    let rekeyed = epoch(&pool, &project()).await.unwrap();
    confirm_owners(&pool, &project(), &[claim("r", rekeyed, &["t1"])])
        .await
        .unwrap();
    release_owner_traces(&pool, &project(), &["t1".to_string()])
        .await
        .unwrap();
    let second = collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    assert_eq!(second[0].epoch, rekeyed);
    let cutoffs = cutoffs(&pool, &project(), &["r".to_string()])
        .await
        .unwrap();
    assert_eq!(cutoffs["r"], rekeyed);
}

#[tokio::test]
async fn a_decided_record_is_not_decided_again_and_stale_decisions_are_found_by_age() {
    let pool = setup_test_pool().await;
    collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    let again = collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    assert!(again.is_empty());
    let stale = stale_collections(&pool, now() + chrono::Duration::seconds(1), 10)
        .await
        .unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].raw_id, "r");
    assert_eq!(stale[0].epoch, 0);
}

#[tokio::test]
async fn a_journalled_trace_deletion_releases_the_traces_ownership_in_the_same_transaction() {
    let pool = setup_test_pool().await;
    confirm_owners(&pool, &project(), &[claim("r", 0, &["t1", "t2"])])
        .await
        .unwrap();
    super::super::project::record_deleted_traces_journalled(
        &pool,
        &project(),
        &["t1".to_string()],
        now(),
    )
    .await
    .unwrap();
    assert_eq!(owners(&pool, "r").await, vec!["t2"]);

    // The confirm fence never grants the deleted trace again.
    confirm_owners(&pool, &project(), &[claim("r", 0, &["t1"])])
        .await
        .unwrap();
    assert_eq!(owners(&pool, "r").await, vec!["t2"]);
}

#[tokio::test]
async fn the_retention_release_takes_ownership_from_swept_traces() {
    let pool = setup_test_pool().await;
    confirm_owners(&pool, &project(), &[claim("r", 0, &["t1", "t2"])])
        .await
        .unwrap();
    release_owner_traces(&pool, &project(), &["t2".to_string()])
        .await
        .unwrap();
    assert_eq!(owners(&pool, "r").await, vec!["t1"]);
}

#[tokio::test]
async fn a_deleted_project_takes_its_arbiter_state_with_it() {
    let pool = setup_test_pool().await;
    confirm_owners(&pool, &project(), &[claim("r", 0, &["t1"])])
        .await
        .unwrap();
    collect_records(&pool, &project(), &["other".to_string()], now())
        .await
        .unwrap();
    delete_project(&pool, &project()).await.unwrap();
    assert!(owners(&pool, "r").await.is_empty());
    assert!(
        stale_collections(&pool, now() + chrono::Duration::seconds(1), 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(epoch(&pool, &project()).await.unwrap(), 0);
}
