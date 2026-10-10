// ============================================================================
// The rows a request span's view is composed from
// ============================================================================

/// A request span of one thread: its delta, its reply, and the derived thread key an ingest would have stored.
fn thread_request(
    project: &str,
    thread: &str,
    span: &str,
    offset: i64,
    delta: &str,
    reply: &str,
) -> NormalizedSpan {
    let messages = serde_json::json!([
        {"source": {"attribute": {"key": "new_context", "time": ts(offset)}},
         "content": {"role": "user", "content": delta}},
        {"source": {"attribute": {"key": "response.model_output", "time": ts(offset + 1)}},
         "content": {"role": "assistant", "content": reply}},
    ]);
    NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: format!("{project}-{span}-trace"),
        span_id: span.to_string(),
        span_name: "claude_code.llm_request".to_string(),
        session_id: Some(format!("{project}-session")),
        timestamp_start: ts(offset),
        timestamp_end: Some(ts(offset + 1)),
        duration_ms: 1000,
        observation_type: Some(ObservationType::Generation),
        ingested_at: Some(ts(offset)),
        messages: Some(messages.to_string()),
        request_thread: thread.to_string(),
        span_marks: 0,
        ..Default::default()
    }
}

/// A tool span holding one call, keyed by the call id a later delta's result names.
fn call_span(project: &str, span: &str, offset: i64, call_id: &str) -> NormalizedSpan {
    let messages = serde_json::json!([
        {"source": {"attribute": {"key": "tool_name", "time": ts(offset)}},
         "content": {"role": "assistant",
                     "content": [{"type": "tool_use", "id": call_id, "name": "weather", "input": {}}]}},
    ]);
    NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: format!("{project}-{span}-trace"),
        span_id: span.to_string(),
        span_name: "claude_code.tool".to_string(),
        timestamp_start: ts(offset),
        timestamp_end: Some(ts(offset + 1)),
        duration_ms: 1000,
        observation_type: Some(ObservationType::Tool),
        ingested_at: Some(ts(offset)),
        messages: Some(messages.to_string()),
        gen_ai_tool_call_id: Some(call_id.to_string()),
        ..Default::default()
    }
}

/// Two threads of one session, a tool span each, and a request of the first thread delivered twice.
async fn seed_request_context(backend: &(impl AnalyticsRepository + ?Sized), project: &str) {
    backend
        .insert_spans(vec![
            // Marked, as the call below is: a composed row goes through the projections its own view does, so
            // both backends must hand a composition the marks each span was stored with.
            NormalizedSpan {
                span_marks: 0b1,
                ..thread_request(project, "thread-a", "req-a1", 10, "first", "reply one")
            },
            thread_request(project, "thread-a", "req-a2", 20, "second", "reply two"),
            thread_request(project, "thread-a", "req-a3", 30, "third", "reply three"),
            thread_request(project, "thread-b", "req-b1", 15, "other", "other reply"),
            NormalizedSpan {
                span_marks: 0b10,
                ..call_span(project, "tool-a", 12, "call-a")
            },
            call_span(project, "tool-b", 16, "call-b"),
        ])
        .await
        .expect("the threads' requests");
    // The same request delivered again: one row, the later revision, on both backends.
    backend
        .insert_spans(vec![NormalizedSpan {
            ingested_at: Some(ts(40)),
            ..thread_request(
                project,
                "thread-a",
                "req-a2",
                20,
                "second",
                "reply two, revised",
            )
        }])
        .await
        .expect("a re-sent request");
}

/// What a composed request's reads returned, reduced to what this suite is about: each row's span, with its marks
/// where it has any, and the thread rows' messages.
fn composed(rows: &RequestContextRows) -> (Vec<(String, String)>, Vec<String>) {
    let label = |row: &MessageSpanRow| match row.span_marks {
        0 => row.span_id.clone(),
        marks => format!("{} marks={marks:#b}", row.span_id),
    };
    let text = |row: &MessageSpanRow| {
        let messages: Vec<serde_json::Value> = serde_json::from_str(&row.messages_json)
            .unwrap_or_else(|e| panic!("messages_json is not an array: {e}: {row:?}"));
        messages
            .iter()
            .map(|message| {
                message["content"]["content"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| message["content"]["content"].to_string())
            })
            .collect::<Vec<_>>()
            .join("|")
    };
    (
        rows.thread
            .iter()
            .map(|row| (label(row), text(row)))
            .collect(),
        rows.calls.iter().map(label).collect(),
    )
}

