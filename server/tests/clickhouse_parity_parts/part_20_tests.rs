/// The container of the second shard, which the retention-release test stops to make a replica unreachable.
const TWO_SHARD_SECOND_CONTAINER_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_TWO_SHARD_SECOND_CONTAINER";
const TWO_SHARD_SECOND_URL_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_TWO_SHARD_SECOND_URL";

/// A project whose rows the cluster's sharding key places on the second shard, so a read through the first node
/// sees them only if it reaches the other one.
async fn project_on_the_second_shard(client: &clickhouse::Client) -> String {
    for n in 0..64 {
        let candidate = format!("release-shard-{n}");
        let shard: Vec<u64> = client
            .query("SELECT toUInt64((sipHash64(?) % 2) + 1)")
            .bind(&candidate)
            .fetch_all()
            .await
            .expect("compute the shard");
        if shard[0] == 2 {
            return candidate;
        }
    }
    panic!("no project id lands on the second shard");
}

fn docker(args: &[&str]) {
    let status = std::process::Command::new("docker")
        .args(args)
        .status()
        .expect("run docker");
    assert!(status.success(), "docker {args:?} failed");
}

async fn wait_for_ping(url: &str) {
    for _ in 0..120 {
        if reqwest::get(format!("{url}/ping"))
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("{url} did not come back");
}

/// Retention on a two-shard cluster without quorum inserts: the age pass records its traces before it deletes,
/// the release reads which rows remain on every replica, an expired record is released and collected while one
/// whose rows remain keeps its owner, and with a replica unreachable nothing is released - the cleanup stays
/// owed, visibly, and completes once the replica is back.
#[tokio::test]
async fn retention_releases_expired_raw_ownership_across_a_two_shard_cluster_without_quorum() {
    use sideseat_ports::traits::{
        FileMetaStore, RawArbiter, RawStore, StorageGovernance, release_retained_raw_owners,
    };
    use sideseat_ports::types::RawOwnerClaim;

    let Ok(url) = std::env::var(TWO_SHARD_URL_ENV) else {
        eprintln!(
            "clickhouse two-shard: skipped - set {TWO_SHARD_URL_ENV} (or run \
             `make test-clickhouse-two-shard`)"
        );
        return;
    };
    let database = "sideseat_two_shard_retention_release";
    let store = replicated_backend_at(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let client = raw_client_at(&url, database, &user, &password);
    let project = ProjectId::from(project_on_the_second_shard(&client).await.as_str());

    let temp = tempfile::TempDir::new().expect("temp dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let sqlite = Arc::new(sideseat_adapter_sqlite::SqliteRepository(Arc::new(
        sideseat_adapter_sqlite::SqliteService::init(
            &storage,
            Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("sqlite"),
    )));

    let old = Utc::now() - chrono::Duration::days(10);
    // Three records, one trace each: `expired` and `stalled` lose their rows to retention, `kept` keeps its own.
    for name in ["expired", "kept", "stalled"] {
        let trace = format!("trace-{name}");
        let raw_id = format!("raw-{name}");
        store
            .insert_raw_records(&[RawRecordRow {
                project_id: project.clone(),
                signal_until: Utc::now(),
                ..raw_row(&raw_id, 1, &[trace.as_str()], name.as_bytes())
            }])
            .await
            .expect("record");
        store
            .insert_spans(vec![NormalizedSpan {
                project_id: Some(project.to_string()),
                timestamp_start: if name == "kept" { Utc::now() } else { old },
                ingested_at: Some(Utc::now()),
                ..span_of(&trace, "span", &raw_id)
            }])
            .await
            .expect("a span naming the record");
        let granted = sqlite
            .confirm_raw_owners(
                &project,
                &[RawOwnerClaim {
                    raw_id: raw_id.clone(),
                    version_epoch: 0,
                    trace_ids: vec![trace.clone()],
                }],
            )
            .await
            .expect("confirm");
        assert_eq!(
            granted,
            vec![sideseat_ports::types::RawConfirmOutcome::Granted]
        );
    }

    // The age pass records the traces it is about to expire, read across both shards.
    let cutoff = Utc::now() - chrono::Duration::days(1);
    let recorded = store
        .0
        .record_age_expiry(&project, cutoff, Utc::now(), sqlite.as_ref())
        .await
        .expect("record the expiring traces");
    assert_eq!(
        recorded, 2,
        "both expiring traces are recorded, though on the far shard"
    );
    let backlog = sqlite
        .retention_cleanup_backlog(&project)
        .await
        .expect("backlog");
    assert_eq!(backlog.pending, 2);
    assert!(backlog.oldest_pending_at.is_some());

    // The rows go, as the age delete takes them.
    store
        .delete_spans(
            &project,
            &[
                ("trace-expired".to_string(), "span".to_string()),
                ("trace-stalled".to_string(), "span".to_string()),
            ],
        )
        .await
        .expect("expire the rows");

    // A replica unreachable: the release fails rather than deciding on the shard it can read, nothing is released,
    // and the owed cleanup is still there to be seen.
    if let (Ok(container), Ok(second_url)) = (
        std::env::var(TWO_SHARD_SECOND_CONTAINER_ENV),
        std::env::var(TWO_SHARD_SECOND_URL_ENV),
    ) {
        docker(&["stop", &container]);
        let refused = release_retained_raw_owners(
            sqlite.as_ref(),
            &store,
            &project,
            &["trace-stalled".to_string()],
        )
        .await;
        docker(&["start", &container]);
        assert!(
            refused.is_err(),
            "a release that cannot read every replica fails"
        );
        assert!(
            !sqlite
                .raw_owner_traces(&project, "raw-stalled")
                .await
                .expect("owners")
                .is_empty(),
            "nothing is released while a replica is unreachable"
        );
        let claimed = sqlite.claim_retention_cleanup(16, 0).await.expect("claim");
        assert_eq!(claimed.len(), 2);
        let stalled = sqlite
            .retention_cleanup_backlog(&project)
            .await
            .expect("backlog");
        assert_eq!(stalled.pending, 2, "the cleanup stays owed");
        assert!(
            stalled.most_attempts >= 1,
            "and its failed passes show: {stalled:?}"
        );
        wait_for_ping(&second_url).await;
    } else {
        eprintln!(
            "clickhouse two-shard: the unreachable-replica case skipped - set {TWO_SHARD_SECOND_CONTAINER_ENV} \
             and {TWO_SHARD_SECOND_URL_ENV}"
        );
    }

    // Every replica reachable: the expired records are released and the kept one is not.
    let traces = ["trace-expired", "trace-kept", "trace-stalled"].map(str::to_string);
    let mut released = None;
    for _ in 0..60 {
        let attempt = release_retained_raw_owners(sqlite.as_ref(), &store, &project, &traces).await;
        let done = attempt.is_ok();
        released = Some(attempt);
        if done {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    released
        .expect("tried")
        .expect("the release completes once every replica answers");
    for (raw_id, owned) in [
        ("raw-expired", false),
        ("raw-kept", true),
        ("raw-stalled", false),
    ] {
        assert_eq!(
            !sqlite
                .raw_owner_traces(&project, raw_id)
                .await
                .expect("owners")
                .is_empty(),
            owned,
            "{raw_id}"
        );
    }

    // And the expired records are collected: decided, purged, dropped.
    let decided = sqlite
        .collect_raw_records(
            &project,
            &[
                "raw-expired".to_string(),
                "raw-kept".to_string(),
                "raw-stalled".to_string(),
            ],
            Utc::now(),
        )
        .await
        .expect("collect");
    let mut collected: Vec<String> = decided.iter().map(|d| d.raw_id.clone()).collect();
    collected.sort();
    assert_eq!(collected, vec!["raw-expired", "raw-stalled"]);
    for decision in &decided {
        store
            .purge_raw_versions(&project, &decision.raw_id, decision.epoch)
            .await
            .expect("purge");
        sqlite
            .drop_raw_collection(&project, &decision.raw_id, decision.epoch)
            .await
            .expect("drop");
    }
    let mut left: Vec<String> = store
        .get_raw_records(
            &project,
            &[
                "raw-expired".to_string(),
                "raw-kept".to_string(),
                "raw-stalled".to_string(),
            ],
        )
        .await
        .expect("records")
        .into_iter()
        .map(|record| record.raw_id)
        .collect();
    left.sort();
    assert_eq!(left, vec!["raw-kept"]);
}

const TWO_REPLICA_URL_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_TWO_REPLICA_URL";
const TWO_REPLICA_SECOND_URL_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_TWO_REPLICA_SECOND_URL";

/// One shard on two replicas, the second lagging: a row acknowledged by the first is not yet on the second. A
/// retention release made through the second must still see it - it reads every replica - and keep the owner;
/// a release that read the second replica's own tables would take the owner of a row that exists.
#[tokio::test]
async fn two_replica_release_keeps_an_owner_whose_row_a_lagging_replica_lacks() {
    use sideseat_ports::traits::{RawArbiter, RawStore, release_retained_raw_owners};
    use sideseat_ports::types::RawOwnerClaim;

    let (Ok(url), Ok(second_url)) = (
        std::env::var(TWO_REPLICA_URL_ENV),
        std::env::var(TWO_REPLICA_SECOND_URL_ENV),
    ) else {
        eprintln!(
            "clickhouse two-replica: skipped - set {TWO_REPLICA_URL_ENV} and {TWO_REPLICA_SECOND_URL_ENV} (or run \
             `make test-clickhouse-two-replica`)"
        );
        return;
    };
    let database = "sideseat_two_replica_release";
    let first = replicated_backend_at(&url, database).await;
    let second = sideseat_adapter_clickhouse::ClickhouseRepository(Arc::new(
        ClickhouseService::init(
            &layout_config(&second_url, database, true),
            Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("the second replica's service"),
    ));
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let lagging = raw_client_at(&second_url, database, &user, &password).with_option(
        sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
        "1",
    );
    lagging
        .query("SYSTEM STOP FETCHES otel_spans_local")
        .execute()
        .await
        .expect("stop the second replica's fetches");

    let temp = tempfile::TempDir::new().expect("temp dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let sqlite = Arc::new(sideseat_adapter_sqlite::SqliteRepository(Arc::new(
        sideseat_adapter_sqlite::SqliteService::init(
            &storage,
            Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("sqlite"),
    )));
    let project = ProjectId::from(PROJECT);
    first
        .insert_raw_records(&[RawRecordRow {
            signal_until: Utc::now(),
            ..raw_row("lagged", 1, &["trace-l"], b"lagged")
        }])
        .await
        .expect("record");
    first
        .insert_spans(vec![NormalizedSpan {
            timestamp_start: Utc::now(),
            ingested_at: Some(Utc::now()),
            ..span_of("trace-l", "span-l", "lagged")
        }])
        .await
        .expect("a row on the first replica");
    sqlite
        .confirm_raw_owners(
            &project,
            &[RawOwnerClaim {
                raw_id: "lagged".to_string(),
                version_epoch: 0,
                trace_ids: vec!["trace-l".to_string()],
            }],
        )
        .await
        .expect("confirm");
    let lagging_rows: Vec<String> = lagging
        .query("SELECT span_id FROM otel_spans_local WHERE trace_id = 'trace-l'")
        .fetch_all()
        .await
        .expect("the lagging replica's rows");
    assert!(
        lagging_rows.is_empty(),
        "the fixture must lag: the second replica already holds the row"
    );

    let released =
        release_retained_raw_owners(sqlite.as_ref(), &second, &project, &["trace-l".to_string()])
            .await;
    lagging
        .query("SYSTEM START FETCHES otel_spans_local")
        .execute()
        .await
        .expect("restart the fetches");
    released.expect("the release");
    assert!(
        !sqlite
            .raw_owner_traces(&project, "lagged")
            .await
            .expect("owners")
            .is_empty(),
        "the row exists on a replica, so its owner stays"
    );
}
