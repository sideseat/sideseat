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

/// A fresh replicated database gets the whole schema - every `Replicated*` table, its `Distributed` front and
/// the raw-record table - and a raw record and the span derived from it are read back through the front tables.
#[tokio::test]
async fn a_replicated_database_is_created_with_the_whole_schema() {
    let Ok(url) = std::env::var(REPLICATED_URL_ENV) else {
        eprintln!(
            "clickhouse replicated: skipped - set {REPLICATED_URL_ENV} (or run `make test-clickhouse-replicated`)"
        );
        return;
    };
    let database = "sideseat_replicated_schema";
    let service = replicated_backend(&url, database).await;
    let project = ProjectId::from("replicated");
    sideseat_ports::traits::RawStore::insert_raw_records(
        &service,
        &[sideseat_ports::types::RawRecordRow {
            project_id: project.clone(),
            raw_id: "raw-1".to_string(),
            signal: sideseat_ports::types::StagedSignal::Traces,
            received_at: ts(10),
            rewritten: false,
            record: b"SSR1\0\0payload".to_vec(),
        }],
    )
    .await
    .expect("insert a raw record");
    service
        .insert_spans(vec![NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: "trace".to_string(),
            span_id: "span".to_string(),
            span_name: "derived".to_string(),
            timestamp_start: ts(10),
            ingested_at: Some(ts(10)),
            raw_id: Some("raw-1".to_string()),
            ..Default::default()
        }])
        .await
        .expect("insert the derived span");
    let records = sideseat_ports::traits::RawStore::get_raw_records(
        &service,
        &project,
        &["raw-1".to_string()],
    )
    .await
    .expect("read the raw record back");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record, b"SSR1\0\0payload");
    let span = service
        .get_span(&project, "trace", "span")
        .await
        .expect("read the span back")
        .expect("the span exists");
    assert_eq!(span.span_name.as_deref(), Some("derived"));
}
