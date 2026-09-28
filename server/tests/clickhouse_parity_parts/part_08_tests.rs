
/// The v2 → v3 migration applied to a **replicated** database, which is where its hardest mechanics live.
///
/// `make test-clickhouse` starts a plain server and the single-node helper sets `distributed: false`, so every
/// other migration assertion in this file runs against `ReplacingMergeTree` tables with no `ON CLUSTER`, no
/// `Distributed` front ends and no Keeper paths. Four things are therefore untested there, and all four are
/// specific to the production shape:
///
/// - the **`distributed_statements`** catch-up ALTERs. A `Distributed` table is created `AS otel_x_local`, which
///   copies the structure *once* and does not follow later changes - so a column added to the local table is
///   absent from the front end, and the first insert through it fails. Every one of those seven ALTERs was
///   unreachable from a `distributed: false` test, including the `datapoint_id` the rebuild's own `ORDER BY`
///   names.
/// - **`{uuid}` Keeper paths.** `CREATE TABLE ... AS <old>` copies the engine including its Keeper path, and two
///   tables cannot share one, so the replacement needs a path of its own; `EXCHANGE TABLES` then swaps names
///   while UUIDs stay with their tables.
/// - **`EXCHANGE TABLES ... ON CLUSTER`**, which is atomic per host rather than across the cluster.
/// - the `Replicated*` engine argument itself, which `{replacement_engine}` renders differently per mode.
///
/// Asserted through a **write and a read back**, not only through metadata: a column present on the local table
/// and absent from the `Distributed` front end reads as success in `system.columns` for the table the test
/// happens to look at, and fails at the first insert. That is exactly the shape this gate exists to catch.
///
/// **Stated gap: one replica.** This covers everything structurally different about distributed mode, and not
/// cross-replica convergence, which needs a second node. Naming that is the point - the plan requires
/// crash-idempotent convergence across hosts, and one host cannot demonstrate it.
#[tokio::test]
async fn the_migration_applies_to_a_replicated_database() {
    let Ok(url) = std::env::var(REPLICATED_URL_ENV) else {
        eprintln!(
            "clickhouse replicated: skipped - set {REPLICATED_URL_ENV} (or run \
             `make test-clickhouse-replicated`)"
        );
        return;
    };

    let database = "sideseat_repl_migrate";
    let service = replicated_backend(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let client = raw_client_at(&url, database, &user, &password).with_option(
        sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
        "1",
    );

    // Reverse v3 to released v2, on the local tables **and** on the `Distributed` front ends - the split is
    // the thing under test, since a front end is created `AS <local>` once and does not follow later changes.
    //
    // The metrics and spans reversals are copy-swaps rather than `ALTER ... DROP COLUMN`, and that is forced
    // rather than stylistic: `datapoint_id` is in the v3 sorting key, so dropping it is `UNKNOWN_IDENTIFIER`
    // against the key itself. The same reason the forward migration is a rebuild.
    //
    // Each replacement gets its **own Keeper path**, because `CREATE TABLE ... AS <old>` copies the engine
    // including its path and two tables cannot share one.
    let cluster = REPLICATED_CLUSTER;
    for statement in [
        // -- spans back to the v2 key, without the v3 columns and index -----------------------------------
        format!("DROP TABLE IF EXISTS otel_spans_v2_local ON CLUSTER {cluster} SYNC"),
        format!(
            "CREATE TABLE otel_spans_v2_local ON CLUSTER {cluster} AS otel_spans_local \
             ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{{uuid}}/spans_v2', \
             '{{replica}}', ingested_at) \
             PARTITION BY toYYYYMM(timestamp_start) \
             ORDER BY (project_id, toDate(timestamp_start), trace_id, span_id)"
        ),
        format!(
            "ALTER TABLE otel_spans_v2_local ON CLUSTER {cluster} DROP INDEX IF EXISTS idx_ingested_at"
        ),
        format!(
            "ALTER TABLE otel_spans_v2_local ON CLUSTER {cluster} DROP COLUMN IF EXISTS scope_name"
        ),
        format!(
            "ALTER TABLE otel_spans_v2_local ON CLUSTER {cluster} DROP COLUMN IF EXISTS scope_version"
        ),
        "INSERT INTO otel_spans_v2_local SELECT * EXCEPT (scope_name, scope_version) \
         FROM otel_spans_local"
            .to_string(),
        format!("EXCHANGE TABLES otel_spans_local AND otel_spans_v2_local ON CLUSTER {cluster}"),
        format!("DROP TABLE IF EXISTS otel_spans_v2_local ON CLUSTER {cluster} SYNC"),
        // -- metrics back to the v2 key and engine ---------------------------------------------------------
        format!("DROP TABLE IF EXISTS otel_metrics_v2_local ON CLUSTER {cluster} SYNC"),
        format!(
            "CREATE TABLE otel_metrics_v2_local ON CLUSTER {cluster} AS otel_metrics_local \
             ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{{uuid}}/metrics_v2', \
             '{{replica}}') \
             PARTITION BY toYYYYMM(timestamp) \
             ORDER BY (project_id, metric_name, toDate(timestamp), timestamp)"
        ),
        format!(
            "ALTER TABLE otel_metrics_v2_local ON CLUSTER {cluster} DROP COLUMN IF EXISTS ingested_at"
        ),
        format!(
            "ALTER TABLE otel_metrics_v2_local ON CLUSTER {cluster} DROP COLUMN IF EXISTS datapoint_id"
        ),
        format!(
            "ALTER TABLE otel_metrics_v2_local ON CLUSTER {cluster} DROP COLUMN IF EXISTS scope_attributes"
        ),
        format!(
            "ALTER TABLE otel_metrics_v2_local ON CLUSTER {cluster} DROP COLUMN IF EXISTS scope_schema_url"
        ),
        format!(
            "ALTER TABLE otel_metrics_v2_local ON CLUSTER {cluster} DROP COLUMN IF EXISTS resource_schema_url"
        ),
        format!(
            "ALTER TABLE otel_metrics_v2_local ON CLUSTER {cluster} DROP COLUMN IF EXISTS exemplars"
        ),
        "INSERT INTO otel_metrics_v2_local SELECT * EXCEPT (ingested_at, datapoint_id, \
         scope_attributes, scope_schema_url, resource_schema_url, exemplars) FROM otel_metrics_local"
            .to_string(),
        // Same reasoning as the spans: the released path, freed and reoccupied, so a replacement reusing it
        // fails here as it would in production.
        format!("DROP TABLE IF EXISTS otel_metrics_local ON CLUSTER {cluster} SYNC"),
        format!(
            "CREATE TABLE otel_metrics_local ON CLUSTER {cluster} AS otel_metrics_v2_local \
             ENGINE = ReplicatedReplacingMergeTree( \
                 '/clickhouse/tables/{{shard}}/{database}/otel_metrics', '{{replica}}') \
             PARTITION BY toYYYYMM(timestamp) \
             ORDER BY (project_id, metric_name, toDate(timestamp), timestamp)"
        ),
        "INSERT INTO otel_metrics_local SELECT * FROM otel_metrics_v2_local".to_string(),
        format!("DROP TABLE IF EXISTS otel_metrics_v2_local ON CLUSTER {cluster} SYNC"),
        // -- and the front ends, which is the half a `distributed: false` test can never reach -------------
        format!("ALTER TABLE otel_metrics ON CLUSTER {cluster} DROP COLUMN IF EXISTS ingested_at"),
        format!("ALTER TABLE otel_metrics ON CLUSTER {cluster} DROP COLUMN IF EXISTS datapoint_id"),
        format!(
            "ALTER TABLE otel_metrics ON CLUSTER {cluster} DROP COLUMN IF EXISTS scope_attributes"
        ),
        format!(
            "ALTER TABLE otel_metrics ON CLUSTER {cluster} DROP COLUMN IF EXISTS scope_schema_url"
        ),
        format!(
            "ALTER TABLE otel_metrics ON CLUSTER {cluster} DROP COLUMN IF EXISTS resource_schema_url"
        ),
        format!("ALTER TABLE otel_metrics ON CLUSTER {cluster} DROP COLUMN IF EXISTS exemplars"),
        format!("ALTER TABLE otel_spans ON CLUSTER {cluster} DROP COLUMN IF EXISTS scope_name"),
        format!("ALTER TABLE otel_spans ON CLUSTER {cluster} DROP COLUMN IF EXISTS scope_version"),
    ] {
        client
            .query(&statement)
            .execute()
            .await
            .unwrap_or_else(|e| panic!("reverting to v2 ({statement}): {e}"));
    }

    service
        .apply_migration_for_test(3)
        .await
        .expect("the migration must apply to a replicated database");

    // The write is the assertion. It goes through the `Distributed` front end - which is what the insert path
    // uses - so a column the local table has and the front end does not fails here and nowhere else.
    service
        .insert_metrics(&[NormalizedMetric {
            project_id: Some(PROJECT.to_string()),
            metric_name: "replicated.histogram".to_string(),
            metric_type: MetricType::Histogram,
            aggregation_temporality: AggregationTemporality::Cumulative,
            timestamp: ts(1),
            datapoint_id: "replicated-1".to_string(),
            exemplars: serde_json::json!([{"trace_id": "bb", "value_double": 2.0}]),
            ..Default::default()
        }])
        .await
        .expect("an upgraded replicated schema must accept a row through the Distributed table");

    let stored: Vec<String> = client
        .query("SELECT coalesce(exemplars, '') FROM otel_metrics WHERE datapoint_id = ? LIMIT 1")
        .bind("replicated-1")
        .fetch_all()
        .await
        .expect("read back through the Distributed table");
    assert_eq!(stored.len(), 1, "the row is readable through the front end");
    assert!(
        stored[0].contains("trace_id"),
        "the migrated column holds what was written, got {:?}",
        stored[0]
    );

    // The local table carries the new sorting key, and the replacement tables are gone.
    let keys: Vec<String> = client
        .query(
            "SELECT sorting_key FROM system.tables \
             WHERE database = currentDatabase() AND name = 'otel_spans_local'",
        )
        .fetch_all()
        .await
        .expect("read the local sorting key");
    assert_eq!(keys.len(), 1);
    assert!(
        !keys[0].contains("toDate("),
        "the replicated local table kept the date expression in its sorting key, got {:?}",
        keys[0]
    );

    let leftovers: Vec<String> = client
        .query(
            "SELECT name FROM system.tables WHERE database = currentDatabase() \
             AND name LIKE '%_v3%' ORDER BY name",
        )
        .fetch_all()
        .await
        .expect("look for leftovers");
    assert!(
        leftovers.is_empty(),
        "replacement tables survived the replicated migration: {leftovers:?}"
    );
}

/// The replicated migration, **interrupted and resumed**, which is what its idempotence is for.
///
/// `EXCHANGE TABLES ... ON CLUSTER` is atomic per host, so a migrator that crashes partway leaves a database
/// whose live tables are already correct and whose replacements are still there. A blind re-run must finish the
/// job rather than toggle the exchange back - which is the failure mode the plan calls out for the replicated
/// case specifically, and which no single-node test can reach because the `ON CLUSTER` path is not taken there.
///
/// The interruption is simulated by applying the statements up to and including the exchange and then stopping,
/// which is the state a crash produces. Then the whole migration is re-run.
#[tokio::test]
async fn an_interrupted_replicated_migration_resumes() {
    let Ok(url) = std::env::var(REPLICATED_URL_ENV) else {
        eprintln!(
            "clickhouse replicated: skipped - set {REPLICATED_URL_ENV} (or run \
             `make test-clickhouse-replicated`)"
        );
        return;
    };

    let database = "sideseat_repl_resume";
    let service = replicated_backend(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let client = raw_client_at(&url, database, &user, &password).with_option(
        sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
        "1",
    );
    let cluster = REPLICATED_CLUSTER;

    // A row on the live (v3) table, so "the re-run did not exchange the leftover back" is checkable against
    // data rather than only against metadata.
    service
        .insert_spans(vec![NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: "live-trace".to_string(),
            span_id: "live-span".to_string(),
            span_name: "current".to_string(),
            timestamp_start: ts(1),
            timestamp_end: Some(ts(1)),
            ..Default::default()
        }])
        .await
        .expect("seed the live table");

    // The state a crash **actually** produces: `EXCHANGE TABLES` has run, so the live table is already v3 and
    // the table wearing the `_v3` name is the *old, populated, v2-shaped* one waiting to be dropped.
    //
    // A first version staged empty v3-shaped tables instead, which is not a state this migration can reach and
    // is the weaker fixture in the way that matters: a blind re-run that exchanged the leftover back would put
    // a v2 sorting key and stale data live, and against a v3-shaped empty leftover that damage is invisible.
    // Only one leftover exists at a time here, because the rebuild is sequential - spans, then metrics - so
    // staging both was also wrong about the shape of the failure.
    for statement in [
        format!(
            "CREATE TABLE otel_spans_v3_local ON CLUSTER {cluster} AS otel_spans_local \
             ENGINE = ReplicatedReplacingMergeTree( \
                 '/clickhouse/tables/{{shard}}/{{uuid}}/spans_leftover', '{{replica}}', ingested_at) \
             PARTITION BY toYYYYMM(timestamp_start) \
             ORDER BY (project_id, toDate(timestamp_start), trace_id, span_id)"
        ),
        // Stale content, so exchanging it back would be observable.
        format!(
            "INSERT INTO otel_spans_v3_local (project_id, trace_id, span_id, span_name, timestamp_start, \
             ingested_at) VALUES ('{PROJECT}', 'stale-trace', 'stale-span', 'obsolete', now64(6), now64(6))"
        ),
    ] {
        client
            .query(&statement)
            .execute()
            .await
            .unwrap_or_else(|e| panic!("staging the interrupted state ({statement}): {e}"));
    }

    let leftover_key: Vec<String> = client
        .query(
            "SELECT sorting_key FROM system.tables \
             WHERE database = currentDatabase() AND name = 'otel_spans_v3_local'",
        )
        .fetch_all()
        .await
        .expect("read the leftover's key");
    assert!(
        leftover_key[0].contains("toDate("),
        "the leftover must carry the *old* key, or this fixture is not the state a crash produces"
    );

    // A blind re-run, as a restarting instance performs.
    service
        .apply_migration_for_test(3)
        .await
        .expect("a re-run over an interrupted replicated migration must succeed");

    let leftovers: Vec<String> = client
        .query(
            "SELECT name FROM system.tables WHERE database = currentDatabase() \
             AND name LIKE '%_v3%' ORDER BY name",
        )
        .fetch_all()
        .await
        .expect("look for leftovers");
    assert!(
        leftovers.is_empty(),
        "the resumed migration left replacement tables behind, so the old tables are never reclaimed and the \
         storage of the two largest tables stays doubled: {leftovers:?}"
    );

    // It did not exchange the leftover back, which is the other way a blind re-run goes wrong - and with a
    // populated v2-shaped leftover that is checkable against both the key and the data.
    let keys: Vec<String> = client
        .query(
            "SELECT sorting_key FROM system.tables \
             WHERE database = currentDatabase() AND name = 'otel_spans_local'",
        )
        .fetch_all()
        .await
        .expect("read the local sorting key");
    assert_eq!(keys.len(), 1);
    assert!(
        !keys[0].contains("toDate("),
        "the re-run exchanged the old table back in, so the date expression is live again: {:?}",
        keys[0]
    );

    let trace_ids: Vec<String> = client
        .query("SELECT DISTINCT trace_id FROM otel_spans ORDER BY trace_id")
        .fetch_all()
        .await
        .expect("read the live rows");
    assert!(
        trace_ids.iter().any(|t| t == "live-trace"),
        "the live table's own row is gone after the re-run: {trace_ids:?}"
    );
    assert!(
        !trace_ids.iter().any(|t| t == "stale-trace"),
        "the leftover's stale row is live, so the re-run exchanged the obsolete table back in: {trace_ids:?}"
    );

    service
        .insert_spans(vec![NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: "resumed-trace".to_string(),
            span_id: "resumed-span".to_string(),
            span_name: "after-resume".to_string(),
            timestamp_start: ts(1),
            timestamp_end: Some(ts(1)),
            ..Default::default()
        }])
        .await
        .expect("the resumed database accepts a write through the Distributed table");
}

