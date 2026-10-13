/// Log records' messages rebuilt from their raw records are answered alike on both backends: the records that carry
/// messages (`messages_bytes`, which replaces `messages != '[]'`), the messages each span's view joins, a framed
/// request's frames and the byte length the frame budget sums - for records whose messages are left unstored, as an
/// ingest leaves nearly all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuilt_log_messages_are_answered_alike_on_both_backends() {
    const DEFAULT_PROJECT: &str = sideseat_core::constants::DEFAULT_PROJECT_ID;
    use prost::Message;
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let clickhouse = clickhouse_backend(&url, "sideseat_parity_rebuilt_log_messages").await;
    let (_duck_dir, duckdb) = duckdb_backend().await;
    let duck_repo: Arc<dyn AnalyticsRepository + Send + Sync> = Arc::new(duckdb.clone());
    let ch_repo: Arc<dyn AnalyticsRepository + Send + Sync> = Arc::new(clickhouse.clone());

    // Every captured logs export: the records that carry messages are those a declared log event recognises.
    fn walk(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("fixture directory")
            .map(|entry| entry.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, found);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("logs-") && name.ends_with(".pb"))
            {
                found.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/messages"),
        &mut paths,
    );
    let mut exports = Vec::new();
    for path in paths {
        let bytes = std::fs::read(&path).expect("a capture");
        let mut request =
            opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest::decode(
                bytes.as_slice(),
            )
            .expect("an OTLP export");
        // The project an ingest stores into: the stores' default one, which needs no registration.
        sideseat_ingestion::otlp::inject_project_id_logs(&mut request, DEFAULT_PROJECT);
        // Within ClickHouse's TTL, which reaps a captured record's own months-old time: every time moved by one
        // shift, so the records keep their order.
        let records = || {
            request
                .resource_logs
                .iter()
                .flat_map(|resource| &resource.scope_logs)
                .flat_map(|scope| &scope.log_records)
        };
        let earliest = records()
            .flat_map(|record| [record.time_unix_nano, record.observed_time_unix_nano])
            .filter(|&t| t > 0)
            .min()
            .unwrap_or(0);
        let place = u64::try_from(
            (Utc::now() - chrono::TimeDelta::days(2))
                .timestamp_nanos_opt()
                .expect("an instant"),
        )
        .expect("after 1970");
        for record in request
            .resource_logs
            .iter_mut()
            .flat_map(|resource| &mut resource.scope_logs)
            .flat_map(|scope| &mut scope.log_records)
        {
            for time in [
                &mut record.time_unix_nano,
                &mut record.observed_time_unix_nano,
            ] {
                if *time > 0 {
                    *time = *time - earliest + place;
                }
            }
        }
        exports.push(request);
    }
    assert!(exports.len() > 200, "the captures are missing");
    for repo in [&duck_repo, &ch_repo] {
        let FilePipeline {
            _temp,
            transactional,
            files,
            ..
        } = file_pipeline(repo).await;
        for request in &exports {
            sideseat_ingestion::logs::ingest(
                request,
                repo.as_ref(),
                transactional.as_ref(),
                Some(&files),
                Utc::now(),
            )
            .await
            .expect("the export is stored");
        }
    }

    // The messages are left unstored, and the length each record carries is the one its messages have.
    let (carriers, unstored, lengths): (i64, i64, Vec<(String, u32)>) = {
        let conn = duckdb.0.conn();
        let count = |sql: &str| {
            conn.query_row(sql, [], |row| row.get::<_, i64>(0))
                .expect(sql)
        };
        let mut statement = conn
            .prepare(
                "SELECT log_digest, messages_bytes FROM otel_logs WHERE messages_bytes > 0 \
                 ORDER BY log_digest, ordinal",
            )
            .expect("the carriers");
        let lengths = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("the carriers")
            .collect::<Result<Vec<_>, _>>()
            .expect("the carriers");
        (
            count("SELECT count(*) FROM otel_logs WHERE messages_bytes > 0"),
            // Unstored; or stored because they hold a file reference a release reads them for, or because the
            // raw record is far larger than they are (`ingest::settle_views`).
            count(
                "SELECT count(*) FROM otel_logs WHERE messages_bytes > 0 \
                 AND (messages IS NULL OR contains(messages, '#!B64!#') \
                      OR strlen(CAST(raw_log AS VARCHAR)) + coalesce(strlen(scope_name), 0) \
                      + coalesce(strlen(noted_references), 0) > 2 * messages_bytes + 1024)",
            ),
            lengths,
        )
    };
    assert!(carriers > 10, "{carriers} records carry messages");
    assert_eq!(
        unstored, carriers,
        "a carrier's messages were stored without a reason to keep them"
    );
    let ch_lengths: Vec<(String, u32)> = raw_client(&url, "sideseat_parity_rebuilt_log_messages")
        .with_option(
            sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
            "1",
        )
        .query(
            "SELECT log_digest, messages_bytes FROM otel_logs FINAL WHERE messages_bytes > 0 \
             ORDER BY log_digest, ordinal",
        )
        .fetch_all()
        .await
        .expect("the ClickHouse carriers");
    let only_duck: Vec<_> = lengths.iter().filter(|l| !ch_lengths.contains(l)).collect();
    let only_ch: Vec<_> = ch_lengths.iter().filter(|l| !lengths.contains(l)).collect();
    assert!(
        only_duck.is_empty() && only_ch.is_empty(),
        "the backends carry different records or lengths: DuckDB alone {only_duck:?}, ClickHouse alone {only_ch:?}"
    );

    // The spans the carriers name, so a span's view has a row to join their messages to.
    let carrier_spans: Vec<(String, String)> = {
        let conn = duckdb.0.conn();
        let mut statement = conn
            .prepare(
                "SELECT DISTINCT trace_id, span_id FROM otel_logs WHERE messages_bytes > 0 \
                 ORDER BY 1, 2",
            )
            .expect("the carriers' spans");
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("the carriers' spans")
            .collect::<Result<_, _>>()
            .expect("the carriers' spans")
    };
    let spans: Vec<NormalizedSpan> = carrier_spans
        .iter()
        .enumerate()
        .map(|(index, (trace, span))| plain_span(DEFAULT_PROJECT, trace, span, index as i64))
        .collect();
    for backend in [
        &clickhouse as &(dyn AnalyticsRepository + Send + Sync),
        &duckdb as &(dyn AnalyticsRepository + Send + Sync),
    ] {
        backend
            .insert_spans(spans.clone())
            .await
            .expect("the spans");
    }

    // Every trace's message view, and every framed key's frames, answered through the domain on each backend.
    let duck_view = sideseat_domain::dedup::DedupAnalyticsRepository::new(Box::new(duckdb.clone()));
    let ch_view =
        sideseat_domain::dedup::DedupAnalyticsRepository::new(Box::new(clickhouse.clone()));
    let (traces, keys): (Vec<String>, Vec<(String, String)>) = {
        let conn = duckdb.0.conn();
        let strings = |sql: &str| -> Vec<String> {
            let mut statement = conn.prepare(sql).expect(sql);
            statement
                .query_map([], |row| row.get(0))
                .expect(sql)
                .collect::<Result<_, _>>()
                .expect(sql)
        };
        let traces =
            strings("SELECT DISTINCT trace_id FROM otel_logs WHERE messages_bytes > 0 ORDER BY 1");
        let mut statement = conn
            .prepare(
                "SELECT DISTINCT trace_id, CAST(frame_key AS VARCHAR) FROM otel_logs \
                 WHERE frame_key IS NOT NULL ORDER BY 1, 2",
            )
            .expect("the keys");
        let keys = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("the keys")
            .collect::<Result<_, _>>()
            .expect("the keys");
        (traces, keys)
    };
    assert!(!traces.is_empty());
    let mut answered = 0usize;
    for trace in &traces {
        let params = MessageQueryParams {
            project_id: ProjectId::from(DEFAULT_PROJECT),
            trace_id: Some(trace.clone()),
            ..Default::default()
        };
        let duck_rows = duck_view
            .get_messages(&params)
            .await
            .expect("duckdb messages");
        let ch_rows = ch_view
            .get_messages(&params)
            .await
            .expect("clickhouse messages");
        let describe = |rows: &[MessageSpanRow]| -> Vec<(String, String)> {
            let mut rows: Vec<_> = rows
                .iter()
                .map(|row| (row.span_id.clone(), canonical_json(&row.log_messages_json)))
                .collect();
            rows.sort();
            rows
        };
        assert_eq!(
            describe(&duck_rows.rows),
            describe(&ch_rows.rows),
            "trace {trace}"
        );
        answered += duck_rows
            .rows
            .iter()
            .filter(|row| row.log_messages_json != "[]")
            .count();
    }
    for (trace, key) in &keys {
        let params = sideseat_ports::types::RequestFramesParams {
            project_id: ProjectId::from(DEFAULT_PROJECT),
            trace_id: trace.clone(),
            key: key.parse().expect("a stored key"),
            ingested_before_us: None,
        };
        let duck_frames = duck_view
            .get_request_frames(&params)
            .await
            .expect("duckdb frames");
        let ch_frames = ch_view
            .get_request_frames(&params)
            .await
            .expect("clickhouse frames");
        // The budget sums the length of what each frame's messages are, rebuilt or not.
        for frame in &duck_frames {
            let messages = frame.messages_json.as_deref().expect("within the budget");
            assert!(
                lengths.contains(&(frame.log_digest.clone(), messages.len() as u32)),
                "frame {} carries {} bytes, not its stored length",
                frame.log_digest,
                messages.len()
            );
        }
        assert_eq!(duck_frames, ch_frames, "trace {trace} key {key}");
    }
    // Every carrier's span joins its messages: an answer that lost them would read as none.
    assert_eq!(
        answered,
        carrier_spans.len(),
        "a span's view joins no messages from the records that carry them"
    );

    // The project feed, paged at several sizes through the whole project - many pages, since the project holds
    // more spans than one - reads the same pages, rows and log messages on both backends.
    for size in [1u32, 7, 20, 50] {
        let mut pages = Vec::new();
        for view in [&duck_view, &ch_view] {
            let mut cursor = None;
            let mut read = Vec::new();
            loop {
                let page = view
                    .get_project_messages(&sideseat_ports::types::FeedMessagesParams {
                        project_id: ProjectId::from(DEFAULT_PROJECT),
                        limit: size,
                        cursor: cursor.clone(),
                        ..Default::default()
                    })
                    .await
                    .expect("a feed page");
                let Some(last) = page.rows.last() else {
                    break;
                };
                cursor = Some((
                    last.ingested_at.timestamp_micros(),
                    last.span_id.clone(),
                    last.trace_id.clone(),
                ));
                read.push(
                    page.rows
                        .iter()
                        .map(|row| {
                            format!("{} {}", describe_message_row(row), row.log_messages_json)
                        })
                        .collect::<Vec<_>>(),
                );
                if read.len() > 10_000 {
                    panic!("the feed does not end");
                }
            }
            pages.push(read);
        }
        assert!(
            pages[0].len() > 1,
            "page size {size}: one page holds the project"
        );
        assert_eq!(pages[0], pages[1], "page size {size}");
        let joined = pages[0]
            .iter()
            .flatten()
            .filter(|row| !row.ends_with(" []"))
            .count();
        assert_eq!(
            joined,
            carrier_spans.len(),
            "page size {size}: spans with log messages"
        );
    }

    // A request's frames whose messages are rebuilt: carriers from the captures, framed under one key in one trace.
    let framed: Vec<sideseat_ports::types::NormalizedLog> = exports
        .iter()
        .flat_map(|request| sideseat_ingestion::logs::extract_logs_batch(request, Utc::now()))
        .filter(|log| log.messages.is_some())
        // Within the frame read's record bound, so every one is returned.
        .take(sideseat_core::constants::REQUEST_FRAMES_MAX_RECORDS)
        .map(|mut log| {
            log.trace_id = Some("framed-trace".to_string());
            log.frame_key = Some(4242);
            log.messages_rebuild_from_raw = true;
            log
        })
        .collect();
    assert_eq!(
        framed.len(),
        sideseat_core::constants::REQUEST_FRAMES_MAX_RECORDS,
        "framed carriers"
    );
    for backend in [
        &clickhouse as &(dyn AnalyticsRepository + Send + Sync),
        &duckdb as &(dyn AnalyticsRepository + Send + Sync),
    ] {
        backend
            .insert_logs(&framed)
            .await
            .expect("the framed carriers");
    }
    let params = sideseat_ports::types::RequestFramesParams {
        project_id: ProjectId::from(DEFAULT_PROJECT),
        trace_id: "framed-trace".to_string(),
        key: 4242,
        ingested_before_us: None,
    };
    let duck_frames = duck_view
        .get_request_frames(&params)
        .await
        .expect("duckdb frames");
    let ch_frames = ch_view
        .get_request_frames(&params)
        .await
        .expect("clickhouse frames");
    assert_eq!(duck_frames.len(), framed.len());
    assert_eq!(
        duck_frames, ch_frames,
        "the framed carriers read differently"
    );
    for frame in &duck_frames {
        let log = framed
            .iter()
            .find(|log| log.log_digest == frame.log_digest && log.ordinal == frame.ordinal)
            .expect("a framed carrier");
        assert_eq!(
            frame.messages_json, log.messages,
            "frame {}",
            frame.log_digest
        );
    }
}

