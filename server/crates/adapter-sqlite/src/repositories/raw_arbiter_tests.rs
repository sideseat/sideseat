use chrono::{TimeZone, Utc};
use sqlx::SqlitePool;

use super::*;
use sideseat_ports::types::{
    ProjectId, RawConfirmOutcome, RawCutoff, RawOwnerClaim, StagedPayload, raw_record_key,
    raw_trace_key,
};

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

/// The traces owning a record, by name: each candidate trace whose key holds an owner row.
async fn owners(pool: &SqlitePool, raw_id: &str) -> Vec<String> {
    let keys: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT trace_key FROM raw_owners WHERE project_id = 'p' AND record_key = ?",
    )
    .bind(raw_record_key(raw_id))
    .fetch_all(pool)
    .await
    .unwrap();
    ["t1", "t2", "t3", "t4"]
        .into_iter()
        .filter(|trace| keys.contains(&raw_trace_key(trace)))
        .map(str::to_string)
        .collect()
}

fn owner(raw_id: &str, trace: &str) -> (i64, Vec<u8>) {
    (raw_record_key(raw_id), raw_trace_key(trace))
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
async fn the_fence_grants_nothing_while_a_decision_is_pending() {
    let pool = setup_test_pool().await;
    collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    // The decision is pending - not yet dropped into the high-water mark. Its purge removes the rows nobody
    // owns, so no claim is granted meanwhile, not even one keyed above it (`RawRecordOwnership.tla`, `RPurge`).
    let outcomes = confirm_owners(
        &pool,
        &project(),
        &[claim("r", 0, &["t1"]), claim("r", 1, &["t2"])],
    )
    .await
    .unwrap();
    assert_eq!(
        outcomes,
        vec![RawConfirmOutcome::Collecting, RawConfirmOutcome::Collecting]
    );
    assert!(owners(&pool, "r").await.is_empty());

    // Dropped, the decision refuses what it bounds and grants what is above it.
    drop_collection(&pool, &project(), "r", 0).await.unwrap();
    let outcomes = confirm_owners(
        &pool,
        &project(),
        &[claim("r", 0, &["t1"]), claim("r", 1, &["t2"])],
    )
    .await
    .unwrap();
    assert_eq!(
        outcomes,
        vec![
            RawConfirmOutcome::Refused { fresh_epoch: 1 },
            RawConfirmOutcome::Granted
        ]
    );
    assert_eq!(owners(&pool, "r").await, vec!["t2"]);
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
    assert_eq!((before["a"].epoch(), before["b"].epoch()), (0, 1));
    assert_eq!(before["a"].pending, Some(0), "a pending decision");

    drop_collection(&pool, &project(), "b", 1).await.unwrap();
    drop_collection(&pool, &project(), "a", 0).await.unwrap();
    let after = cutoffs(&pool, &project(), &ids).await.unwrap();
    assert_eq!(
        (after["a"], after["b"]),
        (
            RawCutoff {
                high_water: 0,
                pending: None
            },
            RawCutoff {
                high_water: 1,
                pending: None
            }
        ),
        "the drop moves a decision into the mark"
    );
    assert_eq!(
        (after["a"].epoch(), after["b"].epoch()),
        (before["a"].epoch(), before["b"].epoch()),
        "and leaves the cutoff where it was"
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
    assert_eq!(cutoffs.get("dead").map(RawCutoff::epoch), Some(0));
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
    let seq = grant_seq(&pool, &project()).await.unwrap();
    release_owners(&pool, &project(), &[owner("r", "t1")], seq)
        .await
        .unwrap();
    let second = collect_records(&pool, &project(), &["r".to_string()], now())
        .await
        .unwrap();
    assert_eq!(second[0].epoch, rekeyed);
    let cutoffs = cutoffs(&pool, &project(), &["r".to_string()])
        .await
        .unwrap();
    assert_eq!(cutoffs["r"].epoch(), rekeyed);
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

/// The retention release takes only grants made before the analytics read it was decided on: an ingest whose
/// rows that read could not see confirms after it, above the sequence the release carries, and keeps its owner.
#[tokio::test]
async fn the_retention_release_keeps_a_grant_made_after_its_read() {
    let pool = setup_test_pool().await;
    confirm_owners(&pool, &project(), &[claim("r", 0, &["t1", "t2"])])
        .await
        .unwrap();
    let read_at = grant_seq(&pool, &project()).await.unwrap();
    // The ingest's rows land after the release's read, and its confirm after that.
    confirm_owners(&pool, &project(), &[claim("r", 0, &["t2"])])
        .await
        .unwrap();
    release_owners(
        &pool,
        &project(),
        &[owner("r", "t1"), owner("r", "t2")],
        read_at,
    )
    .await
    .unwrap();
    assert_eq!(
        owners(&pool, "r").await,
        vec!["t2"],
        "the later grant stays"
    );
    assert_eq!(
        owners_of_traces(&pool, &project(), &["t2".to_string()])
            .await
            .unwrap(),
        vec![owner("r", "t2")]
    );
    assert_eq!(
        owner_traces(&pool, &project(), "r").await.unwrap(),
        vec![owner("r", "t2").1]
    );
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

#[tokio::test]
async fn owned_pairs_answer_exactly_the_pairs_asked() {
    let pool = setup_test_pool().await;
    confirm_owners(
        &pool,
        &project(),
        &[claim("0a", 0, &["t1", "t2"]), claim("0b", 0, &["t1"])],
    )
    .await
    .unwrap();
    let asked = vec![
        owner("0a", "t1"),
        owner("0a", "t3"),
        owner("0b", "t2"),
        owner("0b", "t1"),
    ];
    let mut owned = owned_pairs(&pool, &project(), &asked).await.unwrap();
    owned.sort();
    let mut expected = vec![owner("0a", "t1"), owner("0b", "t1")];
    expected.sort();
    assert_eq!(owned, expected);
    assert!(
        owned_pairs(&pool, &project(), &[])
            .await
            .unwrap()
            .is_empty()
    );
}

/// The arbiter's rows are held transactional bytes: they grow with what is owned and collected, and the
/// project's storage accounting must see them.
#[tokio::test]
async fn the_arbiter_s_rows_are_held_transactional_bytes() {
    let pool = setup_test_pool().await;
    let p = project();
    let held = || crate::repositories::governance::held_transactional_bytes(&pool, &p);
    assert_eq!(held().await.unwrap(), 0);
    confirm_owners(&pool, &project(), &[claim("0a", 0, &["t1", "t2"])])
        .await
        .unwrap();
    // Two owners: the project id, two integers, and each trace key.
    let owners = 2 * (1 + 16) + raw_trace_key("t1").len() + raw_trace_key("t2").len();
    assert_eq!(held().await.unwrap(), owners as u64);
    collect_records(&pool, &project(), &["0b".to_string()], now())
        .await
        .unwrap();
    assert_eq!(held().await.unwrap(), (owners + 1 + 24 + 2) as u64);
}
