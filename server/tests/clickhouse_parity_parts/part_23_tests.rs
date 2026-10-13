/// Search pages read their text a chunk at a time and agree with DuckDB on every page: more candidates than one
/// chunk holds, most of them sharing a timestamp so only the identity orders them, paged a few hits at a time and
/// all at once, with `examined` and the cursors equal on both backends and the order the documented one.
#[tokio::test]
async fn clickhouse_search_pages_agree_across_chunks_and_ties() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_search_chunks").await;
    let spans_total = sideseat_ports::types::SEARCH_SOURCE_CHUNK_ROWS + 77;
    let mut spans: Vec<NormalizedSpan> = (0..spans_total)
        .map(|n| NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: format!("chunk-{:03}", n % 50),
            span_id: format!("s{n:04}"),
            span_name: "chunked".to_string(),
            // Three instants for every span: ties decided by trace and span id.
            timestamp_start: ts(700 + (n % 3) as i64),
            input_preview: Some(if n % 4 == 0 {
                "plain words".to_string()
            } else {
                "needle words".to_string()
            }),
            exception_stacktrace: Some(format!("needle {}", "x".repeat(n * 37 % 4096))),
            ingested_at: Some(ts(800)),
            // Small enough that a chunk holds its most rows, so the page is read in several.
            logical_bytes: 2048,
            ..Default::default()
        })
        .collect();
    sideseat_domain::search::index_spans(&mut spans);
    let mut expected: Vec<(i64, String, String)> = spans
        .iter()
        .map(|span| {
            (
                span.timestamp_start.timestamp_micros(),
                span.trace_id.clone(),
                span.span_id.clone(),
            )
        })
        .collect();
    expected.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    duck.insert_spans(spans.clone())
        .await
        .expect("duckdb insert");
    ch.insert_spans(spans).await.expect("clickhouse insert");

    async fn page(
        repository: &impl SearchIndex,
        limit: u32,
        cursor: Option<sideseat_ports::types::SearchCursor>,
    ) -> (
        Vec<(String, String)>,
        Option<sideseat_ports::types::SearchCursor>,
        u32,
        bool,
    ) {
        let page = sideseat_domain::search::SearchService::execute(
            repository,
            &SearchQuery {
                project_id: ProjectId::from(PROJECT),
                signal: SearchSignal::Spans,
                expression: sideseat_domain::search::parse("needle", SearchSignal::Spans).unwrap(),
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
            .map(|hit| {
                let SearchRecord::Span(span) = hit.record else {
                    panic!("span search returned a log");
                };
                (span.trace_id, span.span_id)
            })
            .collect();
        (
            hits,
            page.next_cursor,
            page.examined,
            page.examination_limit_reached,
        )
    }

    let all_duck = page(&duck, 10_000, None).await;
    let all_ch = page(&ch, 10_000, None).await;
    assert_eq!(all_duck, all_ch);
    assert_eq!(all_duck.2 as usize, spans_total, "every span examined once");
    assert_eq!(
        all_duck.0,
        expected
            .iter()
            .map(|(_, trace, span)| (trace.clone(), span.clone()))
            .collect::<Vec<_>>(),
        "every span holds the needle, in timestamp, trace, span order"
    );

    let (mut duck_cursor, mut ch_cursor) = (None, None);
    let mut paged = Vec::new();
    loop {
        let duck_page = page(&duck, 37, duck_cursor.clone()).await;
        let ch_page = page(&ch, 37, ch_cursor.clone()).await;
        assert_eq!(duck_page, ch_page);
        if duck_page.0.is_empty() {
            break;
        }
        paged.extend(duck_page.0);
        duck_cursor = duck_page.1;
        ch_cursor = ch_page.1;
    }
    assert_eq!(
        paged, all_duck.0,
        "pages of 37 hits read the whole page in order"
    );
}
