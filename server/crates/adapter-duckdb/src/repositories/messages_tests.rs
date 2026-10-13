use super::*;
use crate::DuckdbService;
use crate::repositories::span::insert_batch;
use chrono::{Duration, Utc};
use sideseat_core::storage::AppStorage;
use sideseat_ports::types::{NormalizedSpan, ProjectId};
use tempfile::TempDir;

async fn create_test_service() -> (TempDir, DuckdbService) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let duckdb_dir = temp_dir.path().join("duckdb");
    tokio::fs::create_dir_all(&duckdb_dir)
        .await
        .expect("Failed to create duckdb dir");
    let storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
    let service = DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
        .await
        .expect("Failed to init analytics service");
    (temp_dir, service)
}

fn make_span_with_messages(
    project_id: &str,
    trace_id: &str,
    span_id: &str,
    messages_json: &str,
) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        span_name: "test".to_string(),
        timestamp_start: Utc::now(),
        messages: Some(messages_json.to_string()),
        ..Default::default()
    }
}

/// The scope and the envelope facts survive the round trip: written by the positional appender,
/// read back by the message projection. This is the write-and-read pair the schema-v2 columns
/// exist for, and it is the test that fails if a projection and the appender ever disagree about
/// a column's position - the failure mode of a positional writer.
#[tokio::test]
async fn scope_and_envelope_facts_survive_the_round_trip() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let mut span = make_span_with_messages(
        project_id,
        "trace-env",
        "span-env",
        r#"[{"role": "user", "content": "Hello"}]"#,
    );
    span.scope_name = Some("opentelemetry.instrumentation.langchain".to_string());
    span.scope_version = Some("0.3.1".to_string());
    span.gen_ai_response_model = Some("model-x-20260101".to_string());
    span.gen_ai_response_id = Some("resp_abc".to_string());
    span.gen_ai_temperature = Some(0.7);
    span.gen_ai_max_tokens = Some(1024);
    span.gen_ai_usage_cache_read_tokens = 17;
    span.gen_ai_cost_input = 0.001;
    span.gen_ai_cost_output = 0.002;

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[span]).expect("insert");
    }

    let conn = analytics.conn();
    let params = FeedMessagesParams {
        project_id: ProjectId::from(project_id),
        limit: 10,
        ..Default::default()
    };
    let result = get_project_messages(&conn, &params).expect("query");
    assert_eq!(result.rows.len(), 1);
    let row = &result.rows[0];
    assert_eq!(
        row.scope_name.as_deref(),
        Some("opentelemetry.instrumentation.langchain"),
        "the instrumentation scope must survive the round trip - it is what makes a rule keyed \
         on a producer's identity-and-version expressible at read time"
    );
    assert_eq!(row.scope_version.as_deref(), Some("0.3.1"));
    assert_eq!(row.response_model.as_deref(), Some("model-x-20260101"));
    assert_eq!(row.response_id.as_deref(), Some("resp_abc"));
    assert_eq!(row.temperature, Some(0.7));
    assert_eq!(row.max_tokens, Some(1024));
    assert_eq!(row.cache_read_tokens, 17);
    assert!((row.cost_input - 0.001).abs() < 1e-9);
    assert!((row.cost_output - 0.002).abs() < 1e-9);
}

#[tokio::test]
async fn test_get_project_messages_basic() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let messages_json = r#"[{"role": "user", "content": "Hello"}]"#;
    let span = make_span_with_messages(project_id, "trace-1", "span-1", messages_json);

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[span]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let params = FeedMessagesParams {
        project_id: ProjectId::from(project_id),
        limit: 10,
        ..Default::default()
    };
    let result = get_project_messages(&conn, &params).expect("Query should succeed");

    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].trace_id, "trace-1");
}

#[tokio::test]
async fn test_get_project_messages_filters_empty_spans() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    // Span with messages
    let span_with_messages =
        make_span_with_messages(project_id, "trace-1", "span-1", r#"[{"role": "user"}]"#);

    // Span without messages (empty array)
    let span_empty = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: "trace-2".to_string(),
        span_id: "span-2".to_string(),
        span_name: "empty".to_string(),
        timestamp_start: Utc::now(),
        messages: Some("[]".to_string()),
        ..Default::default()
    };

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[span_with_messages, span_empty]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let params = FeedMessagesParams {
        project_id: ProjectId::from(project_id),
        limit: 10,
        ..Default::default()
    };
    let result = get_project_messages(&conn, &params).expect("Query should succeed");

    // Should only return the span with messages
    assert_eq!(
        result.rows.len(),
        1,
        "Should filter out empty message spans"
    );
    assert_eq!(result.rows[0].span_id, "span-1");
}

