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
            thread_request(project, "thread-a", "req-a1", 10, "first", "reply one"),
            thread_request(project, "thread-a", "req-a2", 20, "second", "reply two"),
            thread_request(project, "thread-a", "req-a3", 30, "third", "reply three"),
            thread_request(project, "thread-b", "req-b1", 15, "other", "other reply"),
            call_span(project, "tool-a", 12, "call-a"),
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

/// What a composed request's reads returned, reduced to what this suite is about.
fn composed(rows: &RequestContextRows) -> (Vec<(String, String)>, Vec<String>) {
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
            .map(|row| (row.span_id.clone(), text(row)))
            .collect(),
        rows.calls.iter().map(|row| row.span_id.clone()).collect(),
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
                ("req-a1".to_string(), "first|reply one".to_string()),
                (
                    "req-a2".to_string(),
                    "second|reply two, revised".to_string()
                ),
                ("req-a3".to_string(), "third|reply three".to_string()),
            ],
            vec!["tool-a".to_string()],
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
        ["req-a1", "req-a2"],
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