async fn request_context(
    backend: &(impl MessageStore + ?Sized),
    project: &str,
    thread: &str,
    before_us: i64,
    call_ids: &[&str],
    watermark: Option<i64>,
) -> (Vec<(String, String)>, Vec<String>) {
    let rows = backend
        .get_request_context(&sideseat_ports::types::RequestContextParams {
            project_id: ProjectId::from(project),
            thread: thread.to_string(),
            before_us,
            call_ids: call_ids.iter().map(|id| (*id).to_string()).collect(),
            // The traces the thread's tool spans sit in - here one per call span, as `call_span` names them.
            call_trace_ids: vec![
                format!("{project}-tool-a-trace"),
                format!("{project}-tool-b-trace"),
            ],
            ingested_before_us: watermark,
        })
        .await
        .expect("the request context");
    composed(&rows)
}

/// A thread's earlier requests and the calls its deltas answer read the same on both backends: the thread's own
/// rows in sequence order, one row per identity at its latest revision, nothing of a sibling thread, nothing
/// after the target, and only the calls whose ids were asked for.
#[tokio::test]
async fn a_request_s_thread_and_calls_read_the_same_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_request_context").await;
    seed_request_context(&duck, PROJECT).await;
    seed_request_context(&ch, PROJECT).await;

    let before = ts(30).timestamp_micros();
    let duck_rows = request_context(&duck, PROJECT, "thread-a", before, &["call-a"], None).await;
    let ch_rows = request_context(&ch, PROJECT, "thread-a", before, &["call-a"], None).await;
    assert_eq!(duck_rows, ch_rows, "the two backends differ");
    assert_eq!(
        duck_rows,
        (
            vec![
                (
                    "req-a1 marks=0b1".to_string(),
                    "first|reply one".to_string()
                ),
                (
                    "req-a2".to_string(),
                    "second|reply two, revised".to_string()
                ),
                ("req-a3".to_string(), "third|reply three".to_string()),
            ],
            vec!["tool-a marks=0b10".to_string()],
        ),
        "the thread's own requests, in order, at their latest revision, and only the call asked for"
    );

    // The target's predecessors alone: a request later than the bound was not sent when it was.
    let earlier = ts(20).timestamp_micros();
    let duck_earlier = request_context(&duck, PROJECT, "thread-a", earlier, &[], None).await;
    let ch_earlier = request_context(&ch, PROJECT, "thread-a", earlier, &[], None).await;
    assert_eq!(duck_earlier, ch_earlier);
    assert_eq!(
        duck_earlier
            .0
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["req-a1 marks=0b1", "req-a2"],
        "nothing after the bound"
    );
    assert!(duck_earlier.1.is_empty(), "no ids asked for, no calls read");

    // A watermark answers with the instant it pins: the re-sent request's earlier revision, and on both alike.
    let watermark = Some(ts(35).timestamp_micros());
    let duck_pinned =
        request_context(&duck, PROJECT, "thread-a", before, &["call-a"], watermark).await;
    let ch_pinned = request_context(&ch, PROJECT, "thread-a", before, &["call-a"], watermark).await;
    assert_eq!(
        duck_pinned, ch_pinned,
        "the two backends differ under a watermark"
    );
    assert_eq!(
        duck_pinned
            .0
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        ["first|reply one", "second|reply two", "third|reply three"],
        "the revision the instant holds"
    );

    // A thread nothing carries reads nothing, rather than every span of the project.
    let empty = request_context(&duck, PROJECT, "thread-missing", before, &[], None).await;
    assert_eq!(empty, (Vec::new(), Vec::new()));
    assert_eq!(
        request_context(&ch, PROJECT, "thread-missing", before, &[], None).await,
        empty
    );
}

// ============================================================================
// The frame records a framed request's view opens with
// ============================================================================

/// A key past `i128::MAX`, as a stored key's digest mostly is: it must bind and compare as an unsigned 128-bit
/// integer on both backends.
const HIGH_KEY: u128 = u128::MAX - 11;

/// A second project whose frames share the first one's trace ids and keys.
const OTHER_FRAMES_PROJECT: &str = "parity-frames-other";