#[tokio::test]
async fn test_get_project_messages_cursor_pagination() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    // Create spans with different ingested_at times
    let base_time = Utc::now();
    let spans: Vec<_> = (0..5)
        .map(|i| {
            let mut span = make_span_with_messages(
                project_id,
                &format!("trace-{}", i),
                &format!("span-{}", i),
                r#"[{"role": "user", "content": "test"}]"#,
            );
            span.timestamp_start = base_time + Duration::seconds(i as i64);
            span.ingested_at = Some(base_time + Duration::seconds(i as i64));
            span
        })
        .collect();

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();

    // First page
    let params = FeedMessagesParams {
        project_id: ProjectId::from(project_id),
        limit: 2,
        ..Default::default()
    };
    let page1 = get_project_messages(&conn, &params).expect("Query should succeed");
    assert_eq!(page1.rows.len(), 2, "First page should have 2 rows");

    // Second page with cursor
    let last_row = page1.rows.last().unwrap();
    let cursor_time_us = last_row.ingested_at.timestamp_micros();
    let params = FeedMessagesParams {
        project_id: ProjectId::from(project_id),
        limit: 2,
        cursor: Some((
            cursor_time_us,
            last_row.span_id.clone(),
            last_row.trace_id.clone(),
        )),
        ..Default::default()
    };
    let page2 = get_project_messages(&conn, &params).expect("Query should succeed");
    assert_eq!(page2.rows.len(), 2, "Second page should have 2 rows");

    // Verify no overlap
    let page1_ids: Vec<_> = page1.rows.iter().map(|r| &r.span_id).collect();
    for row in &page2.rows {
        assert!(
            !page1_ids.contains(&&row.span_id),
            "Pages should not overlap"
        );
    }
}

#[tokio::test]
async fn test_get_project_messages_time_filter() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let base_time = Utc::now();
    let mut old_span =
        make_span_with_messages(project_id, "trace-old", "span-old", r#"[{"role": "user"}]"#);
    old_span.timestamp_start = base_time - Duration::hours(2);

    let mut new_span =
        make_span_with_messages(project_id, "trace-new", "span-new", r#"[{"role": "user"}]"#);
    new_span.timestamp_start = base_time;

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[old_span, new_span]).expect("Insert should succeed");
    }

    let conn = analytics.conn();

    // Filter to only recent spans
    let params = FeedMessagesParams {
        project_id: ProjectId::from(project_id),
        limit: 10,
        start_time: Some(base_time - Duration::hours(1)),
        ..Default::default()
    };
    let result = get_project_messages(&conn, &params).expect("Query should succeed");

    assert_eq!(result.rows.len(), 1, "Should filter by start_time");
    assert_eq!(result.rows[0].span_id, "span-new");
}

#[tokio::test]
async fn test_get_project_messages_with_session_id() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let mut span = make_span_with_messages(
        project_id,
        "trace-1",
        "span-1",
        r#"[{"role": "user", "content": "Hello"}]"#,
    );
    span.session_id = Some("session-123".to_string());

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[span]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let params = FeedMessagesParams {
        project_id: ProjectId::from(project_id),
        limit: 10,
        ..Default::default()
    };
    let result = get_project_messages(&conn, &params).expect("Query should succeed");

    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].session_id, Some("session-123".to_string()));
}

#[tokio::test]
async fn test_get_project_messages_empty_project() {
    let (_temp_dir, analytics) = create_test_service().await;

    let conn = analytics.conn();
    let params = FeedMessagesParams {
        project_id: ProjectId::from("nonexistent"),
        limit: 10,
        ..Default::default()
    };
    let result = get_project_messages(&conn, &params).expect("Query should succeed");

    assert!(result.rows.is_empty());
}

fn log_message(text: &str, second: i64) -> String {
    serde_json::json!([{
        "source": {"event": {"name": "gen_ai.user.message",
                             "time": format!("2026-01-01T00:00:{second:02}Z")}},
        "content": {"content": text}
    }])
    .to_string()
}

fn message_log(
    project_id: &str,
    digest: &str,
    span: (&str, &str),
    second: i64,
    ingested_at: chrono::DateTime<Utc>,
    messages: &str,
) -> sideseat_ports::types::NormalizedLog {
    let timestamp = chrono::DateTime::from_timestamp(1_767_225_600 + second, 0).unwrap();
    sideseat_ports::types::NormalizedLog {
        project_id: Some(project_id.to_string()),
        log_digest: digest.to_string(),
        timestamp,
        time: Some(timestamp),
        trace_id: Some(span.0.to_string()),
        span_id: Some(span.1.to_string()),
        ingested_at: Some(ingested_at),
        messages: Some(messages.to_string()),
        ..Default::default()
    }
}

