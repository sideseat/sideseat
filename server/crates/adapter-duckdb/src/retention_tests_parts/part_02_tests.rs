/// Progress is counted in identities. Counting deleted *rows* let one batch report more progress
/// than it made, zeroing the remainder while the project was still over its limit.
///
/// Driven through [`trim_project_to_limit`] with `batch_size = 1`, because at the production
/// batch size an overage needing two batches is 100 000 identities. With a batch per identity,
/// each batch deletes three rows for one identity - so row-counted progress finishes after the
/// first batch and leaves the project two identities over.
#[tokio::test]
async fn count_retention_progress_is_identities_not_deleted_rows() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Five identities, each carrying three revisions.
    for i in 0..5 {
        let ts = format!("2020-01-0{} 00:00:00", i + 1);
        for rev in 0..3 {
            redeliver_span(
                &conn,
                "default",
                &format!("t{i}"),
                &format!("s{i}"),
                &ts,
                &format!("2020-0{}-01 00:00:00", rev + 1),
            );
        }
    }

    // Three identities over a limit of two, one identity per batch.
    let (_deleted, _) =
        trim_project_to_limit(&conn, "default", 3, 1, u64::MAX, &no_intent).expect("Should trim");

    let winners: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM {DEDUP_SPANS} WHERE project_id = 'default'"),
            [],
            |row| row.get(0),
        )
        .expect("Should query");
    assert_eq!(
        winners, 2,
        "the loop must land exactly on max_spans; row-counted progress stops early"
    );
}

/// Counting raw rows also makes a corrected span look like two, so the sweep overshoots.
#[tokio::test]
async fn count_retention_reaches_the_limit_when_identities_carry_many_revisions() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    for i in 0..2 {
        let ts = format!("2020-01-0{} 00:00:00", i + 1);
        for rev in 0..3 {
            redeliver_span(
                &conn,
                "default",
                &format!("t{i}"),
                &format!("s{i}"),
                &ts,
                &format!("2020-0{}-01 00:00:00", rev + 1),
            );
        }
    }
    insert_span_for_project(&conn, "default", "t2", "s2", "2020-02-01 00:00:00");
    insert_span_for_project(&conn, "default", "t3", "s3", "2020-02-02 00:00:00");

    let (_deleted, _) = cleanup_by_count(&conn, 2, &no_intent).expect("Should cleanup");

    let winners: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM {DEDUP_SPANS} WHERE project_id = 'default'"),
            [],
            |row| row.get(0),
        )
        .expect("Should query");
    assert_eq!(winners, 2, "the sweep must land exactly on max_spans");
}

// ========================================================================
// METRICS RETENTION TESTS
// ========================================================================

fn insert_test_metric(conn: &Connection, name: &str, timestamp: &str) {
    conn.execute(
        "INSERT INTO otel_metrics (datapoint_id, metric_name, metric_type, timestamp)
             VALUES (?1, ?1, 'gauge', ?2)",
        [name, timestamp],
    )
    .expect("Failed to insert test metric");
}

fn insert_governed_metric(
    conn: &Connection,
    project_id: &str,
    name: &str,
    timestamp: &str,
    hold_until: Option<&str>,
    logical_bytes: u64,
) {
    conn.execute(
            "INSERT INTO otel_metrics
                 (project_id, datapoint_id, metric_name, metric_type, timestamp, hold_until, logical_bytes)
             VALUES (?, ?, ?, 'gauge', ?, ?, ?)",
            duckdb::params![
                project_id,
                name,
                name,
                timestamp,
                hold_until,
                logical_bytes
            ],
        )
        .expect("insert governed metric");
}

fn insert_governed_log(
    conn: &Connection,
    project_id: &str,
    digest: &str,
    timestamp: &str,
    hold_until: Option<&str>,
    logical_bytes: u64,
) {
    conn.execute(
        "INSERT INTO otel_logs
                 (project_id, log_digest, ordinal, timestamp, severity_number,
                  dropped_attributes_count, flags, ingested_at, hold_until, logical_bytes)
             VALUES (?, ?, 0, ?, 0, 0, 0, ?, ?, ?)",
        duckdb::params![
            project_id,
            digest,
            timestamp,
            timestamp,
            hold_until,
            logical_bytes
        ],
    )
    .expect("insert governed log");
}

fn signal_count(conn: &Connection, table: &str, project_id: &str) -> i64 {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE project_id = ?"),
        [project_id],
        |row| row.get(0),
    )
    .expect("count signal rows")
}

