/// A revision written after a later one - a worker writing the copy it loaded - is stored at its receipt, so it
/// is superseded at once on both backends, and both answer the later revision as the winner settlement reads.
#[tokio::test]
async fn a_revision_written_after_a_later_one_stays_superseded_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_receipt_order").await;

    let base = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "trace-receipt-order".to_string(),
        span_id: "span-1".to_string(),
        span_name: "generation".to_string(),
        timestamp_start: ts(0),
        timestamp_end: Some(ts(1)),
        duration_ms: 1000,
        ..Default::default()
    };
    let earlier = NormalizedSpan {
        gen_ai_usage_total_tokens: 100,
        content_digest: "earlier".to_string(),
        ingested_at: Some(ts(10)),
        ..base.clone()
    };
    let later = NormalizedSpan {
        gen_ai_usage_total_tokens: 900,
        content_digest: "later".to_string(),
        ingested_at: Some(ts(20)),
        ..base
    };

    let identity = ("trace-receipt-order".to_string(), "span-1".to_string());
    let mut answers = Vec::new();
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        // The later revision first, then the earlier one's late copy.
        repo.insert_spans(vec![later.clone()])
            .await
            .expect("later revision");
        repo.insert_spans(vec![earlier.clone()])
            .await
            .expect("late copy");
        let trace = repo
            .get_trace(&ProjectId::from(PROJECT), "trace-receipt-order")
            .await
            .expect("trace")
            .expect("trace exists");
        let winners = repo
            .span_winners(&ProjectId::from(PROJECT), std::slice::from_ref(&identity))
            .await
            .expect("winners");
        let winner = winners.get(&identity).expect("a winner");
        answers.push((
            trace.total_tokens,
            winner.content_digest.clone(),
            winner.ingested_at,
        ));
    }
    assert_eq!(
        answers,
        vec![(900, "later".to_string(), ts(20)); 2],
        "the revision received later must stay the winner on both backends"
    );
}