/// A re-delivery that moves a trace to another session must move it on both backends.
///
/// `otel_spans` is append-only, so the old row survives. DuckDB resolved session membership from that raw
/// table while reading the messages from the deduplicated view, so the trace stayed in its *old* session
/// while returning its *current* content - a session reporting a trace that no longer belongs to it, and the
/// same trace listed under two sessions at once. ClickHouse read the subquery with `FINAL` and was correct,
/// so the two backends disagreed about the same data with nothing to say which was right.
#[tokio::test]
async fn a_redelivery_that_changes_the_session_moves_the_trace_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_session_move").await;

    let payload = |text: &str| {
        Some(
            serde_json::json!([{
                "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "user", "content": text}
            }])
            .to_string(),
        )
    };

    let base = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "trace-moved".to_string(),
        span_id: "span-1".to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        span_category: Some(SpanCategory::LLM),
        timestamp_start: ts(0),
        timestamp_end: Some(ts(1)),
        duration_ms: 1000,
        status_code: Some("OK".to_string()),
        environment: Some("test".to_string()),
        ..Default::default()
    };

    let first = NormalizedSpan {
        session_id: Some("session-old".to_string()),
        messages: payload("the first attempt"),
        ingested_at: Some(ts(10)),
        ..base.clone()
    };
    let corrected = NormalizedSpan {
        session_id: Some("session-new".to_string()),
        messages: payload("the corrected answer"),
        ingested_at: Some(ts(20)),
        ..base
    };

    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        repo.insert_spans(vec![first.clone()]).await.expect("first");
        repo.insert_spans(vec![corrected.clone()])
            .await
            .expect("corrected");
    }

    async fn rows_for(repo: &dyn AnalyticsRepository, session: &str) -> Vec<MessageSpanRow> {
        let params = MessageQueryParams {
            project_id: ProjectId::from(PROJECT),
            session_id: Some(session.to_string()),
            ..Default::default()
        };
        repo.get_messages(&params).await.expect("messages").rows
    }

    for (label, repo) in [
        ("duckdb", &duck as &dyn AnalyticsRepository),
        ("clickhouse", &ch),
    ] {
        let old = rows_for(repo, "session-old").await;
        assert!(
            old.is_empty(),
            "{label}: the trace moved to another session, so its old session must be empty; got {} row(s)",
            old.len()
        );

        let new = rows_for(repo, "session-new").await;
        assert_eq!(
            new.len(),
            1,
            "{label}: the current session must hold the trace"
        );
        assert!(
            new[0].messages_json.contains("the corrected answer"),
            "{label}: and hand the pipeline the corrected content"
        );
    }
}