/// One record's messages as the typed aggregate answers them: (messages, raw record, scope, noted references).
type SpanSource = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// A span's log messages as `log_message_sources_of_spans` answers them, with its carrier token.
#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct SpanSources {
    #[allow(dead_code)]
    log_trace_id: String,
    #[allow(dead_code)]
    log_span_id: String,
    aggregated: Vec<SpanSource>,
    carriers: u64,
    carrier_token: u64,
}

/// The carrier token a feed page checks its messages by moves with every revision that changes what a span joins -
/// a redelivery of a carrier, a new carrier - and the read answers the winning delivery's messages.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_span_s_carrier_token_moves_with_what_it_joins() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_carrier_token";
    let clickhouse = clickhouse_backend(&url, database).await;
    let client = raw_client(&url, database).with_option(
        sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
        "1",
    );
    let carrier = |digest: char, messages: &str, ingested: DateTime<Utc>| {
        sideseat_ports::types::NormalizedLog {
            project_id: Some(PROJECT.to_string()),
            log_digest: digest.to_string().repeat(64),
            trace_id: Some("token-trace".to_string()),
            span_id: Some("token-span".to_string()),
            timestamp: Utc::now() - chrono::Duration::days(1),
            time: Some(Utc::now() - chrono::Duration::days(1)),
            ingested_at: Some(ingested),
            messages: Some(messages.to_string()),
            ..Default::default()
        }
    };
    let read = || {
        let client = client.clone();
        async move {
            let query = sideseat_query_sql::messages::log_message_sources_of_spans(
                PROJECT,
                None,
                &[("token-trace".to_string(), "token-span".to_string())],
            )
            .expect("a statement");
            let mut statement = client.query(query.sql());
            for value in query.params() {
                statement = match value {
                    sideseat_query_sql::analytics::QueryValue::String(text) => {
                        statement.bind(text.as_str())
                    }
                    other => panic!("an unexpected value: {other:?}"),
                };
            }
            let mut rows = statement
                .fetch_all::<SpanSources>()
                .await
                .expect("the span");
            assert_eq!(rows.len(), 1);
            rows.remove(0)
        }
    };
    let start = Utc::now() - chrono::Duration::hours(3);
    clickhouse
        .insert_logs(&[carrier('a', r#"[{"seen":"first"}]"#, start)])
        .await
        .expect("a carrier");
    let first = read().await;
    assert_eq!(first.carriers, 1);
    assert_eq!(
        first.aggregated[0].0.as_deref(),
        Some(r#"[{"seen":"first"}]"#)
    );
    clickhouse
        .insert_logs(&[carrier(
            'a',
            r#"[{"seen":"again"}]"#,
            start + chrono::Duration::hours(1),
        )])
        .await
        .expect("a redelivery");
    let redelivered = read().await;
    assert_eq!(redelivered.carriers, 1);
    assert_ne!(redelivered.carrier_token, first.carrier_token);
    assert_eq!(
        redelivered.aggregated[0].0.as_deref(),
        Some(r#"[{"seen":"again"}]"#)
    );
    clickhouse
        .insert_logs(&[carrier('b', r#"[{"seen":"another"}]"#, start)])
        .await
        .expect("another carrier");
    let more = read().await;
    assert_eq!(more.carriers, 2);
    assert_ne!(more.carrier_token, redelivered.carrier_token);
    // The same record at the same instant with its messages in another order - a key-value list's members
    // reordered, which the identity sorts - is another token, though the messages are as long.
    clickhouse
        .insert_logs(&[carrier('b', r#"[{"a":"1","b":"2"}]"#, start)])
        .await
        .expect("the record");
    let ordered = read().await;
    clickhouse
        .insert_logs(&[carrier('b', r#"[{"b":"2","a":"1"}]"#, start)])
        .await
        .expect("the same record, reordered");
    let reordered = read().await;
    assert_eq!(reordered.carriers, ordered.carriers);
    assert_ne!(reordered.carrier_token, ordered.carrier_token);
}

/// A feed page whose span has many carriers reads the same on both backends: every one of the span's log messages, on every row of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_feed_page_with_many_carriers_reads_alike_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let clickhouse = clickhouse_backend(&url, "sideseat_parity_feed_many_carriers").await;
    let (_duck_dir, duckdb) = duckdb_backend().await;
    let carriers: Vec<sideseat_ports::types::NormalizedLog> = (0..1_100)
        .map(|n| sideseat_ports::types::NormalizedLog {
            project_id: Some(PROJECT.to_string()),
            log_digest: format!("{n:064}"),
            trace_id: Some("busy-trace".to_string()),
            span_id: Some("busy-span".to_string()),
            timestamp: Utc::now() - chrono::Duration::hours(3) + chrono::Duration::milliseconds(n),
            time: Some(Utc::now() - chrono::Duration::hours(3) + chrono::Duration::milliseconds(n)),
            ingested_at: Some(Utc::now() - chrono::Duration::hours(2)),
            messages: Some(format!(r#"[{{"n":{n}}}]"#)),
            ..Default::default()
        })
        .collect();
    for backend in [
        &clickhouse as &(dyn AnalyticsRepository + Send + Sync),
        &duckdb as &(dyn AnalyticsRepository + Send + Sync),
    ] {
        backend.insert_logs(&carriers).await.expect("the carriers");
        backend
            .insert_spans(vec![plain_span(PROJECT, "busy-trace", "busy-span", 0)])
            .await
            .expect("the span");
    }
    let feed = sideseat_ports::types::FeedMessagesParams {
        project_id: ProjectId::from(PROJECT),
        limit: 10,
        ..Default::default()
    };
    let duck_view = sideseat_domain::dedup::DedupAnalyticsRepository::new(Box::new(duckdb.clone()));
    let ch_view =
        sideseat_domain::dedup::DedupAnalyticsRepository::new(Box::new(clickhouse.clone()));
    let duck_page = duck_view
        .get_project_messages(&feed)
        .await
        .expect("duckdb feed");
    let ch_page = ch_view
        .get_project_messages(&feed)
        .await
        .expect("clickhouse feed");
    let describe = |page: &sideseat_ports::types::MessageQueryResult| -> Vec<String> {
        page.rows
            .iter()
            .map(|row| format!("{} {}", describe_message_row(row), row.log_messages_json))
            .collect()
    };
    assert_eq!(describe(&duck_page), describe(&ch_page));
    let joined: Vec<serde_json::Value> =
        serde_json::from_str(&ch_page.rows[0].log_messages_json).expect("an array");
    assert_eq!(joined.len(), carriers.len());
}
