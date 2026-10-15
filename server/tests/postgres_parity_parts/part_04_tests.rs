// The raw-record arbiter: both backends must arbitrate identically, and a store missing the arbiter's
// tables must be refused at startup. The protocol is `server/specs/RawRecordOwnership.tla`.

use chrono::Utc;
use sideseat_ports::types::{RawConfirmOutcome, RawOwnerClaim, raw_record_key, raw_trace_key};

fn arbiter_claim(raw_id: &str, version_epoch: i64, traces: &[&str]) -> RawOwnerClaim {
    RawOwnerClaim {
        raw_id: raw_id.to_string(),
        version_epoch,
        trace_ids: traces.iter().map(|t| t.to_string()).collect(),
    }
}

/// One arbitration scenario, as a transcript both backends must produce verbatim: grant, deletion release,
/// collection refusal while written, decision with its epoch, the fence refusing a doomed late claim, the
/// re-keyed claim granted, and the drop folding the epoch into the high-water mark.
#[tokio::test]
async fn the_raw_arbiter_arbitrates_identically_on_both_backends() {
    let Some((sqlite, postgres)) = pair().await else {
        return;
    };
    let project = ProjectId::from("default");
    let mut transcripts: Vec<Vec<String>> = Vec::new();
    for repository in repositories(sqlite, postgres) {
        let mut lines = Vec::new();
        let now = Utc::now();

        let outcomes = repository
            .confirm_raw_owners(&project, &[arbiter_claim("r", 0, &["t1", "t2"])])
            .await
            .expect("confirm");
        lines.push(format!("confirm fresh: {outcomes:?}"));
        let owned = repository
            .raw_owned_pairs(
                &project,
                &[
                    (raw_record_key("r"), raw_trace_key("t1")),
                    (raw_record_key("r"), raw_trace_key("t9")),
                ],
            )
            .await
            .expect("owned pairs");
        lines.push(format!("owned pairs: {}", owned.len()));

        let refused = repository
            .collect_raw_records(&project, &["r".to_string()], now)
            .await
            .expect("collect owned");
        lines.push(format!("collect while owned decides {}", refused.len()));

        repository
            .record_deleted_traces_journalled(&project, &["t1".to_string(), "t2".to_string()])
            .await
            .expect("journalled deletion");
        let decided = repository
            .collect_raw_records(&project, &["r".to_string()], now)
            .await
            .expect("collect unowned");
        lines.push(format!(
            "collect after deletion decides epochs {:?}",
            decided.iter().map(|d| d.epoch).collect::<Vec<_>>()
        ));

        let late = repository
            .confirm_raw_owners(&project, &[arbiter_claim("r", 0, &["t3"])])
            .await
            .expect("late confirm");
        lines.push(format!("late claim under the decision: {late:?}"));
        assert_eq!(
            late,
            vec![RawConfirmOutcome::Collecting],
            "no grant while a decision is pending"
        );

        let stale = repository
            .stale_raw_collections(now + chrono::Duration::seconds(1), 10)
            .await
            .expect("stale");
        lines.push(format!(
            "stale decisions: {:?}",
            stale
                .iter()
                .map(|d| (d.raw_id.clone(), d.epoch))
                .collect::<Vec<_>>()
        ));

        repository
            .drop_raw_collection(&project, "r", decided[0].epoch)
            .await
            .expect("drop");
        let epoch = repository.raw_arbiter_epoch(&project).await.expect("epoch");
        let cutoffs = repository
            .raw_record_cutoffs(&project, &["r".to_string(), "other".to_string()])
            .await
            .expect("cutoffs");
        lines.push(format!("after drop: epoch {epoch} cutoffs {:?}", {
            let mut sorted: Vec<_> = cutoffs.into_iter().collect();
            sorted.sort();
            sorted
        }));

        let stale_claim = repository
            .confirm_raw_owners(&project, &[arbiter_claim("r", 0, &["t3"])])
            .await
            .expect("stale confirm");
        lines.push(format!("stale claim at the cutoff: {stale_claim:?}"));
        assert!(matches!(
            stale_claim.as_slice(),
            [RawConfirmOutcome::Refused { .. }]
        ));

        let rekeyed = repository
            .confirm_raw_owners(&project, &[arbiter_claim("r", epoch, &["t3"])])
            .await
            .expect("rekeyed confirm");
        lines.push(format!("re-keyed claim above the cutoff: {rekeyed:?}"));
        assert_eq!(rekeyed, vec![RawConfirmOutcome::Granted]);

        // Another record's decision never cuts this one off.
        let other = repository
            .collect_raw_records(&project, &["other".to_string()], now)
            .await
            .expect("collect other");
        repository
            .drop_raw_collection(&project, "other", other[0].epoch)
            .await
            .expect("drop other");
        let still = repository
            .confirm_raw_owners(&project, &[arbiter_claim("r", epoch, &["t4"])])
            .await
            .expect("confirm after another record's collection");
        lines.push(format!("after another record's collection: {still:?}"));

        transcripts.push(lines);
    }
    assert_eq!(
        transcripts[0], transcripts[1],
        "SQLite (reference) and PostgreSQL arbitrated differently"
    );
}

