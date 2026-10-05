// ============================================================================
// Log-carried messages, joined to their span at read time
// ============================================================================

/// A raw message as log ingestion stores it: the same shape a span event produces.
fn log_carried(text: &str, second: i64) -> String {
    serde_json::json!([{
        "source": {"event": {"name": "gen_ai.user.message",
                             "time": format!("2026-01-01T00:00:{second:02}Z")}},
        "content": {"content": text}
    }])
    .to_string()
}

fn message_log(
    project: &str,
    digest: &str,
    (trace, span): (&str, &str),
    second: i64,
    ingested_at: DateTime<Utc>,
    messages: &str,
) -> NormalizedLog {
    NormalizedLog {
        project_id: Some(project.to_string()),
        log_digest: digest.to_string(),
        timestamp: ts(second),
        time: Some(ts(second)),
        trace_id: Some(trace.to_string()),
        span_id: Some(span.to_string()),
        event_name: Some("gen_ai.user.message".to_string()),
        ingested_at: Some(ingested_at),
        messages: Some(messages.to_string()),
        ..Default::default()
    }
}

fn plain_span(project: &str, trace: &str, span: &str, offset: i64) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: trace.to_string(),
        span_id: span.to_string(),
        span_name: "chat".to_string(),
        session_id: Some(format!("{project}-session")),
        timestamp_start: ts(offset),
        timestamp_end: Some(ts(offset + 1)),
        duration_ms: 1000,
        observation_type: Some(ObservationType::Generation),
        ingested_at: Some(ts(offset)),
        ..Default::default()
    }
}

/// What a message read returns, reduced to what this suite is about.
fn joined(rows: &[MessageSpanRow]) -> Vec<(String, String, Vec<String>)> {
    rows.iter()
        .map(|row| {
            let messages: Vec<serde_json::Value> = serde_json::from_str(&row.log_messages_json)
                .unwrap_or_else(|e| panic!("log_messages_json is not an array: {e}: {row:?}"));
            (
                row.trace_id.clone(),
                row.span_id.clone(),
                messages
                    .iter()
                    .map(|message| {
                        message["content"]["content"]
                            .as_str()
                            .unwrap_or("?")
                            .to_string()
                    })
                    .collect(),
            )
        })
        .collect()
}

/// Every message read, for one project, as `(label, rows)`.
async fn message_reads(
    backend: &(impl MessageStore + ?Sized),
    project: &str,
    watermark: Option<i64>,
) -> Vec<(&'static str, Vec<(String, String, Vec<String>)>)> {
    let project_id = ProjectId::from(project);
    let base = MessageQueryParams {
        project_id: project_id.clone(),
        ingested_before_us: watermark,
        ..Default::default()
    };
    let mut out = Vec::new();
    for (label, params) in [
        (
            "span",
            MessageQueryParams {
                span_id: Some("span-late-log".to_string()),
                trace_id: Some(format!("{project}-trace")),
                ..base.clone()
            },
        ),
        (
            "trace",
            MessageQueryParams {
                trace_id: Some(format!("{project}-trace")),
                ..base.clone()
            },
        ),
        (
            "session",
            MessageQueryParams {
                session_id: Some(format!("{project}-session")),
                ..base.clone()
            },
        ),
        (
            "trace_ids",
            MessageQueryParams {
                trace_ids: Some(vec![format!("{project}-trace")]),
                ..base.clone()
            },
        ),
    ] {
        let rows = backend
            .get_messages(&params)
            .await
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        out.push((label, joined(&rows.rows)));
    }
    let feed = backend
        .get_project_messages(&sideseat_ports::types::FeedMessagesParams {
            project_id,
            limit: 50,
            ingested_before_us: watermark,
            ..Default::default()
        })
        .await
        .expect("feed");
    out.push(("feed", joined(&feed.rows)));
    out
}

/// One project's fixture: logs written *before* their spans, a re-sent record, an orphan, an empty record and
/// a span whose conversation arrives only as logs.
async fn seed_log_messages(backend: &(impl AnalyticsRepository + ?Sized), project: &str) {
    let trace = format!("{project}-trace");
    let logs = vec![
        message_log(
            project,
            "d-second",
            (&trace, "span-late-log"),
            12,
            ts(5),
            &log_carried("second", 12),
        ),
        message_log(
            project,
            "d-first",
            (&trace, "span-late-log"),
            11,
            ts(5),
            &log_carried("first", 11),
        ),
        // The record nobody has a span for yet: stored, and attached to nothing.
        message_log(
            project,
            "d-orphan",
            (&trace, "span-missing"),
            11,
            ts(5),
            &log_carried("orphan", 11),
        ),
        // Recognised, but carrying nothing: does not make its span pass the content filter.
        message_log(project, "d-empty", (&trace, "span-quiet"), 11, ts(5), "[]"),
    ];
    backend
        .insert_logs(&logs)
        .await
        .expect("logs before their spans");
    // A re-sent record: the same identity, delivered again later. One record, not two.
    backend
        .insert_logs(&[message_log(
            project,
            "d-first",
            (&trace, "span-late-log"),
            11,
            ts(6),
            &log_carried("first", 11),
        )])
        .await
        .expect("re-sent log");
    backend
        .insert_spans(vec![
            plain_span(project, &trace, "span-late-log", 10),
            plain_span(project, &trace, "span-quiet", 20),
        ])
        .await
        .expect("spans after their logs");
}