#[tokio::test]
async fn active_hold_survives_project_age_retention_for_every_signal() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();
    let hold = "2030-01-01 00:00:00";

    insert_span_for_project(
        &conn,
        "held",
        "held-trace",
        "held-span",
        "2020-01-01 00:00:00",
    );
    conn.execute(
        "UPDATE otel_spans SET hold_until = ?, logical_bytes = 10 WHERE project_id = 'held'",
        [hold],
    )
    .unwrap();
    insert_span_for_project(
        &conn,
        "held",
        "plain-trace",
        "plain-span",
        "2020-01-02 00:00:00",
    );
    insert_governed_metric(
        &conn,
        "held",
        "held-metric",
        "2020-01-01 00:00:00",
        Some(hold),
        10,
    );
    insert_governed_metric(
        &conn,
        "held",
        "plain-metric",
        "2020-01-02 00:00:00",
        None,
        10,
    );
    insert_governed_log(
        &conn,
        "held",
        "held-log",
        "2020-01-01 00:00:00",
        Some(hold),
        10,
    );
    insert_governed_log(&conn, "held", "plain-log", "2020-01-02 00:00:00", None, 10);

    let config = RetentionConfig {
        max_age_minutes: Some(1),
        max_spans: None,
    };
    let result = super::run_retention_for_project(&conn, &config, "held", &no_intent, test_now())
        .expect("project retention");
    assert_eq!(result.deleted_count, 3);

    assert_eq!(signal_count(&conn, "otel_spans", "held"), 1);
    assert_eq!(signal_count(&conn, "otel_metrics", "held"), 1);
    assert_eq!(signal_count(&conn, "otel_logs", "held"), 1);
    for (table, identity_column, identity) in [
        ("otel_spans", "trace_id", "held-trace"),
        ("otel_metrics", "datapoint_id", "held-metric"),
        ("otel_logs", "log_digest", "held-log"),
    ] {
        let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM {table} WHERE project_id = 'held' AND {identity_column} = ?"
                    ),
                    [identity],
                    |row| row.get(0),
                )
                .unwrap();
        assert_eq!(count, 1, "{table}'s held row was deleted");
    }
}

#[tokio::test]
async fn active_hold_survives_count_retention_while_unheld_rows_are_reclaimed() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();
    insert_span_for_project(
        &conn,
        "held",
        "held-trace",
        "held-span",
        "2020-01-01 00:00:00",
    );
    conn.execute(
        "UPDATE otel_spans SET hold_until = '2030-01-01 00:00:00'
             WHERE project_id = 'held' AND trace_id = 'held-trace'",
        [],
    )
    .unwrap();
    insert_span_for_project(&conn, "held", "plain-a", "span-a", "2020-01-02 00:00:00");
    insert_span_for_project(&conn, "held", "plain-b", "span-b", "2020-01-03 00:00:00");

    let config = RetentionConfig {
        max_age_minutes: None,
        max_spans: Some(1),
    };
    super::run_retention_for_project(&conn, &config, "held", &no_intent, test_now())
        .expect("count retention");

    assert_eq!(span_count(&conn, "held"), 1);
    let held: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM otel_spans
                 WHERE project_id = 'held' AND trace_id = 'held-trace'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(held, 1);
}

#[tokio::test]
async fn test_cleanup_metrics_by_time_empty_table() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    let deleted = cleanup_metrics_by_time(&conn, 60).expect("Should cleanup");
    assert_eq!(deleted, 0);
}

#[tokio::test]
async fn test_cleanup_metrics_by_time_removes_old_metrics() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Insert metrics: 2 old, 1 recent
    insert_test_metric(&conn, "metric1", "2020-01-01 00:00:00");
    insert_test_metric(&conn, "metric2", "2020-01-02 00:00:00");
    let recent = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    insert_test_metric(&conn, "metric3", &recent);

    // Cleanup metrics older than 1 minute
    let deleted = cleanup_metrics_by_time(&conn, 1).expect("Should cleanup");
    assert_eq!(deleted, 2);

    // Verify only recent metric remains
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_metrics", [], |row| row.get(0))
        .expect("Should query");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_cleanup_metrics_preserves_recent_metrics() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Insert only recent metrics
    let now = Utc::now();
    let recent1 = now.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    let recent2 = (now - TimeDelta::seconds(30))
        .format("%Y-%m-%d %H:%M:%S%.6f")
        .to_string();

    insert_test_metric(&conn, "metric1", &recent1);
    insert_test_metric(&conn, "metric2", &recent2);

    // Cleanup with 1 minute retention - should preserve both
    let deleted = cleanup_metrics_by_time(&conn, 1).expect("Should cleanup");
    assert_eq!(deleted, 0);

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_metrics", [], |row| row.get(0))
        .expect("Should query");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn test_run_retention_cleans_both_spans_and_metrics() {
    let (_temp_dir, analytics) = create_test_service().await;
    let conn = analytics.conn();

    // Insert old spans and metrics
    insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
    insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");
    insert_test_metric(&conn, "metric1", "2020-01-01 00:00:00");
    insert_test_metric(&conn, "metric2", "2020-01-02 00:00:00");

    // Run retention with 1 minute limit
    let config = RetentionConfig {
        max_age_minutes: Some(1),
        max_spans: None,
    };
    let result = run_retention(&conn, &config, &no_intent).expect("Should run retention");
    assert!(result.deleted_count > 0);

    // Verify both spans and metrics are deleted
    let span_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
        .expect("Should query");
    assert_eq!(span_count, 0);

    let metric_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM otel_metrics", [], |row| row.get(0))
        .expect("Should query");
    assert_eq!(metric_count, 0);
}
