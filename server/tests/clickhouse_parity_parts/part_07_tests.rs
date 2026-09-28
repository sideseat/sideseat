
/// Every `MIGRATIONS` entry actually runs against a real ClickHouse, from the state it exists to upgrade.
///
/// A migration is the code most likely to be wrong and least likely to be exercised: a fresh database never
/// touches it, so `make test-clickhouse` can be green while the statement is rejected by every server it
/// exists to serve. DuckDB's equivalent guard found three defects in one migration this way - an `ALTER`
/// ClickHouse-equivalent refusal among them - and none was visible from a fresh schema.
///
/// The prior state is built by *reversing* what each migration adds, so the test needs no captured old
/// schema to drift out of date. Then the runner is invoked exactly as a startup upgrade would invoke it,
/// and the result has to accept a row naming the new column - which is the property that matters, since a
/// column present in the table but absent from the `Distributed` front end reads as success here and fails
/// at the first insert.
#[tokio::test]
async fn every_clickhouse_migration_applies_to_the_state_it_upgrades() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let database = "sideseat_parity_migrations";
    let service = clickhouse_backend(&url, database).await;
    let client = raw_client(&url, database).with_option(
        sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
        "1",
    );

    // The reverse of each migration, so a v3 migration is applied to a v2-shaped table. Kept beside the
    // migration list rather than as a captured schema dump: whoever adds a migration adds its inverse here
    // and the test keeps working, and forgetting to fails loudly on the next line.
    // Several statements per version, because reverting a *rebuild* takes more than one: v3 changed the
    // span sorting key and gave metrics a version column, and neither can be undone by an `ALTER` any more
    // than it could be applied by one. Single-node shapes, which is what this test runs.
    let undo: &[(i32, &[&str])] = &[
        (
            3,
            &[
                // The test starts from the current fresh schema. A v3 source predates every later migration,
                // so remove their additions before reversing v3 itself.
                "DROP TABLE IF EXISTS otel_logs SYNC",
                "ALTER TABLE otel_spans MODIFY TTL timestamp_start + INTERVAL 90 DAY DELETE",
                "ALTER TABLE otel_metrics MODIFY TTL timestamp + INTERVAL 90 DAY DELETE",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS content_digest",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS hold_until",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS logical_bytes",
                "ALTER TABLE otel_metrics DROP COLUMN IF EXISTS content_digest",
                "ALTER TABLE otel_metrics DROP COLUMN IF EXISTS hold_until",
                "ALTER TABLE otel_metrics DROP COLUMN IF EXISTS logical_bytes",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_prompt",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_completion",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_tool_name",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_tool_args",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_error",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_span_name",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_indexed",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_prompt",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_prompt_truncated",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_completion",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_completion_truncated",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_tool_name",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_tool_name_truncated",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_tool_args",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_tool_args_truncated",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_error",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_error_truncated",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_span_name",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_span_name_truncated",
                // The anomaly table is dropped, because released v2 has none - it arrives with v3. Leaving it in
                // place let the migration's `CREATE TABLE IF NOT EXISTS` be deleted with this test green: a fresh
                // database already has the table, so nothing here would notice, and a *real* v2 upgrade would then
                // record v3 without it and every scheduled consistency pass would fail with `UNKNOWN_TABLE`.
                "DROP TABLE IF EXISTS span_partition_anomalies SYNC",
                // The skip index has to go too, and forgetting it was the same defect in a third place: v3 adds
                // `idx_ingested_at`, so a "v2" reconstructed by reversing only the *columns* keeps an index that
                // released v2 never had - and the migration's `ADD INDEX` could then be deleted with this test
                // still green, while a production upgrade lost the index the consistency check depends on to
                // avoid a corpus-scale scan. Dropped before the rebuild, since `CREATE TABLE ... AS` copies
                // indexes.
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_ingested_at",
                // Spans back to the v2 sorting key, with the date expression in it.
                "DROP TABLE IF EXISTS otel_spans_v2 SYNC",
                "CREATE TABLE otel_spans_v2 AS otel_spans ENGINE = ReplacingMergeTree(ingested_at) \
             PARTITION BY toYYYYMM(timestamp_start) \
             ORDER BY (project_id, toDate(timestamp_start), trace_id, span_id)",
                "INSERT INTO otel_spans_v2 SELECT * FROM otel_spans",
                "EXCHANGE TABLES otel_spans AND otel_spans_v2",
                "DROP TABLE IF EXISTS otel_spans_v2 SYNC",
                // Metrics back to an engine with no version argument, and without the columns a *released* v2
                // lacks. The released v1.0.13 schema declares version 2 and has no `datapoint_id`,
                // `scope_attributes`, `scope_schema_url`, `resource_schema_url` or `exemplars` - they were added
                // to the fresh schema later without the version being bumped. Reversing only what *this*
                // migration adds reconstructed a "v2" that still had them, so the test passed against a shape no
                // real database has while v3 would have failed on every real one with `UNKNOWN_IDENTIFIER`.
                "ALTER TABLE otel_metrics DROP COLUMN IF EXISTS scope_attributes",
                "ALTER TABLE otel_metrics DROP COLUMN IF EXISTS scope_schema_url",
                "ALTER TABLE otel_metrics DROP COLUMN IF EXISTS resource_schema_url",
                "ALTER TABLE otel_metrics DROP COLUMN IF EXISTS exemplars",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS scope_name",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS scope_version",
                "DROP TABLE IF EXISTS otel_metrics_v2 SYNC",
                "CREATE TABLE otel_metrics_v2 AS otel_metrics ENGINE = ReplacingMergeTree() \
             PARTITION BY toYYYYMM(timestamp) \
             ORDER BY (project_id, metric_name, toDate(timestamp), timestamp)",
                "ALTER TABLE otel_metrics_v2 DROP COLUMN ingested_at",
                "ALTER TABLE otel_metrics_v2 DROP COLUMN IF EXISTS datapoint_id",
                "INSERT INTO otel_metrics_v2 SELECT * EXCEPT (ingested_at, datapoint_id) FROM otel_metrics",
                "EXCHANGE TABLES otel_metrics AND otel_metrics_v2",
                "DROP TABLE IF EXISTS otel_metrics_v2 SYNC",
            ],
        ),
        (
            4,
            &[
                "DROP TABLE IF EXISTS otel_logs SYNC",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS content_digest",
                "ALTER TABLE otel_metrics DROP COLUMN IF EXISTS content_digest",
            ],
        ),
        (
            5,
            &[
                "ALTER TABLE otel_spans MODIFY TTL timestamp_start + INTERVAL 90 DAY DELETE",
                "ALTER TABLE otel_metrics MODIFY TTL timestamp + INTERVAL 90 DAY DELETE",
                "ALTER TABLE otel_logs MODIFY TTL timestamp + INTERVAL 90 DAY DELETE",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS hold_until",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS logical_bytes",
                "ALTER TABLE otel_metrics DROP COLUMN IF EXISTS hold_until",
                "ALTER TABLE otel_metrics DROP COLUMN IF EXISTS logical_bytes",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS hold_until",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS logical_bytes",
            ],
        ),
        (
            6,
            &[
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_prompt",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_completion",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_tool_name",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_tool_args",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_error",
                "ALTER TABLE otel_spans DROP INDEX IF EXISTS idx_search_span_name",
                "ALTER TABLE otel_logs DROP INDEX IF EXISTS idx_search_body",
                "ALTER TABLE otel_logs DROP INDEX IF EXISTS idx_search_event_name",
                "ALTER TABLE otel_logs DROP INDEX IF EXISTS idx_search_severity",
                "ALTER TABLE otel_logs DROP INDEX IF EXISTS idx_search_attributes",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_indexed",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS search_indexed",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_prompt",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_prompt_truncated",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_completion",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_completion_truncated",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_tool_name",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_tool_name_truncated",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_tool_args",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_tool_args_truncated",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_error",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_error_truncated",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_span_name",
                "ALTER TABLE otel_spans DROP COLUMN IF EXISTS search_span_name_truncated",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS search_body",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS search_body_truncated",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS search_event_name",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS search_event_name_truncated",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS search_severity",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS search_severity_truncated",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS search_attributes",
                "ALTER TABLE otel_logs DROP COLUMN IF EXISTS search_attributes_truncated",
            ],
        ),
        (
            7,
            &[
                "DROP ROW POLICY IF EXISTS sideseat_tenant_filter ON otel_spans",
                "DROP ROW POLICY IF EXISTS sideseat_tenant_filter ON otel_metrics",
                "DROP ROW POLICY IF EXISTS sideseat_tenant_filter ON otel_logs",
                "DROP ROW POLICY IF EXISTS sideseat_tenant_filter ON span_partition_anomalies",
            ],
        ),
    ];

    for migration in sideseat_adapter_clickhouse::schema::MIGRATIONS {
        let version = migration.version;
        let (_, revert) = undo
            .iter()
            .find(|(v, _)| *v == version)
            .unwrap_or_else(|| panic!(
                "migration v{version} ({}) has no inverse in this test, so it is never applied to the \
                 state it upgrades - add one",
                migration.name
            ));

        for statement in *revert {
            client
                .query(statement)
                .execute()
                .await
                .unwrap_or_else(|e| panic!("reverting v{version} ({statement}): {e}"));
        }

        // **The reconstructed state is checked against the *released* shape, not trusted.** The `undo` above
        // reverses what each migration adds to the *current* schema, which is only released v2 while v3 is the
        // newest migration: once a v4 exists this loop would test v3 against fresh-v4-minus-v3, and a v3
        // regression depending on a v4 column would pass while a real upgrade failed. Comparing against a
        // recorded fact about v1.0.13 is what makes the fixture's claim checkable rather than assumed.
        if version == 3 {
            for (table, expected) in [
                ("otel_spans", released_v2::RELEASED_V2_SPANS),
                ("otel_metrics", released_v2::RELEASED_V2_METRICS),
            ] {
                // Names **and types**: comparing names alone let the reconstructed database differ in type,
                // nullability or width while passing, and a migration applied to a source whose types are wrong
                // is being tested against a schema no database has.
                let mut actual: Vec<(String, String)> = client
                    .query(
                        "SELECT name, type FROM system.columns \
                         WHERE database = currentDatabase() AND table = ? ORDER BY name",
                    )
                    .bind(table)
                    .fetch_all()
                    .await
                    .expect("read the reduced columns");
                actual.sort();
                // `Decimal64(S)` is what the released schema *declares*; `system.columns` reports the canonical
                // `Decimal(18, S)`, which is the same type. Normalised here rather than in the snapshot so that
                // file stays a faithful transcription of the release - the whole point of it being a recorded
                // fact. Only this one alias is mapped: an unrecognised spelling should fail rather than be
                // massaged into agreement.
                let mut want: Vec<(String, String)> = expected
                    .iter()
                    .map(|(n, t)| {
                        let canonical = match *t {
                            "Decimal64(6)" => "Decimal(18, 6)".to_string(),
                            other => other.to_string(),
                        };
                        (n.to_string(), canonical)
                    })
                    .collect();
                want.sort();
                assert_eq!(
                    actual, want,
                    "the reduced {table} is not what released v2 declared, so this migration is being applied \
                     to a state no database has. Either the undo above is incomplete, or it is reversing \
                     against a schema newer than v3"
                );
            }
        }

        service
            .apply_migration_for_test(version)
            .await
            .unwrap_or_else(|e| panic!("applying v{version}: {e}"));
    }

    // A write naming every column the current build knows about. This is what a schema that is present but
    // not reachable - the Distributed front-end case - fails on, and nothing before this line would.
    let metric = NormalizedMetric {
        project_id: Some(PROJECT.to_string()),
        metric_name: "migrated.histogram".to_string(),
        metric_type: MetricType::Histogram,
        aggregation_temporality: AggregationTemporality::Cumulative,
        timestamp: ts(1),
        datapoint_id: "migrated-1".to_string(),
        exemplars: serde_json::json!([{"trace_id": "aa", "value_double": 1.0}]),
        ..Default::default()
    };
    service
        .insert_metrics(&[metric])
        .await
        .expect("an upgraded schema must accept a row naming the added column");

    // `coalesce`, because a bare `Nullable(String)` has no `Row` impl to deserialise into.
    let stored: Vec<String> = client
        .query("SELECT coalesce(exemplars, '') FROM otel_metrics WHERE datapoint_id = ? LIMIT 1")
        .bind("migrated-1")
        .fetch_all()
        .await
        .expect("read back");
    assert_eq!(stored.len(), 1);
    assert!(
        stored[0].contains("trace_id"),
        "the migrated column must hold what was written, got {:?}",
        stored[0]
    );

    // The skip index the consistency check needs, asserted on the upgraded table. Without this the
    // migration's `ADD INDEX` could be deleted and every other assertion here would still pass, leaving a
    // production upgrade whose consistency check scans the corpus instead of a window.
    let indexes: Vec<String> = client
        .query(
            "SELECT name FROM system.data_skipping_indices \
             WHERE database = currentDatabase() AND table = 'otel_spans' AND name = 'idx_ingested_at'",
        )
        .fetch_all()
        .await
        .expect("read the skip indexes back");
    assert_eq!(
        indexes.len(),
        1,
        "the upgraded span table is missing idx_ingested_at, so the consistency check has no index to \
         find recent rows with"
    );

    // The table the consistency check writes to has to exist after an upgrade, not only after a fresh
    // install: `apply_initial_schema` is skipped whenever a version record exists, so a `CREATE TABLE` in the
    // fresh schema alone never reaches a database that upgrades.
    let anomalies: Vec<u8> = client
        .query(
            "SELECT 1 FROM system.tables WHERE database = currentDatabase() \
             AND name = 'span_partition_anomalies'",
        )
        .fetch_all()
        .await
        .expect("look for the anomaly table");
    assert_eq!(
        anomalies.len(),
        1,
        "the upgraded database has no span_partition_anomalies table, so every consistency pass will fail \
         with UNKNOWN_TABLE and the cross-month residual is undetected"
    );

    // And it is usable, not merely present - a pass against the upgraded database must run.
    service
        .check_partition_consistency()
        .await
        .expect("the consistency check must run against an upgraded database");
}

