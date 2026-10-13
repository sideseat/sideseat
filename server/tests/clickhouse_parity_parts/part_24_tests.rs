/// One search's answers on a backend: span and log searches over more candidates than one source read holds,
/// nearly all tied on their instant, paged 37 hits at a time and read whole - hits, cursors and `examined`.
async fn chunked_search_answers(
    repository: &(impl SearchIndex + ?Sized),
    project: &str,
) -> Vec<(
    Vec<String>,
    Option<sideseat_ports::types::SearchCursor>,
    u32,
)> {
    let mut answers = Vec::new();
    for signal in [SearchSignal::Spans, SearchSignal::Logs] {
        let page = |limit: u32, cursor: Option<sideseat_ports::types::SearchCursor>| async move {
            let page = sideseat_domain::search::SearchService::execute(
                repository,
                &SearchQuery {
                    project_id: ProjectId::from(project),
                    signal,
                    expression: sideseat_domain::search::parse("needle", signal).unwrap(),
                    limit,
                    max_examined: 10_000,
                    cursor,
                    from_timestamp: None,
                    to_timestamp: None,
                },
            )
            .await
            .expect("search");
            let hits = page
                .hits
                .into_iter()
                .map(|hit| match hit.record {
                    SearchRecord::Span(span) => format!("{}/{}", span.trace_id, span.span_id),
                    SearchRecord::Log(log) => format!("{}/{}", log.log_digest, log.ordinal),
                })
                .collect::<Vec<_>>();
            (hits, page.next_cursor, page.examined)
        };
        answers.push(page(10_000, None).await);
        let mut cursor = None;
        loop {
            let next = page(37, cursor.clone()).await;
            let done = next.0.is_empty();
            cursor = next.1.clone();
            answers.push(next);
            if done {
                break;
            }
        }
    }
    answers
}

/// The fixture `chunked_search_answers` reads, written to `backend` under `project`.
async fn seed_chunked_search(backend: &(impl AnalyticsRepository + ?Sized), project: &str) {
    let total = sideseat_ports::types::SEARCH_SOURCE_CHUNK_ROWS + 77;
    let mut spans: Vec<NormalizedSpan> = (0..total)
        .map(|n| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: format!("chunk-{:03}", n % 50),
            span_id: format!("s{n:04}"),
            span_name: "chunked".to_string(),
            timestamp_start: ts(700 + (n % 3) as i64),
            exception_stacktrace: Some(format!("needle {}", "x".repeat(n * 37 % 4096))),
            ingested_at: Some(ts(800)),
            logical_bytes: 2048,
            ..Default::default()
        })
        .collect();
    sideseat_domain::search::index_spans(&mut spans);
    backend.insert_spans(spans).await.expect("spans");
    let mut logs: Vec<NormalizedLog> = (0..total)
        .map(|n| NormalizedLog {
            project_id: Some(project.to_string()),
            log_digest: format!("{n:064x}"),
            ordinal: 0,
            timestamp: ts(700 + (n % 3) as i64),
            time: Some(ts(700 + (n % 3) as i64)),
            severity_text: Some("INFO".to_string()),
            body: serde_json::json!(format!("needle body {n}")),
            body_text: Some(format!("needle body {n}")),
            raw_log: serde_json::json!({"body": {"stringValue": format!("needle body {n}")}}),
            ingested_at: Some(ts(800)),
            logical_bytes: 2048,
            ..Default::default()
        })
        .collect();
    sideseat_domain::search::index_logs(&mut logs);
    backend.insert_logs(&logs).await.expect("logs");
}

/// On a replicated cluster a search reads its page and its chunked source text through the `Distributed` tables -
/// whose `GLOBAL IN` a cluster refuses in its plain form - and answers what one DuckDB node answers.
#[tokio::test]
async fn replicated_search_reads_its_page_and_sources_like_one_node() {
    let Ok(url) = std::env::var(REPLICATED_URL_ENV) else {
        eprintln!(
            "clickhouse replicated: skipped - set {REPLICATED_URL_ENV} (or run `make test-clickhouse-replicated`)"
        );
        return;
    };
    let cluster = replicated_backend(&url, "sideseat_replicated_search").await;
    let (_temp, duck) = duckdb_backend().await;
    for backend in [
        &cluster as &(dyn AnalyticsRepository + Send + Sync),
        &duck as &(dyn AnalyticsRepository + Send + Sync),
    ] {
        seed_chunked_search(backend, PROJECT).await;
    }
    let reference = chunked_search_answers(&duck, PROJECT).await;
    assert!(
        reference.iter().any(|(hits, _, _)| hits.len() > 300),
        "the premise: pages of many chunks"
    );
    assert_eq!(chunked_search_answers(&cluster, PROJECT).await, reference);
}

/// On two shards, with the project's rows on the shard the reading node is not, a search reads its page and its
/// chunked source text across the cluster and answers what one DuckDB node answers.
#[tokio::test]
async fn two_shard_search_reads_its_page_and_sources_like_one_node() {
    let Ok(url) = std::env::var(TWO_SHARD_URL_ENV) else {
        eprintln!(
            "clickhouse two-shard: skipped - set {TWO_SHARD_URL_ENV} (or run \
             `make test-clickhouse-two-shard`)"
        );
        return;
    };
    let cluster = replicated_backend_at(&url, "sideseat_two_shard_search").await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let project = project_on_the_second_shard(&raw_client_at(
        &url,
        "sideseat_two_shard_search",
        &user,
        &password,
    ))
    .await;
    let (_temp, duck) = duckdb_backend().await;
    for backend in [
        &cluster as &(dyn AnalyticsRepository + Send + Sync),
        &duck as &(dyn AnalyticsRepository + Send + Sync),
    ] {
        seed_chunked_search(backend, &project).await;
    }
    let reference = chunked_search_answers(&duck, &project).await;
    assert!(
        reference.iter().any(|(hits, _, _)| hits.len() > 300),
        "the premise: pages of many chunks"
    );
    assert_eq!(chunked_search_answers(&cluster, &project).await, reference);
}