/// One frame record: a log record naming `span`, its frame key, and one message.
fn frame_record(
    project: &str,
    digest: &str,
    trace: &str,
    key: Option<u128>,
    offset: i64,
    text: &str,
) -> NormalizedLog {
    NormalizedLog {
        project_id: Some(project.to_string()),
        log_digest: digest.to_string(),
        ordinal: 0,
        timestamp: ts(offset),
        time: Some(ts(offset)),
        trace_id: Some(trace.to_string()),
        span_id: Some(format!("span-{digest}")),
        ingested_at: Some(ts(offset)),
        messages: Some(
            serde_json::json!([{"source": {"event": {"name": "system_prompt", "time": ts(offset)}},
                                 "content": {"role": "system", "content": format!("{text} ({project})")}}])
            .to_string(),
        ),
        frame_key: key,
        ..Default::default()
    }
}

async fn seed_frames(backend: &(impl AnalyticsRepository + ?Sized), project: &str) {
    backend
        .insert_logs(&[
            frame_record(project, "f-b", "frame-trace", Some(1), 20, "second"),
            frame_record(project, "f-a", "frame-trace", Some(1), 10, "first"),
            frame_record(project, "f-k2", "frame-trace", Some(2), 5, "another key"),
            frame_record(project, "f-t2", "other-trace", Some(1), 5, "another trace"),
            frame_record(project, "f-none", "frame-trace", None, 5, "frames nothing"),
        ])
        .await
        .expect("the frame records");
    // The same record delivered again: one row, the later revision, on both backends.
    backend
        .insert_logs(&[NormalizedLog {
            ingested_at: Some(ts(40)),
            ..frame_record(project, "f-a", "frame-trace", Some(1), 10, "first, revised")
        }])
        .await
        .expect("a re-sent record");
    // Past the byte bound: a small record, then one whose bytes would carry the total over it.
    let big = "x".repeat(sideseat_core::constants::REQUEST_FRAMES_MAX_BYTES);
    backend
        .insert_logs(&[
            frame_record(project, "f-big-1", "big-trace", Some(HIGH_KEY), 1, "small"),
            frame_record(project, "f-big-2", "big-trace", Some(HIGH_KEY), 2, &big),
        ])
        .await
        .expect("the large frames");
    // A request span carrying the key its frames are read by, as an ingest would have stored it.
    backend
        .insert_spans(vec![NormalizedSpan {
            request_frame: "k1".to_string(),
            ..thread_request(project, "", "framed-request", 30, "question", "answer")
        }])
        .await
        .expect("the framed request");
}

async fn frames(
    backend: &(impl MessageStore + ?Sized),
    project: &str,
    trace: &str,
    key: u128,
    watermark: Option<i64>,
) -> Vec<(String, String, String)> {
    backend
        .get_request_frames(&sideseat_ports::types::RequestFramesParams {
            project_id: ProjectId::from(project),
            trace_id: trace.to_string(),
            key,
            ingested_before_us: watermark,
        })
        .await
        .expect("the frames")
        .into_iter()
        .map(|record| {
            (
                record.span_id,
                record.log_digest,
                record.messages_json.unwrap_or_default(),
            )
        })
        .collect()
}