/// A crash between `EXCHANGE TABLES` and the `DROP` must not read as a completed migration.
///
/// The v3 rebuild is copy-swap: a `_v3` replacement is created with the new sorting key, filled, exchanged
/// into place, and the *old* table - now wearing the `_v3` name - is dropped. After the exchange and before
/// the drop, both facts a shape-based precondition asks about already look right: the live table has the new
/// sorting key and metrics have their version column. So a precondition asking only about those reports the
/// migration applied, the version record advances, and the old full-size table is never reclaimed - silently
/// doubling the storage of the two largest tables, with nothing to detect it.
///
/// Naming the replacement tables in the precondition makes a re-run finish the job. That is safe because the
/// statements begin by dropping the leftover, so re-running is idempotent - which is what this asserts, since
/// a precondition that fires but a statement list that then fails would be no better.
#[tokio::test]
async fn a_leftover_replacement_table_makes_the_migration_run_again() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    // Each replacement table is staged **alone**, in its own database, and that is the whole point of the
    // arrangement. Staging both was the first version and could not distinguish anything: the precondition
    // fires on whichever clause is present, and the statements then drop *both* leftovers - so deleting the
    // `otel_metrics_v3` clause left the test green. One leftover per case is what makes each clause
    // individually load-bearing.
    for (index, (leftover, ddl)) in [
        (
            "otel_spans_v3",
            "CREATE TABLE otel_spans_v3 AS otel_spans ENGINE = ReplacingMergeTree(ingested_at) \
             PARTITION BY toYYYYMM(timestamp_start) ORDER BY (project_id, trace_id, span_id)",
        ),
        (
            "otel_metrics_v3",
            "CREATE TABLE otel_metrics_v3 AS otel_metrics ENGINE = ReplacingMergeTree(ingested_at) \
             PARTITION BY toYYYYMM(timestamp) \
             ORDER BY (project_id, metric_name, toDate(timestamp), timestamp, datapoint_id)",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let database = format!("sideseat_parity_leftover_{index}");
        let service = clickhouse_backend(&url, &database).await;
        let client = raw_client(&url, &database);

        let exists = |name: String| {
            let client = client.clone();
            async move {
                let found: Option<u8> = client
                    .query(
                        "SELECT 1 FROM system.tables WHERE database = currentDatabase() AND name = ? LIMIT 1",
                    )
                    .bind(name)
                    .fetch_optional()
                    .await
                    .expect("system.tables is readable");
                found.is_some()
            }
        };

        // A fresh database is already at v3, so the shape-based half of the precondition is satisfied and
        // this leftover is the only thing that can make the migration run.
        client
            .query(ddl)
            .execute()
            .await
            .unwrap_or_else(|e| panic!("stage {leftover}: {e}"));
        assert!(exists(leftover.to_string()).await, "{leftover} was staged");

        service
            .apply_migration_for_test(3)
            .await
            .unwrap_or_else(|e| {
                panic!("a re-run over an already-migrated table must succeed ({leftover}): {e}")
            });

        assert!(
            !exists(leftover.to_string()).await,
            "{leftover} survived - the old table is never reclaimed and its storage stays doubled. The \
             precondition does not name it, so a crash between its EXCHANGE and its DROP is invisible"
        );

        // And the re-run left the live tables intact and writable, which is what makes re-running the right
        // remedy rather than merely a detectable state.
        let sorting_key: Vec<String> = client
            .query(
                "SELECT sorting_key FROM system.tables \
                 WHERE database = currentDatabase() AND name = 'otel_spans'",
            )
            .fetch_all()
            .await
            .expect("read the sorting key back");
        assert_eq!(sorting_key.len(), 1);
        assert!(
            !sorting_key[0].contains("toDate("),
            "the re-run must not reinstate the date expression, got {:?}",
            sorting_key[0]
        );

        service
            .insert_spans(vec![NormalizedSpan {
                project_id: Some(PROJECT.to_string()),
                trace_id: "leftover-trace".to_string(),
                span_id: "leftover-span".to_string(),
                span_name: "after-rerun".to_string(),
                timestamp_start: ts(1),
                timestamp_end: Some(ts(1)),
                ..Default::default()
            }])
            .await
            .expect("the live table accepts a write after the re-run");
    }
}