/// A store at the current version created before the arbiter's tables existed is refused at startup with the
/// reset command named, exactly as SQLite and DuckDB refuse an earlier layout of their version.
#[tokio::test]
async fn a_postgres_store_missing_the_arbiter_tables_is_refused_with_the_reset_named() {
    let Some((_sqlite, _postgres)) = pair().await else {
        return;
    };
    // The drops need the table owner: the bootstrap user the URL names, not the maintenance role the
    // service's pools assume.
    let owner = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&std::env::var(URL_ENV).expect("url"))
        .await
        .expect("owner connection");
    for table in [
        "raw_owners",
        "raw_writers",
        "raw_collections",
        "raw_arbiter",
    ] {
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(&owner)
            .await
            .expect("drop an arbiter table");
    }

    let config = PostgresConfig {
        url: std::env::var(URL_ENV).expect("url"),
        max_connections: 2,
        min_connections: 1,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 60,
        max_lifetime_secs: 600,
        statement_timeout_secs: 30,
    };
    let error = match PostgresService::init(
        &config,
        Arc::new(sideseat_server::runtime::clock::SystemClock),
    )
    .await
    {
        Ok(_) => panic!("an earlier layout of this version must be refused"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(
        message.contains("missing"),
        "names what is missing: {message}"
    );
    assert!(
        message.contains("raw_"),
        "names the arbiter tables: {message}"
    );
    assert!(
        message.contains("sideseat system prune"),
        "names the reset command: {message}"
    );

    // Put the tables back for the scenarios that follow: the schema script is idempotent, and the fresh
    // tables need their grants and policies again.
    sqlx::raw_sql(sideseat_adapter_postgres::schema::SCHEMA)
        .execute(&owner)
        .await
        .expect("restore the arbiter tables");
    sqlx::raw_sql(sideseat_adapter_postgres::schema::TENANT_RLS_SQL)
        .execute(&owner)
        .await
        .expect("restore the arbiter policies");
}

/// The confirm fence and a journalled trace deletion racing on PostgreSQL's concurrent writers: whatever the
/// interleaving, a tombstoned trace never ends up owning a record (`RawRecordOwnership.tla`,
/// `NoResurrectedRead`'s `aown \cap tomb = {}`).
#[tokio::test]
async fn a_deleted_trace_never_owns_a_record_whatever_races_its_deletion() {
    use sideseat_ports::traits::{DeletionJournal, RawArbiter};
    let Some((_sqlite, postgres)) = pair().await else {
        return;
    };
    let repository = std::sync::Arc::new(postgres);
    let project = ProjectId::from("default");
    for round in 0..100 {
        let trace = format!("race-{round}");
        // Both sides released together, so their transactions overlap as often as the scheduler allows.
        let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let confirm = {
            let repository = repository.clone();
            let project = project.clone();
            let trace = trace.clone();
            let start = start.clone();
            tokio::spawn(async move {
                start.wait().await;
                repository
                    .confirm_raw_owners(&project, &[arbiter_claim("raced", 0, &[trace.as_str()])])
                    .await
            })
        };
        let delete = {
            let repository = repository.clone();
            let project = project.clone();
            let trace = trace.clone();
            // Both entry points that tombstone a trace, alternately: each must take the owner locks.
            let start = start.clone();
            tokio::spawn(async move {
                start.wait().await;
                if round % 2 == 0 {
                    repository
                        .record_deleted_traces_journalled(&project, &[trace])
                        .await
                } else {
                    sideseat_ports::traits::ProjectStore::record_deleted_traces(
                        repository.as_ref(),
                        &project,
                        &[trace],
                    )
                    .await
                }
            })
        };
        confirm.await.expect("join").expect("confirm");
        delete.await.expect("join").expect("delete");
    }
    // Owner rows carry trace keys, so the tombstoned traces are keyed the same way before they are compared.
    let deleted: Vec<String> =
        sqlx::query_scalar("SELECT trace_id FROM deleted_traces WHERE project_id = 'default'")
            .fetch_all(repository.pool())
            .await
            .expect("tombstones");
    let keys: Vec<Vec<u8>> = deleted
        .iter()
        .map(|trace| sideseat_ports::types::raw_trace_key(trace))
        .collect();
    let owned: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM raw_owners WHERE project_id = 'default' AND trace_key = ANY($1)",
    )
    .bind(&keys)
    .fetch_one(repository.pool())
    .await
    .expect("count");
    assert_eq!(deleted.len(), 100, "every round tombstoned its trace");
    assert_eq!(owned, 0, "a tombstoned trace owns a record");
}

