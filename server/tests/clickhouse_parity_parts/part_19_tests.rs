// Raw-record arbiter parity: the epoch-bounded collection purge, the supersede purge, the horizon-filtered
// queue read and the spans naming a record answer the same on both backends. The protocol is
// `server/specs/RawRecordOwnership.tla` (`RPurge`, the supersede purge, `RClear`'s horizon).

/// A record's stored versions at or below a key, physically - not the latest a read resolves - so a purge that
/// removed nothing cannot pass for one that worked. Below a bound rather than all of them: ClickHouse's merge
/// folds a record's versions to its latest whenever it runs, and a mutation runs as a merge, so merges cannot
/// be stopped for the test; what a purge must remove is never the latest, and a fold leaves the latest alone.
type PhysicalVersions<'a> =
    &'a (dyn Fn(&'static str, i64) -> futures::future::BoxFuture<'static, Vec<i64>> + Sync);

fn arbiter_key(epoch: i64, n: i64) -> i64 {
    sideseat_ports::types::raw_version(epoch, n).expect("in domain")
}

async fn raw_arbiter_store_answers(
    store: &(impl AnalyticsRepository + ?Sized),
    physical: PhysicalVersions<'_>,
) -> Vec<String> {
    let key = arbiter_key;
    let project = ProjectId::from(PROJECT);
    let mut answers = Vec::new();
    let latest = |rows: Vec<RawRecordRow>| {
        rows.iter()
            .map(|row| {
                format!(
                    "{}/{}:{}",
                    row.raw_id,
                    row.version,
                    String::from_utf8_lossy(&row.record)
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    let read = |raw_id: &'static str| {
        let project = project.clone();
        async move {
            store
                .get_raw_records(&project, &[raw_id.to_string()])
                .await
                .expect("read")
        }
    };

    store
        .append_raw_records(&[
            raw_row("purged", key(0, 1), &["trace-p"], b"first"),
            raw_row("purged", key(0, 3), &["trace-p"], b"repair"),
            raw_row("purged", key(2, 1), &["trace-p"], b"after-the-decision"),
            raw_row("superseded", key(0, 1), &["trace-s"], b"with-deleted"),
            raw_row("superseded", key(0, 2), &["trace-s"], b"rewrite"),
            raw_row("superseded", key(1, 1), &["trace-s"], b"written-since"),
            raw_row("lonely", key(0, 1), &["trace-l"], b"only-version"),
        ])
        .await
        .expect("append");

    store
        .purge_raw_versions(&project, "purged", 1)
        .await
        .expect("purge");
    answers.push(format!(
        "stored at or below epoch 1 after its purge: {:?}",
        physical("purged", key(1, 0xFFFF_FFFF)).await
    ));
    answers.push(format!(
        "latest after the epoch-1 purge: {}",
        latest(read("purged").await)
    ));

    store
        .delete_superseded_raw_versions(&project, "superseded", key(0, 2))
        .await
        .expect("supersede");
    answers.push(format!(
        "stored below the rewrite after the supersede purge: {:?}",
        physical("superseded", key(0, 1)).await
    ));
    // A record whose only version is below the bound: no merge can fold it away, so its absence afterwards is
    // the supersede purge's doing and nothing else's.
    store
        .delete_superseded_raw_versions(&project, "lonely", key(0, 2))
        .await
        .expect("supersede the only version");
    answers.push(format!(
        "stored of a record superseded entirely: {:?}",
        physical("lonely", i64::MAX).await
    ));
    store
        .purge_raw_versions(&project, "superseded", -1)
        .await
        .expect("a purge bounded below every epoch reaches nothing");
    answers.push(format!(
        "latest after the supersede purge: {}",
        latest(read("superseded").await)
    ));
    store
        .purge_raw_versions(&project, "superseded", 0)
        .await
        .expect("purge epoch 0");
    answers.push(format!(
        "stored at or below epoch 0 after its purge: {:?}",
        physical("superseded", key(0, 0xFFFF_FFFF)).await
    ));
    answers.push(format!(
        "latest after purging epoch 0: {}",
        latest(read("superseded").await)
    ));
    store
        .purge_raw_versions(&project, "purged", 2)
        .await
        .expect("the purge of the last version");
    answers.push(format!(
        "stored after purging the last epoch: {:?}",
        physical("purged", i64::MAX).await
    ));

    let earlier = store
        .pending_raw_records(
            usize::MAX,
            Some(chrono::Utc::now() - chrono::Duration::hours(1)),
        )
        .await
        .expect("queue before the horizon");
    let due = store
        .pending_raw_records(usize::MAX, None)
        .await
        .expect("queue at horizon 0");
    let mut due_ids: Vec<String> = due.iter().map(|entry| entry.raw_id.clone()).collect();
    due_ids.sort_unstable();
    due_ids.dedup();
    answers.push(format!(
        "entries enqueued an hour ago: {}; due at horizon 0: {due_ids:?}",
        earlier.len()
    ));
    answers
}

#[tokio::test]
async fn the_raw_arbiter_s_store_operations_match_across_backends() {
    let (_temp, duckdb) = duckdb_backend().await;
    let embedded_service = duckdb.0.clone();
    let embedded_physical = move |raw_id: &'static str,
                                  at_most: i64|
          -> futures::future::BoxFuture<'static, Vec<i64>> {
        let service = embedded_service.clone();
        Box::pin(async move {
            let conn = service.conn();
            // The bound is an integer, written into the statement; the id is bound.
            let mut statement = conn
                .prepare(&format!(
                    "SELECT version_key FROM otel_raw WHERE raw_id = ? AND version_key <= {at_most} \
                     ORDER BY version_key"
                ))
                .expect("prepare");
            statement
                .query_map([raw_id], |row| row.get(0))
                .expect("query")
                .collect::<Result<_, _>>()
                .expect("rows")
        })
    };
    let embedded = raw_arbiter_store_answers(&duckdb, &embedded_physical).await;
    let named = arbiter_key(2, 1);
    let since = arbiter_key(1, 1);
    for expected in [
        "stored at or below epoch 1 after its purge: []".to_string(),
        format!("latest after the epoch-1 purge: purged/{named}:after-the-decision"),
        "stored below the rewrite after the supersede purge: []".to_string(),
        format!("latest after the supersede purge: superseded/{since}:written-since"),
        "stored at or below epoch 0 after its purge: []".to_string(),
        format!("latest after purging epoch 0: superseded/{since}:written-since"),
        "stored after purging the last epoch: []".to_string(),
        "stored of a record superseded entirely: []".to_string(),
        "entries enqueued an hour ago: 0; due at horizon 0: [\"lonely\", \"purged\", \"superseded\"]"
            .to_string(),
    ] {
        assert!(
            embedded.contains(&expected),
            "missing `{expected}`: {embedded:?}"
        );
    }
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_raw_arbiter";
    let clickhouse = clickhouse_backend(&url, database).await;
    let client = raw_client(&url, database);
    let distributed_physical = move |raw_id: &'static str,
                                     at_most: i64|
          -> futures::future::BoxFuture<'static, Vec<i64>> {
        let client = client.clone();
        Box::pin(async move {
            client
                .query(
                    "SELECT version_key FROM otel_raw WHERE raw_id = ? AND version_key <= ? \
                         ORDER BY version_key",
                )
                .bind(raw_id)
                .bind(at_most)
                .fetch_all::<i64>()
                .await
                .expect("physical versions")
        })
    };
    let distributed = raw_arbiter_store_answers(&clickhouse, &distributed_physical).await;
    assert_eq!(
        embedded, distributed,
        "the raw arbiter's store operations differ between the backends"
    );
}

/// A store whose raw records still carry receipt-instant versions in a `version` column - one created before the
/// arbiter keyed versions by epoch - is at the supported version, so the version check accepts it, and read as
/// keys its versions would sort as epochs no decision reached, out of every purge's bound. It is refused at
/// startup instead, with the reset named.
#[tokio::test]
async fn a_store_with_receipt_keyed_raw_versions_is_refused_at_startup() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_raw_unkeyed";
    let _created = clickhouse_backend(&url, database).await;
    let client = raw_client(&url, database);
    // ClickHouse forbids renaming a ReplacingMergeTree's version column, so the earlier table is rebuilt.
    for statement in [
        "CREATE TABLE otel_raw_receipt_keyed (project_id String, raw_id String, signal String, \
         received_at DateTime64(6), origin String, version Int64, signal_until DateTime64(6), \
         hold_until Nullable(DateTime64(6)), record String) \
         ENGINE = ReplacingMergeTree(version) ORDER BY (project_id, raw_id)",
        "DROP TABLE otel_raw SYNC",
        "RENAME TABLE otel_raw_receipt_keyed TO otel_raw",
    ] {
        client.query(statement).execute().await.expect(statement);
    }
    let refused = match ClickhouseService::init(
        &layout_config(&url, database, false),
        std::sync::Arc::new(sideseat_server::runtime::clock::SystemClock),
    )
    .await
    {
        Ok(_) => None,
        Err(error) => Some(error.to_string()),
    };
    let message = refused.expect("a store with receipt-keyed raw versions started");
    assert!(
        message.contains("keys its versions by receipt instant")
            && message.contains("drop the configured database"),
        "the refusal names the layout and the reset: {message}"
    );
}