/// The two reconciliation reads, on ClickHouse: which fields carry file references, and which traces are empty.
///
/// Both were written for the survivor reconciliation and only DuckDB's had behavioural coverage - the file tests
/// hardwire DuckDB plus SQLite, so ClickHouse's `FINAL` versions of both queries were never executed by any
/// test. Two hand-written statements per question is exactly what the parity suite exists to compare, and these
/// two decide whether a live span keeps its file.
///
/// `FINAL` is the part with teeth: an expired revision's text must not keep an association alive, and an
/// obsolete revision must not make a deleted trace look alive. So the fixture corrects both a span's *content*
/// and a trace's existence, which is what tells a `FINAL` read from a raw one.
#[tokio::test]
async fn the_reconciliation_reads_agree_with_duckdb() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_reconcile").await;

    let old_hash = "a".repeat(64);
    let new_hash = "b".repeat(64);
    let base = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "reconcile-trace".to_string(),
        span_id: "reconcile-span".to_string(),
        span_name: "generation".to_string(),
        timestamp_start: ts(1),
        timestamp_end: Some(ts(1)),
        ..Default::default()
    };

    // A span whose correction *replaces* which file it references. The obsolete revision still names the old
    // hash, so a raw read would keep an association the current span does not justify.
    for (hash, ingested) in [(&old_hash, ts(10)), (&new_hash, ts(20))] {
        let spans = vec![NormalizedSpan {
            messages: Some(format!(r#"[{{"content":"see #!B64!#image/png::{hash}"}}]"#)),
            ingested_at: Some(ingested),
            ..base.clone()
        }];
        duck.insert_spans(spans.clone())
            .await
            .expect("duckdb insert");
        ch.insert_spans(spans).await.expect("clickhouse insert");
    }

    let traces = vec!["reconcile-trace".to_string(), "never-existed".to_string()];

    let mut d_fields = duck
        .file_reference_fields_for_traces(&ProjectId::from(PROJECT), &traces)
        .await
        .expect("duckdb fields");
    let mut c_fields = ch
        .file_reference_fields_for_traces(&ProjectId::from(PROJECT), &traces)
        .await
        .expect("clickhouse fields");
    d_fields.sort();
    c_fields.sort();
    assert_eq!(
        d_fields, c_fields,
        "the two backends disagree about which field text a surviving span carries, so a file kept on one is \
         released on the other"
    );
    assert!(
        c_fields.iter().any(|f| f.contains(&new_hash)),
        "the correction's reference is missing, so its file would be released: {c_fields:?}"
    );
    assert!(
        !c_fields.iter().any(|f| f.contains(&old_hash)),
        "the *obsolete* revision's reference is still returned, so a file the current span does not reference \
         is kept forever - the read is not going through FINAL: {c_fields:?}"
    );

    let mut d_empty = duck
        .traces_without_spans(&ProjectId::from(PROJECT), &traces)
        .await
        .expect("duckdb empty");
    let mut c_empty = ch
        .traces_without_spans(&ProjectId::from(PROJECT), &traces)
        .await
        .expect("clickhouse empty");
    d_empty.sort();
    c_empty.sort();
    assert_eq!(
        d_empty, c_empty,
        "the two backends disagree about which traces retention emptied, so a favourite survives on one and is \
         removed on the other"
    );
    assert_eq!(
        c_empty,
        vec!["never-existed".to_string()],
        "only the trace with no spans is empty; the corrected one is still there"
    );
}

/// Restore discovery must not assume that every project has spans.
///
/// A newer analytics backup can contain a metric-only or log-only project whose transactional metadata is
/// outside the selected recovery point. If the maintenance query only enumerates spans, restore repair never
/// sees that project and its rows remain unreachable forever.
#[tokio::test]
async fn restore_project_discovery_covers_every_signal_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_restore_projects").await;
    let span_project = "a-span-only";
    let metric_project = "b-metric-only";
    let log_project = "c-log-only";

    let span = NormalizedSpan {
        project_id: Some(span_project.to_string()),
        trace_id: "restore-trace".to_string(),
        span_id: "restore-span".to_string(),
        timestamp_start: ts(1),
        timestamp_end: Some(ts(1)),
        ..Default::default()
    };
    duck.insert_spans(vec![span.clone()])
        .await
        .expect("duckdb span");
    ch.insert_spans(vec![span]).await.expect("clickhouse span");

    let metric = NormalizedMetric {
        project_id: Some(metric_project.to_string()),
        metric_name: "restore.metric".to_string(),
        metric_type: MetricType::Sum,
        aggregation_temporality: AggregationTemporality::Cumulative,
        timestamp: ts(2),
        value_int: Some(1),
        ..Default::default()
    };
    duck.insert_metrics(std::slice::from_ref(&metric))
        .await
        .expect("duckdb metric");
    ch.insert_metrics(std::slice::from_ref(&metric))
        .await
        .expect("clickhouse metric");

    let log = NormalizedLog {
        project_id: Some(log_project.to_string()),
        log_digest: "restore-log".to_string(),
        ordinal: 0,
        timestamp: ts(3),
        body: serde_json::json!("restore"),
        body_text: Some("restore".to_string()),
        ingested_at: Some(ts(3)),
        ..Default::default()
    };
    duck.insert_logs(std::slice::from_ref(&log))
        .await
        .expect("duckdb log");
    ch.insert_logs(std::slice::from_ref(&log))
        .await
        .expect("clickhouse log");

    let expected = [span_project, metric_project, log_project]
        .into_iter()
        .map(ProjectId::from)
        .collect::<Vec<_>>();
    assert_eq!(
        duck.analytics_project_ids(10)
            .await
            .expect("duckdb project discovery"),
        expected
    );
    assert_eq!(
        ch.analytics_project_ids(10)
            .await
            .expect("clickhouse project discovery"),
        expected
    );
    assert_eq!(
        ch.analytics_project_ids(2)
            .await
            .expect("bounded clickhouse project discovery"),
        expected[..2]
    );

    let metric_project = ProjectId::from(metric_project);
    duck.delete_project_data(&metric_project)
        .await
        .expect("duckdb project delete");
    ch.delete_project_data(&metric_project)
        .await
        .expect("clickhouse project delete");
    let after_delete = vec![expected[0].clone(), expected[2].clone()];
    assert_eq!(
        duck.analytics_project_ids(10)
            .await
            .expect("duckdb discovery after delete"),
        after_delete
    );
    assert_eq!(
        ch.analytics_project_ids(10)
            .await
            .expect("clickhouse discovery after delete"),
        after_delete
    );
}