/// The retention-cleanup backlog an operator reads to tell retention stalled from storage growing reads alike on
/// both backends: what is owed, since when, and how many passes claimed it without completing it.
#[tokio::test]
async fn the_retention_cleanup_backlog_reads_identically_on_both_backends() {
    let Some((sqlite, postgres)) = pair().await else {
        return;
    };
    let project = ProjectId::from("default");
    let services = [
        TransactionalService::Sqlite(sqlite),
        TransactionalService::Postgres(postgres.0),
    ];
    let mut answers = Vec::new();
    for service in &services {
        let repository = service.repository();
        let governance = service.governance_repository();
        let empty = governance
            .retention_cleanup_backlog(&project)
            .await
            .expect("backlog");
        let recorded = repository
            .record_retention_cleanup(&project, &["t1".to_string(), "t2".to_string()])
            .await
            .expect("record");
        for _ in 0..2 {
            repository
                .claim_retention_cleanup(16, 0)
                .await
                .expect("claim");
        }
        let owed = governance
            .retention_cleanup_backlog(&project)
            .await
            .expect("backlog");
        repository
            .complete_retention_cleanup(&project, &recorded)
            .await
            .expect("complete");
        let done = governance
            .retention_cleanup_backlog(&project)
            .await
            .expect("backlog");
        repository
            .confirm_raw_owners(&project, &[arbiter_claim("0a", 0, &["t1", "t2"])])
            .await
            .expect("confirm");
        repository
            .collect_raw_records(&project, &["0b".to_string()], Utc::now())
            .await
            .expect("collect");
        let held = governance
            .held_transactional_bytes(&project)
            .await
            .expect("held bytes");
        answers.push(format!("arbiter rows held: {held}"));
        answers.push(format!(
            "empty {}/{}/{} owed {}/{}/{} done {}",
            empty.pending,
            empty.oldest_pending_at.is_some(),
            empty.most_attempts,
            owed.pending,
            owed.oldest_pending_at.is_some(),
            owed.most_attempts,
            done.pending
        ));
    }
    assert_eq!(answers[1], "empty 0/false/0 owed 2/true/2 done 0");
    assert_eq!(
        answers[..2],
        answers[2..],
        "SQLite (reference) and PostgreSQL account and report alike"
    );
}

/// A deferral keeps a trace's cleanup owed under a fresh token, past a stale completion, without bringing its
/// next attempt forward; a trace with no row is due now. Alike on both backends.
#[tokio::test]
async fn a_deferred_cleanup_keeps_its_schedule_on_both_backends() {
    let Some((sqlite, postgres)) = pair().await else {
        return;
    };
    let project = ProjectId::from("default");
    let services = [
        TransactionalService::Sqlite(sqlite),
        TransactionalService::Postgres(postgres.0),
    ];
    let mut answers = Vec::new();
    for service in &services {
        let repository = service.repository();
        repository
            .record_retention_cleanup(&project, &["t1".to_string()])
            .await
            .expect("record");
        let claimed: Vec<(String, i64)> = repository
            .claim_retention_cleanup(16, 600)
            .await
            .expect("claim")
            .into_iter()
            .map(|(_, trace, token)| (trace, token))
            .collect();
        repository
            .defer_retention_cleanup(&project, &["t1".to_string()])
            .await
            .expect("defer");
        repository
            .complete_retention_cleanup(&project, &claimed)
            .await
            .expect("complete");
        let before = repository
            .claim_retention_cleanup(16, 600)
            .await
            .expect("claim");
        repository
            .defer_retention_cleanup(&project, &["t2".to_string()])
            .await
            .expect("defer");
        let fresh = repository
            .claim_retention_cleanup(16, 600)
            .await
            .expect("claim");
        let owed = service
            .governance_repository()
            .retention_cleanup_backlog(&project)
            .await
            .expect("backlog");
        answers.push(format!(
            "due before its backoff {:?}, due fresh {:?}, owed {}",
            before
                .iter()
                .map(|(_, t, _)| t.as_str())
                .collect::<Vec<_>>(),
            fresh.iter().map(|(_, t, _)| t.as_str()).collect::<Vec<_>>(),
            owed.pending
        ));
    }
    assert_eq!(
        answers[0],
        r#"due before its backoff [], due fresh ["t2"], owed 2"#
    );
    assert_eq!(
        answers[0], answers[1],
        "SQLite (reference) and PostgreSQL alike"
    );
}