/// A framed request's frame records read the same on both backends: only its trace's records stating its key,
/// each identity once at its winning revision, in record order, and nothing for another key, another trace, the
/// zero key, or a watermark before them; each of two projects sharing a trace id and key reads only its own; and the
/// request's key itself reads back on its message row.
#[tokio::test]
async fn a_request_s_frames_read_the_same_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_request_frames").await;
    // Two projects under the same trace ids and keys: a client's trace ids are not unique across projects.
    for project in [PROJECT, OTHER_FRAMES_PROJECT] {
        seed_frames(&duck, project).await;
        seed_frames(&ch, project).await;
    }

    let duck_frames = frames(&duck, PROJECT, "frame-trace", 1, None).await;
    let ch_frames = frames(&ch, PROJECT, "frame-trace", 1, None).await;
    assert_eq!(duck_frames, ch_frames, "the two backends differ");
    for project in [PROJECT, OTHER_FRAMES_PROJECT] {
        let read = frames(&duck, project, "frame-trace", 1, None).await;
        assert_eq!(read, frames(&ch, project, "frame-trace", 1, None).await);
        assert!(
            read.len() == 2
                && read
                    .iter()
                    .all(|(_, _, messages)| messages.contains(&format!("({project})"))),
            "each project reads only its own frames under a shared trace id and key: {read:?}"
        );
    }
    assert_eq!(
        duck_frames
            .iter()
            .map(|(span, digest, messages)| (
                span.as_str(),
                digest.as_str(),
                messages.contains("revised")
            ))
            .collect::<Vec<_>>(),
        [("span-f-a", "f-a", true), ("span-f-b", "f-b", false)],
        "this trace's records stating this key, in record order, at their winning revision"
    );
    for (trace, key, watermark, why) in [
        ("frame-trace", 3, None, "another key"),
        ("frame-trace", 0, None, "the zero key"),
        ("missing-trace", 1, None, "another trace"),
        (
            "frame-trace",
            1,
            Some(ts(1).timestamp_micros()),
            "a watermark before them",
        ),
    ] {
        let duck_none = frames(&duck, PROJECT, trace, key, watermark).await;
        assert_eq!(
            duck_none,
            frames(&ch, PROJECT, trace, key, watermark).await,
            "{why}"
        );
        assert!(duck_none.is_empty(), "{why}: {duck_none:?}");
    }

    // The record past the byte bound comes back as its identity alone, on both backends.
    let bounded = |frames: Vec<(String, String, String)>| {
        frames
            .into_iter()
            .map(|(_, digest, messages)| (digest, !messages.is_empty()))
            .collect::<Vec<_>>()
    };
    let duck_big = bounded(frames(&duck, PROJECT, "big-trace", HIGH_KEY, None).await);
    assert_eq!(
        duck_big,
        bounded(frames(&ch, PROJECT, "big-trace", HIGH_KEY, None).await)
    );
    assert_eq!(
        duck_big,
        [
            ("f-big-1".to_string(), true),
            ("f-big-2".to_string(), false)
        ]
    );

    let message_row = |rows: Vec<MessageSpanRow>| {
        rows.into_iter()
            .map(|row| (row.span_id, row.request_frame))
            .collect::<Vec<_>>()
    };
    let params = MessageQueryParams {
        project_id: ProjectId::from(PROJECT),
        span_id: Some("framed-request".to_string()),
        trace_id: Some(format!("{PROJECT}-framed-request-trace")),
        ..Default::default()
    };
    let duck_row = message_row(duck.get_messages(&params).await.expect("duck row").rows);
    assert_eq!(
        duck_row,
        message_row(ch.get_messages(&params).await.expect("ch row").rows)
    );
    assert_eq!(duck_row, [("framed-request".to_string(), "k1".to_string())]);
}

/// **A trace with more frames than a read should hold is answered alike, and in little memory.** Twelve thousand
/// frame records of 16 KB under one key - about 200 MB of messages: both backends return the bound's worth, the
/// same records, and ClickHouse's own log says the statement held a fraction of them. With the record bound and the
/// running total one window over every keyed row, the statement held them all, three times over.
#[tokio::test]
async fn a_trace_with_more_frames_than_a_read_holds_is_answered_alike() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_frames_memory";
    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, database).await;
    let text = "x".repeat(16 * 1024);
    let records = 12_000usize;
    for chunk in (0..records).collect::<Vec<_>>().chunks(1_000) {
        let logs: Vec<NormalizedLog> = chunk
            .iter()
            .map(|n| {
                frame_record(
                    PROJECT,
                    &format!("many-{n:05}"),
                    "many-trace",
                    Some(21),
                    *n as i64,
                    &text,
                )
            })
            .collect();
        duck.insert_logs(&logs).await.expect("duck frames");
        ch.insert_logs(&logs).await.expect("ch frames");
    }
    let client = raw_client(&url, database);
    let since = server_now(&client).await;
    let duck_frames = frames(&duck, PROJECT, "many-trace", 21, None).await;
    assert_eq!(
        duck_frames,
        frames(&ch, PROJECT, "many-trace", 21, None).await
    );
    assert_eq!(
        duck_frames.len(),
        sideseat_core::constants::REQUEST_FRAMES_MAX_RECORDS + 1
    );
    assert!(
        duck_frames
            .iter()
            .all(|(_, _, messages)| !messages.is_empty())
    );
    assert_eq!(
        duck_frames[0].1, "many-00000",
        "the first records, in order"
    );

    let used = frames_statement_memory(&client, database, since).await;
    let total = (records * text.len()) as u64;
    // Measured about 75 MB for the 197 MB here, over the twelve parts the inserts leave unmerged: a block of each
    // part's records at a time. With the running total a window over every keyed row it held 610 MB.
    assert!(
        used < 128 * 1024 * 1024,
        "the frames statement held {used} bytes for {total} bytes of frames, so it carried messages it left out"
    );
}

