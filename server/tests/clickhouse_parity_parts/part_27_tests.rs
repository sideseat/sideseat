/// The spans a search for `term` finds, as `trace/span`, sorted: every page, so a hit beyond the first is seen.
async fn spans_matching(repository: &impl SearchIndex, term: &str) -> Vec<String> {
    let page = sideseat_domain::search::SearchService::execute(
        repository,
        &SearchQuery {
            project_id: ProjectId::from(PROJECT),
            signal: SearchSignal::Spans,
            expression: sideseat_ports::types::SearchExpr::Term {
                field: None,
                term: term.to_string(),
            },
            limit: 50,
            max_examined: 1_000,
            cursor: None,
            from_timestamp: None,
            to_timestamp: None,
        },
    )
    .await
    .expect("search");
    let mut found: Vec<String> = page
        .hits
        .into_iter()
        .map(|hit| {
            let SearchRecord::Span(span) = hit.record else {
                panic!("span search returned a log");
            };
            format!("{}/{}", span.trace_id, span.span_id)
        })
        .collect();
    found.sort();
    found
}

/// What a search for each term finds, once a ClickHouse mutation has settled to `expected` - or as it stands when
/// it does not, which the comparison then reports.
async fn settled_matches(
    repository: &impl SearchIndex,
    terms: &[&str],
    expected: &[Vec<String>],
) -> Vec<Vec<String>> {
    let mut found = Vec::new();
    for _ in 0..100 {
        found = Vec::new();
        for term in terms {
            found.push(spans_matching(repository, term).await);
        }
        if found == expected {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    found
}

/// The terms every step searches for.
const TERMS_ON_THE_ROW: [&str; 4] = ["alpha", "oldword", "newword", "beta"];

/// Hold both backends to what a search for each term must find after `step`: DuckDB at once, ClickHouse once its
/// mutations settle.
async fn both_find(
    duck: &impl SearchIndex,
    ch: &impl SearchIndex,
    step: &str,
    expected: [&[&str]; 4],
) {
    let expected: Vec<Vec<String>> = expected
        .iter()
        .map(|found| found.iter().map(|name| name.to_string()).collect())
        .collect();
    let mut embedded = Vec::new();
    for term in TERMS_ON_THE_ROW {
        embedded.push(spans_matching(duck, term).await);
    }
    assert_eq!(
        embedded, expected,
        "DuckDB after {step}, for {TERMS_ON_THE_ROW:?}"
    );
    assert_eq!(
        settled_matches(ch, &TERMS_ON_THE_ROW, &expected).await,
        expected,
        "ClickHouse after {step}, for {TERMS_ON_THE_ROW:?}"
    );
}

/// A span indexed for search from its prompt text, in its own trace.
fn searchable_span(trace_id: &str, span_id: &str, prompt: &str, ingested_s: i64) -> NormalizedSpan {
    let mut spans = vec![NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        span_name: "generation".to_string(),
        timestamp_start: ts(100),
        input_preview: Some(prompt.to_string()),
        content_digest: format!("{trace_id}-{span_id}-{prompt}"),
        ingested_at: Some(ts(ingested_s)),
        ..Default::default()
    }];
    sideseat_domain::search::index_spans(&mut spans);
    spans.pop().expect("one span")
}

/// A span's search terms live and die with its row, alike on both backends: a correction's terms replace the
/// corrected revision's, and a trace, span, session, retention or project deletion leaves no term behind - neither
/// one a search still finds, nor one a span later stored under the same identity inherits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_span_s_search_terms_go_with_its_row_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_duck_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_terms_on_the_row").await;
    let config = sideseat_core::config::RetentionConfig {
        max_age_minutes: Some(24 * 60),
        max_spans: None,
    };
    let duck_side = tempfile::TempDir::new().expect("temp dir");
    let duck_tx = retention_transactional(&duck_side).await;
    let ch_side = tempfile::TempDir::new().expect("temp dir");
    let ch_tx = retention_transactional(&ch_side).await;

    let expired = NormalizedSpan {
        timestamp_start: Utc::now() - chrono::Duration::days(10),
        ..searchable_span("trace-r", "r", "alpha expiring", 600)
    };
    let in_session = NormalizedSpan {
        session_id: Some("session-terms".to_string()),
        ..searchable_span("trace-ss", "ss", "alpha sessioned", 600)
    };
    let initial = vec![
        searchable_span("trace-k", "k", "alpha kept", 600),
        searchable_span("trace-c", "c", "alpha oldword", 600),
        searchable_span("trace-t", "t", "alpha traced", 600),
        searchable_span("trace-s", "s", "alpha single", 600),
        in_session,
        expired,
    ];
    duck.insert_spans(initial.clone())
        .await
        .expect("duckdb insert");
    ch.insert_spans(initial).await.expect("clickhouse insert");
    both_find(
        &duck,
        &ch,
        "stored",
        [
            &[
                "trace-c/c",
                "trace-k/k",
                "trace-r/r",
                "trace-s/s",
                "trace-ss/ss",
                "trace-t/t",
            ],
            &["trace-c/c"],
            &[],
            &[],
        ],
    )
    .await;

    let correction = vec![searchable_span("trace-c", "c", "alpha newword", 700)];
    duck.insert_spans(correction.clone())
        .await
        .expect("duckdb correction");
    ch.insert_spans(correction)
        .await
        .expect("clickhouse correction");
    both_find(
        &duck,
        &ch,
        "corrected",
        [
            &[
                "trace-c/c",
                "trace-k/k",
                "trace-r/r",
                "trace-s/s",
                "trace-ss/ss",
                "trace-t/t",
            ],
            &[],
            &["trace-c/c"],
            &[],
        ],
    )
    .await;

    duck.delete_traces(&ProjectId::from(PROJECT), &["trace-t".to_string()])
        .await
        .expect("duckdb delete trace");
    ch.delete_traces(&ProjectId::from(PROJECT), &["trace-t".to_string()])
        .await
        .expect("clickhouse delete trace");
    both_find(
        &duck,
        &ch,
        "trace deleted",
        [
            &[
                "trace-c/c",
                "trace-k/k",
                "trace-r/r",
                "trace-s/s",
                "trace-ss/ss",
            ],
            &[],
            &["trace-c/c"],
            &[],
        ],
    )
    .await;

    // A span stored again under the deleted identity holds its own terms only.
    let again = vec![searchable_span("trace-t", "t", "beta", 800)];
    duck.insert_spans(again.clone())
        .await
        .expect("duckdb again");
    ch.insert_spans(again).await.expect("clickhouse again");
    both_find(
        &duck,
        &ch,
        "identity reused",
        [
            &[
                "trace-c/c",
                "trace-k/k",
                "trace-r/r",
                "trace-s/s",
                "trace-ss/ss",
            ],
            &[],
            &["trace-c/c"],
            &["trace-t/t"],
        ],
    )
    .await;

    let pair = [("trace-s".to_string(), "s".to_string())];
    duck.delete_spans(&ProjectId::from(PROJECT), &pair)
        .await
        .expect("duckdb delete span");
    ch.delete_spans(&ProjectId::from(PROJECT), &pair)
        .await
        .expect("clickhouse delete span");
    both_find(
        &duck,
        &ch,
        "span deleted",
        [
            &["trace-c/c", "trace-k/k", "trace-r/r", "trace-ss/ss"],
            &[],
            &["trace-c/c"],
            &["trace-t/t"],
        ],
    )
    .await;

    duck.delete_sessions(&ProjectId::from(PROJECT), &["session-terms".to_string()])
        .await
        .expect("duckdb delete session");
    ch.delete_sessions(&ProjectId::from(PROJECT), &["session-terms".to_string()])
        .await
        .expect("clickhouse delete session");
    both_find(
        &duck,
        &ch,
        "session deleted",
        [
            &["trace-c/c", "trace-k/k", "trace-r/r"],
            &[],
            &["trace-c/c"],
            &["trace-t/t"],
        ],
    )
    .await;

    Arc::clone(&duck.0)
        .run_retention_to_completion(&config, 0, None, duck_tx.clone(), duck_tx.clone())
        .await
        .expect("DuckDB retention");
    Arc::clone(&ch.0)
        .run_retention_to_completion(&config, 0, None, ch_tx.clone(), ch_tx.clone())
        .await
        .expect("ClickHouse retention");
    both_find(
        &duck,
        &ch,
        "expired",
        [
            &["trace-c/c", "trace-k/k"],
            &[],
            &["trace-c/c"],
            &["trace-t/t"],
        ],
    )
    .await;

    duck.delete_project_data(&ProjectId::from(PROJECT))
        .await
        .expect("duckdb delete project");
    ch.delete_project_data(&ProjectId::from(PROJECT))
        .await
        .expect("clickhouse delete project");
    both_find(&duck, &ch, "project deleted", [&[], &[], &[], &[]]).await;
}