/// A store at the current version created before the cleanup token sequence existed is refused at startup with
/// the reset command named: a sequence no column default names is part of the layout too.
#[tokio::test]
async fn a_postgres_store_missing_the_cleanup_token_sequence_is_refused() {
    let Some((_sqlite, _postgres)) = pair().await else {
        return;
    };
    let owner = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&std::env::var(URL_ENV).expect("url"))
        .await
        .expect("owner connection");
    sqlx::query("DROP SEQUENCE retention_cleanup_tokens")
        .execute(&owner)
        .await
        .expect("drop the sequence");

    let config = PostgresConfig {
        url: std::env::var(URL_ENV).expect("url"),
        max_connections: 2,
        min_connections: 1,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 60,
        max_lifetime_secs: 600,
        statement_timeout_secs: 30,
    };
    let refused = PostgresService::init(
        &config,
        Arc::new(sideseat_server::runtime::clock::SystemClock),
    )
    .await;
    // Put the sequence back before asserting, so a failure here leaves the scenarios that follow a whole
    // store: the schema script is idempotent, and the default privileges grant the roles their use of it.
    sqlx::raw_sql(sideseat_adapter_postgres::schema::SCHEMA)
        .execute(&owner)
        .await
        .expect("restore the sequence");
    let message = match refused {
        Ok(_) => panic!("a layout without the sequence must be refused"),
        Err(error) => error.to_string(),
    };
    assert!(
        message.contains("retention_cleanup_tokens"),
        "names the sequence: {message}"
    );
    assert!(
        message.contains("sideseat system prune"),
        "names the reset command: {message}"
    );
}