/// Log records attach to the span they name the same way on both backends - whichever arrived first, with a
/// re-sent record counted once, in log-time order, and for every message read.
#[tokio::test]
async fn log_messages_join_their_span_the_same_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_log_messages").await;
    seed_log_messages(&duck, PROJECT).await;
    seed_log_messages(&ch, PROJECT).await;

    let expected = vec![(
        format!("{PROJECT}-trace"),
        "span-late-log".to_string(),
        vec!["first".to_string(), "second".to_string()],
    )];
    let duck_reads = message_reads(&duck, PROJECT, None).await;
    let ch_reads = message_reads(&ch, PROJECT, None).await;
    for ((label, duck_rows), (_, ch_rows)) in duck_reads.iter().zip(&ch_reads) {
        assert_eq!(duck_rows, ch_rows, "{label} differs between backends");
        assert_eq!(
            duck_rows, &expected,
            "{label}: the log-only span is returned once, with its messages in log-time order, and the \
             quiet span is filtered out"
        );
    }
}

/// The watermark bounds the joined log records exactly as it bounds span rows, on both backends: a record
/// first delivered after it is not part of the instant, and a re-delivered record is versioned at its newest
/// delivery, because that is the only version DuckDB keeps.
#[tokio::test]
async fn the_watermark_bounds_log_messages_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_log_watermark").await;
    for backend in [&duck as &dyn AnalyticsRepository, &ch] {
        seed_log_messages(backend, PROJECT).await;
        backend
            .insert_logs(&[message_log(
                PROJECT,
                "d-after",
                (&format!("{PROJECT}-trace"), "span-late-log"),
                13,
                ts(100),
                &log_carried("after", 13),
            )])
            .await
            .expect("a log after the watermark");
    }

    let watermark = Some(ts(50).timestamp_micros());
    let duck_reads = message_reads(&duck, PROJECT, watermark).await;
    let ch_reads = message_reads(&ch, PROJECT, watermark).await;
    for ((label, duck_rows), (_, ch_rows)) in duck_reads.iter().zip(&ch_reads) {
        assert_eq!(
            duck_rows, ch_rows,
            "{label} differs between backends at the watermark"
        );
        assert_eq!(
            duck_rows.first().map(|row| row.2.clone()),
            Some(vec!["first".to_string(), "second".to_string()]),
            "{label}: the record delivered after the watermark is not joined"
        );
    }
    let unbounded = message_reads(&duck, PROJECT, None).await;
    assert_eq!(
        unbounded[1].1[0].2,
        vec!["first", "second", "after"],
        "without the bound it is there, so the assertion above is about the bound"
    );

    // Re-delivered after the watermark: the newest delivery is the version both backends answer with.
    for backend in [&duck as &dyn AnalyticsRepository, &ch] {
        backend
            .insert_logs(&[message_log(
                PROJECT,
                "d-second",
                (&format!("{PROJECT}-trace"), "span-late-log"),
                12,
                ts(100),
                &log_carried("second", 12),
            )])
            .await
            .expect("a re-delivery after the watermark");
    }
    let duck_reads = message_reads(&duck, PROJECT, watermark).await;
    let ch_reads = message_reads(&ch, PROJECT, watermark).await;
    for ((label, duck_rows), (_, ch_rows)) in duck_reads.iter().zip(&ch_reads) {
        assert_eq!(
            duck_rows, ch_rows,
            "{label}: a re-delivered record must be versioned the same way on both backends"
        );
        assert_eq!(
            duck_rows.first().map(|row| row.2.clone()),
            Some(vec!["first".to_string()]),
            "{label}: the stated semantics - a record is as old as its newest delivery"
        );
    }
}

/// The join on a two-shard cluster: the log aggregate reads the `Distributed` table, the session scope is a
/// distributed subquery, and each project lives on a different shard - so a read that saw only the shard the
/// client reached, or a subquery `distributed_product_mode` refuses, fails here and nowhere else.
#[tokio::test]
async fn log_messages_join_across_a_two_shard_cluster() {
    let Ok(url) = std::env::var(TWO_SHARD_URL_ENV) else {
        eprintln!(
            "clickhouse two-shard: skipped - set {TWO_SHARD_URL_ENV} (or run \
             `make test-clickhouse-two-shard`)"
        );
        return;
    };
    let database = "sideseat_two_shard_log_messages";
    let service = replicated_backend_at(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let client = raw_client_at(&url, database, &user, &password);

    let mut per_shard: [Option<String>; 2] = [None, None];
    for n in 0..64 {
        let candidate = format!("log-shard-{n}");
        let shard: Vec<u64> = client
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

    let (_temp, duck) = duckdb_backend().await;
    for project in [&near, &far] {
        seed_log_messages(&duck, project).await;
        seed_log_messages(&service, project).await;
    }
    for project in [&near, &far] {
        for watermark in [None, Some(ts(50).timestamp_micros())] {
            let duck_reads = message_reads(&duck, project, watermark).await;
            let ch_reads = message_reads(&service, project, watermark).await;
            for ((label, duck_rows), (_, ch_rows)) in duck_reads.iter().zip(&ch_reads) {
                assert_eq!(
                    duck_rows, ch_rows,
                    "{project} {label} {watermark:?}: the two-shard answer differs from DuckDB's"
                );
                assert_eq!(
                    duck_rows.first().map(|row| row.2.clone()),
                    Some(vec!["first".to_string(), "second".to_string()]),
                    "{project} {label}: the log messages were not joined"
                );
            }
        }
    }
}