/// A read that must span **both shards**: the consistency check, and the pre-identity metric count.
///
/// Both of these read the `Distributed` front end, and both read `_local` in a previous version - which on a
/// one-shard fixture is the same thing, so the fix was unfalsifiable. Here the rows are placed so that each
/// question has an answer on the shard the client is *not* connected to: a `_local` read reports clean or zero,
/// a distributed read finds them.
///
/// The sharding key is `sipHash64(project_id)`, so two project ids that hash to different shards are what makes
/// this work. They are found rather than assumed, by asking the cluster which shard each would land on - a
/// hardcoded pair would silently stop testing anything if the key ever changed.
#[tokio::test]
async fn a_two_shard_cluster_reports_anomalies_and_legacy_rows_from_every_shard() {
    let Ok(url) = std::env::var(TWO_SHARD_URL_ENV) else {
        eprintln!(
            "clickhouse two-shard: skipped - set {TWO_SHARD_URL_ENV} (or run \
             `make test-clickhouse-two-shard`)"
        );
        return;
    };

    let database = "sideseat_two_shard";
    let service = replicated_backend_at(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let client = raw_client_at(&url, database, &user, &password);

    // Which shard each candidate project lands on, asked rather than assumed.
    let mut per_shard: [Option<String>; 2] = [None, None];
    for n in 0..64 {
        let candidate = format!("shard-probe-{n}");
        let shard: Vec<u64> = client
            // `toUInt64` explicitly: `(x % 2) + 1` narrows to **UInt16**, which a `Vec<u64>` fetch rejects
            // with a schema mismatch - and the failure reads as a broken cluster rather than a wrong type.
            .query("SELECT toUInt64((sipHash64(?) % 2) + 1)")
            .bind(&candidate)
            .fetch_all()
            .await
            .expect("compute the shard");
        let index = (shard[0] - 1) as usize;
        if per_shard[index].is_none() {
            per_shard[index] = Some(candidate);
        }
        if per_shard.iter().all(Option::is_some) {
            break;
        }
    }
    let [Some(near), Some(far)] = per_shard else {
        panic!("could not find a project id for each shard");
    };

    // A cross-month correction on **each** shard's project, so whichever shard the client reaches, the other
    // one also holds an anomaly. A `_local` read finds at most one.
    let now = Utc::now();
    let this_month = Utc
        .with_ymd_and_hms(now.year(), now.month(), 5, 12, 0, 0)
        .unwrap();
    let last_month = this_month - chrono::Duration::days(20);

    for project in [&near, &far] {
        for (instant, tokens, ingested) in [(last_month, 100, ts(10)), (this_month, 900, ts(20))] {
            service
                .insert_spans(vec![NormalizedSpan {
                    project_id: Some(project.clone()),
                    trace_id: format!("{project}-trace"),
                    span_id: format!("{project}-span"),
                    span_name: "generation".to_string(),
                    observation_type: Some(ObservationType::Generation),
                    timestamp_start: instant,
                    timestamp_end: Some(instant),
                    ingested_at: Some(ingested),
                    gen_ai_usage_input_tokens: tokens,
                    duration_ms: 1000,
                    ..Default::default()
                }])
                .await
                .expect("insert a cross-month correction");
        }
    }

    let policy_count: u64 = client
        .query(&format!(
            "SELECT count() FROM clusterAllReplicas('{REPLICATED_CLUSTER}', system.row_policies) \
             WHERE database = ? AND short_name = 'sideseat_tenant_filter'"
        ))
        .bind(database)
        .fetch_one()
        .await
        .expect("inspect every shard's local row policies");
    assert_eq!(
        policy_count, 8,
        "four physical-table policies must exist on each of the two shards"
    );

    let tenant_client = client.clone().with_option(
        sideseat_adapter_clickhouse::schema::TENANT_PROJECT_SETTING,
        &near,
    );
    let tenant_rows: Vec<String> = tenant_client
        .query(
            // No explicit project predicate: each remote `_local` policy has to receive the setting
            // forwarded with this Distributed query.
            "SELECT DISTINCT project_id FROM otel_spans ORDER BY project_id",
        )
        .fetch_all()
        .await
        .expect("read one tenant through the Distributed table");
    assert_eq!(tenant_rows, vec![near.clone()]);

    let unscoped_rows: Vec<String> = client
        .query("SELECT DISTINCT project_id FROM otel_spans ORDER BY project_id")
        .fetch_all()
        .await
        .expect("run an unscoped Distributed read");
    assert!(
        unscoped_rows.is_empty(),
        "an unset setting must fail closed on every shard"
    );

    let maintenance_rows: Vec<String> = client
        .clone()
        .with_option(
            sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
            "1",
        )
        .query("SELECT DISTINCT project_id FROM otel_spans ORDER BY project_id")
        .fetch_all()
        .await
        .expect("read every tenant through the explicit maintenance path");
    let mut expected_projects = vec![near.clone(), far.clone()];
    expected_projects.sort();
    assert_eq!(maintenance_rows, expected_projects);

    let outcome = service
        .check_partition_consistency()
        .await
        .expect("the consistency check must run on a multi-shard cluster");
    assert_eq!(
        outcome.anomalies, 2,
        "the check found {} of 2 anomalies. One sits on each shard, so anything less means the query reads only \
         the shard the connection reached - and on a real cluster most anomalies are then invisible. A failure \
         *running* it means the distributed subquery was refused (`distributed_product_mode = deny`), which is \
         the other half of the same finding",
        outcome.anomalies
    );

    let recorded = service
        .partition_anomalies()
        .await
        .expect("the anomaly records are readable");
    assert_eq!(
        recorded.len(),
        2,
        "the anomaly *records* are per-shard, so a report described as deployment-wide depends on which shard \
         answered: {recorded:?}"
    );

    // The same question for pre-identity metric rows: one on each shard, counted through the front end.
    //
    // `insert_distributed_sync`, because these go through the `Distributed` table and this client is a raw one -
    // the service sets it, a raw client does not. Without it the insert returns once the rows are spooled on the
    // initiating node, so a count taken immediately afterwards saw one of the two and the failure read as a
    // one-shard *read* rather than an unfinished write. The fixture has to be at least as careful as the code it
    // is checking.
    let sync_client =
        raw_client_at(&url, database, &user, &password).with_option("insert_distributed_sync", "1");
    for project in [&near, &far] {
        sync_client
            .query(
                "INSERT INTO otel_metrics (project_id, metric_name, metric_type, timestamp, value_double, \
                 datapoint_id) VALUES (?, 'legacy.counter', 'sum', now64(6), 1, '')",
            )
            .bind(project)
            .execute()
            .await
            .expect("insert a pre-identity row");
    }

    let unidentified = service
        .report_unidentified_metric_rows()
        .await
        .expect("the count is available");
    assert_eq!(
        unidentified, 2,
        "counted {unidentified} of 2 pre-identity rows - one per shard, so a smaller number means the count \
         reads one shard and an operator is told the exposure is smaller than it is"
    );
}

/// The v7 access-control migration must qualify its policy targets on every host.
///
/// ClickHouse accepts `CREATE ROW POLICY ... ON otel_spans_local ON CLUSTER ...`, but remote
/// hosts resolve that unqualified table in `default`, not in the database selected by the initiating
/// HTTP client. The DDL therefore succeeds while protecting no application table. This exercises the
/// incremental migration renderer, separately from the fresh-schema two-shard oracle above.
#[tokio::test]
async fn the_replicated_tenant_policy_migration_targets_the_configured_database() {
    let Ok(url) = std::env::var(REPLICATED_URL_ENV) else {
        eprintln!(
            "clickhouse replicated: skipped - set {REPLICATED_URL_ENV} (or run \
             `make test-clickhouse-replicated`)"
        );
        return;
    };

    let database = "sideseat_repl_policy_migration";
    let service = replicated_backend(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let raw = raw_client_at(&url, database, &user, &password);

    for table in [
        "otel_spans_local",
        "otel_metrics_local",
        "otel_logs_local",
        "span_partition_anomalies_local",
    ] {
        raw.query(&format!(
            "DROP ROW POLICY IF EXISTS sideseat_tenant_filter \
             ON `{database}`.{table} ON CLUSTER {REPLICATED_CLUSTER}"
        ))
        .execute()
        .await
        .expect("remove the fresh-schema policy");
    }

    service
        .apply_migration_for_test(7)
        .await
        .expect("apply the tenant policy migration");

    let configured_policies: u64 = raw
        .query(&format!(
            "SELECT count() FROM clusterAllReplicas('{REPLICATED_CLUSTER}', system.row_policies) \
             WHERE database = ? AND short_name = 'sideseat_tenant_filter'"
        ))
        .bind(database)
        .fetch_one()
        .await
        .expect("inspect configured-database policies");
    assert_eq!(configured_policies, 4);

    let default_policies: u64 = raw
        .query(&format!(
            "SELECT count() FROM clusterAllReplicas('{REPLICATED_CLUSTER}', system.row_policies) \
             WHERE database = 'default' AND short_name = 'sideseat_tenant_filter'"
        ))
        .fetch_one()
        .await
        .expect("inspect default-database policies");
    assert_eq!(
        default_policies, 0,
        "the migration must not silently protect default instead of the configured database"
    );
}