/// The span's log messages as the store answered them, for records stored with their messages - as these
/// tests write them: the arrays in order, joined, as the domain concatenates them (`domain::log_messages`).
fn stored_log_messages(row: &MessageSpanRow) -> String {
    let parts: Vec<String> = row
        .log_message_sources
        .iter()
        .map(|source| {
            let messages = source.messages.as_deref().expect("stored messages");
            messages[1..messages.len() - 1].to_string()
        })
        .collect();
    if parts.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", parts.join(","))
    }
}

/// A span whose conversation arrived only as log records is returned, with those messages joined in
/// log-time order, by every message read - and not before its own watermark.
#[tokio::test]
async fn log_messages_join_their_span_in_order_and_respect_the_watermark() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "log-project";
    let base = Utc::now() - Duration::minutes(10);
    let mut span = make_span_with_messages(project_id, "trace-l", "span-l", "[]");
    span.ingested_at = Some(base);
    let other = make_span_with_messages(project_id, "trace-l", "span-other", "[]");
    let conn = analytics.conn();
    // The logs are written first: the join does not care which arrived first.
    crate::repositories::log::insert_batch(
        &conn,
        &[
            message_log(
                project_id,
                "b",
                ("trace-l", "span-l"),
                2,
                base,
                &log_message("second", 2),
            ),
            message_log(
                project_id,
                "a",
                ("trace-l", "span-l"),
                1,
                base,
                &log_message("first", 1),
            ),
            message_log(
                project_id,
                "late",
                ("trace-l", "span-l"),
                3,
                base + Duration::minutes(5),
                &log_message("late", 3),
            ),
            message_log(
                project_id,
                "orphan",
                ("trace-l", "span-missing"),
                1,
                base,
                &log_message("orphan", 1),
            ),
            message_log(
                project_id,
                "empty",
                ("trace-l", "span-other"),
                1,
                base,
                "[]",
            ),
        ],
    )
    .expect("logs");
    insert_batch(&conn, &[span, other]).expect("spans");

    let trace = get_messages(
        &conn,
        &MessageQueryParams {
            project_id: ProjectId::from(project_id),
            trace_id: Some("trace-l".to_string()),
            ..Default::default()
        },
    )
    .expect("trace read");
    assert_eq!(
        trace
            .rows
            .iter()
            .map(|row| row.span_id.as_str())
            .collect::<Vec<_>>(),
        vec!["span-l"],
        "the span with log messages passes the content filter; the one with none does not"
    );
    let joined: Vec<serde_json::Value> =
        serde_json::from_str(&stored_log_messages(&trace.rows[0])).expect("a JSON array");
    assert_eq!(
        joined
            .iter()
            .map(|message| message["content"]["content"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["first", "second", "late"],
        "concatenated in log-time order"
    );

    let bounded = get_messages(
        &conn,
        &MessageQueryParams {
            project_id: ProjectId::from(project_id),
            trace_ids: Some(vec!["trace-l".to_string()]),
            ingested_before_us: Some((base + Duration::minutes(1)).timestamp_micros()),
            ..Default::default()
        },
    )
    .expect("bounded read");
    let joined: Vec<serde_json::Value> =
        serde_json::from_str(&stored_log_messages(&bounded.rows[0])).unwrap();
    assert_eq!(
        joined.len(),
        2,
        "a log after the watermark is not part of the instant"
    );

    let alone = get_messages(
        &conn,
        &MessageQueryParams {
            project_id: ProjectId::from(project_id),
            span_id: Some("span-other".to_string()),
            ..Default::default()
        },
    )
    .expect("span read");
    assert_eq!(
        alone.rows.len(),
        1,
        "the span view applies no content filter"
    );
    assert!(alone.rows[0].log_message_sources.is_empty());
}

/// **A parser reads its columns by name, so where the query puts them is the query's business.** The
/// positional reader failed exactly here: one column added to a projection moved the joined log messages
/// along, and they were read from the column beside them. Reversed columns read the same row, and a column
/// the query does not return is an error that names it rather than a value read from somewhere else.
#[test]
fn a_parser_reads_by_name_whatever_order_the_query_returns() {
    let conn = Connection::open_in_memory().expect("open");
    conn.execute_batch(
        "CREATE TABLE t AS SELECT 'trace-1' AS trace_id, 'span-1' AS span_id, \
         1700000000000000::BIGINT AS timestamp_start, 'OK' AS status_code, '[]' AS messages, \
         'generation' AS observation_type, 'chat' AS span_name, 'scope' AS scope_name, '1.0' AS scope_version, \
         'session-1' AS session_id, 5::USMALLINT AS span_marks",
    )
    .expect("create");
    let read = |columns: &str| {
        read_rows(
            &conn,
            &format!("SELECT {columns} FROM t"),
            &[],
            ThreadColumns::resolve,
            parse_thread_row,
        )
    };
    let declared = "trace_id, span_id, timestamp_start, status_code, messages, observation_type, \
                    span_name, scope_name, scope_version, session_id, span_marks";
    let mut columns: Vec<&str> = declared.split(", ").collect();
    columns.reverse();
    let reversed = columns.join(", ");
    let in_order = read(declared).expect("the declared order");
    let out_of_order = read(&reversed).expect("the reversed order");
    assert_eq!(in_order.len(), 1);
    let row = &out_of_order[0];
    assert_eq!(
        (
            row.trace_id.as_str(),
            row.span_id.as_str(),
            row.session_id.as_deref(),
            row.span_marks
        ),
        ("trace-1", "span-1", Some("session-1"), 5),
        "every column is read from the one that holds it"
    );
    assert_eq!(
        format!("{in_order:?}"),
        format!("{out_of_order:?}"),
        "and the order changes nothing"
    );
    let missing = read(&declared.replace(", span_marks", ""))
        .expect_err("a column the query does not return");
    assert!(
        missing.to_string().contains("span_marks"),
        "the error names the column: {missing}"
    );
}

/// **A composed row carries the marks its own view reads.** A request's view is composed from its thread's
/// rows, and those rows go through the same projections as each row's own view - so a projection that asks
/// about a mark must see, on the composed row, the word the span was stored with. The thread and call reads
/// once left it out, and every predecessor arrived unmarked: a request withdrawn from its own view would still
/// have been composed into the next one's.
#[tokio::test]
async fn a_composed_row_carries_the_marks_its_own_view_reads() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project = "test-project";
    let thread = r#"["probe.thread","session-1"]"#;
    let start = Utc::now() - Duration::seconds(60);
    let messages = r#"[{"source":{"attribute":{"key":"probe","time":"2025-01-01T00:00:00Z"}},"content":{"role":"user","content":"q"}}]"#;
    let request = |span: &str, offset: i64, marks: u16| NormalizedSpan {
        timestamp_start: start + Duration::seconds(offset),
        request_thread: thread.to_string(),
        span_marks: marks,
        ..make_span_with_messages(project, &format!("trace-{span}"), span, messages)
    };
    let call = NormalizedSpan {
        timestamp_start: start + Duration::seconds(1),
        gen_ai_tool_call_id: Some("call-1".to_string()),
        span_marks: 0b100,
        ..make_span_with_messages(project, "trace-call", "call", messages)
    };
    {
        let conn = analytics.conn();
        insert_batch(
            &conn,
            &[request("marked", 0, 0b10), request("plain", 2, 0), call],
        )
        .expect("insert");
    }

    let conn = analytics.conn();
    let context = get_request_context(
        &conn,
        &RequestContextParams {
            project_id: ProjectId::from(project),
            thread: thread.to_string(),
            before_us: (start + Duration::seconds(10)).timestamp_micros(),
            call_ids: vec!["call-1".to_string()],
            call_trace_ids: vec!["trace-call".to_string()],
            ingested_before_us: None,
        },
    )
    .expect("the request context");
    let composed: std::collections::BTreeMap<String, u16> = context
        .thread
        .iter()
        .chain(&context.calls)
        .map(|row| (row.span_id.clone(), row.span_marks))
        .collect();
    assert_eq!(
        composed,
        [("call", 0b100), ("marked", 0b10), ("plain", 0)]
            .into_iter()
            .map(|(span, marks)| (span.to_string(), marks))
            .collect(),
        "the thread and call reads carry each span's stored marks"
    );
    for (span, marks) in &composed {
        let own = get_messages(
            &conn,
            &MessageQueryParams {
                project_id: ProjectId::from(project),
                span_id: Some(span.clone()),
                trace_id: Some(format!("trace-{span}")),
                ..Default::default()
            },
        )
        .expect("the span's own view");
        assert_eq!(
            own.rows
                .iter()
                .map(|row| row.span_marks)
                .collect::<Vec<_>>(),
            vec![*marks],
            "`{span}`: its own view and the composition read the same marks"
        );
    }
}
