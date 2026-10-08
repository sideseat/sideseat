/// A ClickHouse span write that fails partway names the projects that committed before it failed.
///
/// ClickHouse writes each tenant's rows in an insert of its own, so a later tenant can fail after an earlier one
/// committed. Reported as a plain failure, the batch undid the committed tenant's file associations while its
/// rows stayed readable. The fault is raised inside the real server: a constraint on the span table that
/// rejects one project's rows fails exactly that project's insert.
#[tokio::test]
async fn a_partial_span_write_names_the_projects_that_committed() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_partial_write";
    let clickhouse = clickhouse_backend(&url, database).await;
    // A constraint is checked before a block is written, so the doomed tenant's insert stores nothing. (A
    // throwing materialized view is not that fault: ClickHouse writes the source block before its views run.)
    raw_client(&url, database)
        .query("ALTER TABLE otel_spans ADD CONSTRAINT partial_write_fault CHECK project_id != 'doomed'")
        .execute()
        .await
        .expect("fault constraint");

    let span = |project: &str| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "partial-trace".to_string(),
        span_id: format!("{project}-span"),
        span_name: "step".to_string(),
        timestamp_start: ts(0),
        timestamp_end: Some(ts(1)),
        ..Default::default()
    };
    // Tenants are written in project order, so "alive" commits before "doomed" fails.
    let error = clickhouse
        .insert_spans(vec![span("alive"), span("doomed")])
        .await
        .expect_err("the doomed tenant's insert fails");
    match error {
        sideseat_ports::error::DataError::PartiallyWritten {
            committed_projects, ..
        } => assert_eq!(committed_projects, vec!["alive".to_string()]),
        other => panic!("expected a partial write naming the committed tenant, got {other}"),
    }

    let stored = |project: &str| sideseat_ports::types::ListSpansParams {
        project_id: ProjectId::from(project),
        page: 1,
        limit: 10,
        ..Default::default()
    };
    let (alive, _) = clickhouse
        .list_spans(&stored("alive"))
        .await
        .expect("alive spans");
    let (doomed, _) = clickhouse
        .list_spans(&stored("doomed"))
        .await
        .expect("doomed spans");
    assert_eq!(alive.len(), 1, "the committed tenant's row is readable");
    assert!(doomed.is_empty(), "the failed tenant stored nothing");

    // The embedded store writes a batch in one transaction: never partially.
    let (_temp, duckdb) = duckdb_backend().await;
    duckdb
        .insert_spans(vec![span("alive"), span("doomed")])
        .await
        .expect("an atomic write of both tenants");
}