/// The server's clock, for telling the statement a test runs from earlier runs' in the query log.
async fn server_now(client: &clickhouse::Client) -> i64 {
    client
        .query("SELECT toInt64(toUnixTimestamp64Micro(now64(6)))")
        .fetch_one()
        .await
        .expect("the server's clock")
}

/// The memory ClickHouse's own log says the frames statement in `database` that finished at or after `since` (the
/// server's clock, in microseconds) held. The log is written after the answer is sent, so it is asked until the
/// entry is there.
async fn frames_statement_memory(client: &clickhouse::Client, database: &str, since: i64) -> u64 {
    for _ in 0..50 {
        client
            .query("SYSTEM FLUSH LOGS")
            .execute()
            .await
            .expect("flush the query log");
        let used: Vec<u64> = client
            .query(
                "SELECT memory_usage FROM system.query_log \
                 WHERE type = 'QueryFinish' AND current_database = ? AND query LIKE '%WITH chosen AS%' \
                   AND query NOT LIKE '%query_log%' \
                   AND toInt64(toUnixTimestamp64Micro(event_time_microseconds)) >= ? \
                 ORDER BY event_time_microseconds DESC LIMIT 1",
            )
            .bind(database)
            .bind(since)
            .fetch_all()
            .await
            .expect("read the query log");
        if let Some(used) = used.first() {
            return *used;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    panic!("the frames statement is not in the query log");
}

/// **However many records a key finds, the ClickHouse frame read holds the bound's worth.** A million frame
/// records of a few bytes under one key in one trace: the read returns the first ones, and the statement's memory
/// stays small - the first records are kept by a top-N, which holds those alone. Grouped by identity in a hash
/// table, a form of this read held 684 MB here, and 2 GB at three million records.
#[tokio::test]
async fn the_frame_read_holds_the_bound_however_many_records_a_key_finds_on_clickhouse() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_frames_cardinality";
    let ch = clickhouse_backend(&url, database).await;
    let client = raw_client(&url, database);
    client
        .query(&format!(
            "INSERT INTO otel_logs (project_id, log_digest, ordinal, timestamp, severity_number, \
             dropped_attributes_count, flags, trace_id, span_id, ingested_at, messages, frame_key) \
             SELECT '{PROJECT}', leftPad(toString(number), 8, '0'), 0, \
             now64(6) - INTERVAL 1 DAY + toIntervalMicrosecond(number), 9, 0, 0, 'tiny-trace', 'span', now64(6), \
             '[{{\"text\":\"f\"}}]', toUInt128(77) FROM numbers(1000000)"
        ))
        .execute()
        .await
        .expect("the frame records");
    let since = server_now(&client).await;
    let read = frames(&ch, PROJECT, "tiny-trace", 77, None).await;
    assert_eq!(
        read.len(),
        sideseat_core::constants::REQUEST_FRAMES_MAX_RECORDS + 1
    );
    assert_eq!(read[0].1, "00000000", "the first records, in order");
    let used = frames_statement_memory(&client, database, since).await;
    // Measured about 14 MB.
    assert!(
        used < 48 * 1024 * 1024,
        "the frames statement held {used} bytes for a million records under one key"
    );
}

/// **A request's frames read alike on a two-shard cluster.** Both passes read the `Distributed` table, and the
/// second narrows by the first's identities, so a plain `IN` would be refused there (`distributed_product_mode`);
/// each project lives on a different shard, so a read that saw only the shard the client reached fails here too.
#[tokio::test]
async fn a_request_s_frames_read_alike_across_a_two_shard_cluster() {
    let Ok(url) = std::env::var(TWO_SHARD_URL_ENV) else {
        eprintln!(
            "clickhouse two-shard: skipped - set {TWO_SHARD_URL_ENV} (or run \
             `make test-clickhouse-two-shard`)"
        );
        return;
    };
    let database = "sideseat_two_shard_request_frames";
    let service = replicated_backend_at(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let client = raw_client_at(&url, database, &user, &password);
    let mut per_shard: [Option<String>; 2] = [None, None];
    for n in 0..64 {
        let candidate = format!("frames-shard-{n}");
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
        seed_frames(&duck, project).await;
        seed_frames(&service, project).await;
    }
    for project in [&near, &far] {
        for (trace, key) in [
            ("frame-trace", 1),
            ("big-trace", HIGH_KEY),
            ("frame-trace", 3),
        ] {
            let duck_frames = frames(&duck, project, trace, key, None).await;
            assert_eq!(
                duck_frames,
                frames(&service, project, trace, key, None).await,
                "{project} {trace} {key}: the two-shard answer differs from DuckDB's"
            );
            assert!(
                duck_frames
                    .iter()
                    .all(|(_, _, messages)| messages.is_empty()
                        || messages.contains(&format!("({project})"))),
                "{project}: only its own frames"
            );
        }
        assert_eq!(
            frames(&duck, project, "frame-trace", 1, None).await.len(),
            2
        );
    }
}

/// **A frame redelivered in another month reads once, as its latest delivery, alike.** A record with no time of
/// its own is received at a different time each delivery, and ClickHouse's `FINAL` merges an identity's deliveries
/// only within a partition, so the deliveries must share one (`OTEL_LOGS_PARTITION`); DuckDB keeps one row per
/// identity. Both return the record once, with the later delivery's messages, and a later delivery that re-keys or
/// clears the record takes it out of its earlier key.
#[tokio::test]
async fn a_frame_redelivered_in_another_month_reads_once_alike() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_frames_months").await;
    // The day before this month began, and the month's first instant: two months, and recent enough that the
    // store's retention keeps both.
    let month = chrono::Utc::now()
        .date_naive()
        .with_day(1)
        .expect("a first day")
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc();
    let delivery = |later: bool, text: &str| {
        let at = if later {
            month
        } else {
            month - chrono::TimeDelta::days(1)
        };
        NormalizedLog {
            timestamp: at,
            time: None,
            ingested_at: Some(at),
            ..frame_record(PROJECT, "f-timeless", "month-trace", Some(31), 0, text)
        }
    };
    for backend in [
        &duck as &dyn AnalyticsRepository,
        &ch as &dyn AnalyticsRepository,
    ] {
        backend
            .insert_logs(&[delivery(false, "first delivery")])
            .await
            .expect("the first delivery");
        backend
            .insert_logs(&[delivery(true, "second delivery")])
            .await
            .expect("the redelivery a month later");
    }
    let duck_frames = frames(&duck, PROJECT, "month-trace", 31, None).await;
    assert_eq!(
        duck_frames,
        frames(&ch, PROJECT, "month-trace", 31, None).await
    );
    assert_eq!(duck_frames.len(), 1, "one record, once: {duck_frames:?}");
    assert!(
        duck_frames[0].2.contains("second delivery"),
        "the latest delivery"
    );

    // A redelivery that no longer frames the key - its key cleared, or another - leaves the earlier delivery's
    // key answering nothing, on both: the latest delivery is resolved before its key is asked.
    let keyed = |digest: &str, key: Option<u128>, later: bool, text: &str| NormalizedLog {
        log_digest: digest.to_string(),
        frame_key: key,
        ..delivery(later, text)
    };
    for backend in [
        &duck as &dyn AnalyticsRepository,
        &ch as &dyn AnalyticsRepository,
    ] {
        backend
            .insert_logs(&[
                keyed("f-cleared", Some(41), false, "keyed"),
                keyed("f-rekeyed", Some(42), false, "keyed"),
            ])
            .await
            .expect("the first deliveries");
        backend
            .insert_logs(&[
                keyed("f-cleared", None, true, "no longer a frame"),
                keyed("f-rekeyed", Some(43), true, "another key"),
            ])
            .await
            .expect("the redeliveries a month later");
    }
    for (key, expected) in [(41, 0), (42, 0), (43, 1)] {
        let duck_frames = frames(&duck, PROJECT, "month-trace", key, None).await;
        assert_eq!(
            duck_frames,
            frames(&ch, PROJECT, "month-trace", key, None).await,
            "key {key}"
        );
        assert_eq!(duck_frames.len(), expected, "key {key}: {duck_frames:?}");
    }
}