/// A confirm that drew its stamp before a newer confirm committed on the same association never lowers that
/// association's stamp: PostgreSQL's `nextval` is outside the transaction, so the older confirm can reach the row
/// after the newer one has committed on it (`SurvivorRelease.tla`, mutant `Max = FALSE`).
///
/// The interleaving is forced on one row, so no plan order matters. Two writers hold `held`. B, on the owner
/// connection, locks the row; A - the repository's confirm - draws its stamp and waits on that lock. A pass reads
/// its horizon there. B then draws a newer stamp, confirms the row with it as a confirm does, and commits; A
/// updates the row last, leaving no writer pending. Had A put its own stamp back, `held` would sit at or below the
/// horizon, and a release with nothing to keep - B's span written after that pass's scan - would take the
/// association of an acknowledged span.
#[tokio::test]
async fn an_older_confirm_never_lowers_a_newer_stamp() {
    let Some((_sqlite, postgres)) = pair().await else {
        return;
    };
    let repository = Arc::new(postgres);
    let project = ProjectId::from("default");
    let trace = "stamp-race";
    let held = hash(0x52);
    for _ in 0..2 {
        repository
            .associate_file(trace, &project, &held, None, 5, "sha256")
            .await
            .expect("associate");
    }
    let owner = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var(URL_ENV).expect("url"))
        .await
        .expect("owner connection");
    let mut newer = owner.begin().await.expect("begin B");
    sqlx::query("SELECT set_config('sideseat.project_id', 'default', true)")
        .execute(&mut *newer)
        .await
        .expect("tenant");
    let locked = sqlx::query(
        "SELECT 1 FROM trace_files WHERE project_id = 'default' AND trace_id = $1 FOR UPDATE",
    )
    .bind(trace)
    .fetch_all(&mut *newer)
    .await
    .expect("lock held");
    assert_eq!(locked.len(), 1, "B holds the row");

    let older = {
        let repository = repository.clone();
        let associations = vec![("default".to_string(), trace.to_string(), held.clone())];
        tokio::spawn(async move {
            repository
                .confirm_trace_file_associations(&associations)
                .await
        })
    };
    // A has drawn its stamp once it waits on the row: the stamp is part of the row's new values, computed before
    // the update waits for the row's lock.
    let mut waited = false;
    for _ in 0..500 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' \
             AND query LIKE '%trace_file_confirm_stamps%'",
        )
        .fetch_one(&owner)
        .await
        .expect("read the waits");
        if waiting > 0 {
            waited = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(waited, "confirm A waits on the row, its stamp drawn");
    let horizon = repository
        .trace_file_confirm_horizon(&project)
        .await
        .expect("horizon");

    // B's confirm of `held`: a stamp drawn after the horizon, applied as the repository's confirm applies it.
    let confirmed = sqlx::query(
        "WITH stamp AS (SELECT nextval('trace_file_confirm_stamps') AS value) \
         UPDATE trace_files SET durable = TRUE, pending_writers = GREATEST(pending_writers - 1, 0), \
             confirm_stamp = GREATEST(confirm_stamp, (SELECT value FROM stamp)) \
         WHERE project_id = 'default' AND trace_id = $1 AND file_hash = $2",
    )
    .bind(trace)
    .bind(&held)
    .execute(&mut *newer)
    .await
    .expect("confirm B");
    assert_eq!(confirmed.rows_affected(), 1, "B confirms first");
    newer.commit().await.expect("commit B");
    assert_eq!(
        older.await.expect("join").expect("confirm A"),
        1,
        "A confirms after B"
    );

    let (pending, stamp): (i32, i64) = sqlx::query_as(
        "SELECT pending_writers, confirm_stamp FROM trace_files \
         WHERE project_id = 'default' AND trace_id = $1",
    )
    .bind(trace)
    .fetch_one(&owner)
    .await
    .expect("read held");
    assert_eq!(pending, 0, "both writers resolved");
    assert!(
        stamp > horizon,
        "`held` keeps B's stamp {stamp}, above the horizon {horizon} read before B drew it"
    );
    let released = repository
        .release_trace_files_except(&project, trace, &[], horizon)
        .await
        .expect("release");
    assert!(
        released.is_empty(),
        "a release at that horizon leaves B's association: {released:?}"
    );
}

/// A lease renewal never waits on the maintenance pool retention and repair share: with every one of that pool's
/// connections held, the lease's own connection still renews, and its statement timeout is the lease statement
/// bound, which `LOST_AFTER` is held to.
#[tokio::test]
async fn a_lease_renews_while_every_maintenance_connection_is_held() {
    use sideseat_ports::traits::{ProcessLeases, ProcessRole};
    let Some((_sqlite, postgres)) = pair().await else {
        return;
    };
    // `pair` sizes the pools at 8: every one of them held.
    let mut held = Vec::new();
    for _ in 0..8 {
        held.push(
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                postgres.0.pool().acquire(),
            )
            .await
            .expect("a free connection")
            .expect("a connection"),
        );
    }
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(300),
            postgres.0.pool().acquire()
        )
        .await
        .is_err(),
        "the maintenance pool is exhausted"
    );
    let renewed = tokio::time::timeout(
        std::time::Duration::from_secs(
            sideseat_core::constants::PROCESS_LEASE_STATEMENT_TIMEOUT_SECS,
        ),
        postgres.renew_process_lease("server-a", ProcessRole::Server, 30),
    )
    .await;
    assert!(
        matches!(renewed, Ok(Ok(()))),
        "renewed through its own connection: {renewed:?}"
    );
    let timeout: String = postgres
        .0
        .lease_connection()
        .run(|connection| {
            Box::pin(async move {
                Ok(sqlx::query_scalar("SHOW statement_timeout")
                    .fetch_one(&mut *connection)
                    .await?)
            })
        })
        .await
        .expect("statement timeout");
    assert_eq!(
        timeout,
        format!(
            "{}s",
            sideseat_core::constants::PROCESS_LEASE_STATEMENT_TIMEOUT_SECS
        )
    );
    drop(held);
}
