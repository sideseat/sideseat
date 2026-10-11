// The raw-record arbiter: both backends must arbitrate identically, and a store missing the arbiter's
// tables must be refused at startup. The protocol is `server/specs/RawRecordOwnership.tla`.

use chrono::Utc;
use sideseat_ports::types::{RawConfirmOutcome, RawOwnerClaim};

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
    let owned: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM raw_owners o JOIN deleted_traces d
         ON d.project_id = o.project_id AND d.trace_id = o.trace_id",
    )
    .fetch_one(repository.pool())
    .await
    .expect("count");
    assert_eq!(owned, 0, "a tombstoned trace owns a record");
}
