//! Message query operations.
//!
//! Provides queries for extracting conversation messages from spans.
//! Messages are stored as raw JSON (SideML conversion happens at query time).
//!
//! This repository only handles data retrieval. All message processing
//! (filtering, deduplication, sorting, metadata harvesting) is done by
//! the feed pipeline (process_spans) in the domain layer.

use duckdb::Connection;

use crate::error::DuckdbError;
use sideseat_core::utils::time::micros_to_datetime;
use sideseat_ports::types::{
    FeedMessagesParams, MessageQueryParams, MessageQueryResult, MessageSpanRow,
    RequestContextParams, RequestContextRows,
};
use sideseat_query_sql::{Backend, analytics, messages, request_context};

fn duckdb_values(values: &[analytics::QueryValue]) -> Vec<&dyn duckdb::ToSql> {
    values
        .iter()
        .map(|value| match value {
            analytics::QueryValue::String(value) => value as &dyn duckdb::ToSql,
            analytics::QueryValue::Int64(value) => value as &dyn duckdb::ToSql,
            analytics::QueryValue::Float64(value) => value as &dyn duckdb::ToSql,
        })
        .collect()
}

fn execute_message_query(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<MessageQueryResult, DuckdbError> {
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let rows: Vec<Result<MessageSpanRow, _>> =
        stmt.query_map(values.as_slice(), parse_span_row)?.collect();
    Ok(MessageQueryResult {
        rows: rows.into_iter().collect::<Result<_, _>>()?,
    })
}

// ============================================================================
// Query functions - return raw unfiltered data
// ============================================================================

/// Get span rows for a span, trace, or session (unified query).
///
/// Priority: span_id > session_id > trace_id > trace_ids
pub fn get_messages(
    conn: &Connection,
    params: &MessageQueryParams,
) -> Result<MessageQueryResult, DuckdbError> {
    let query = messages::get_messages(params, Backend::Duckdb);
    execute_message_query(conn, &query)
}

/// Get span rows for entire project (feed API).
///
/// Uses cursor-based pagination on (ingested_at, span_id, trace_id).
pub fn get_project_messages(
    conn: &Connection,
    params: &FeedMessagesParams,
) -> Result<MessageQueryResult, DuckdbError> {
    let query = messages::get_project_messages(params, Backend::Duckdb);
    execute_message_query(conn, &query)
}

/// The rows a request span's view is composed from: its thread's earlier requests, and the tool spans holding the
/// calls those requests' deltas answer. Two keyed reads, in one call so both see one connection's instant.
pub fn get_request_context(
    conn: &Connection,
    params: &RequestContextParams,
) -> Result<RequestContextRows, DuckdbError> {
    let ids: Vec<&str> = params.call_ids.iter().map(String::as_str).collect();
    let traces: Vec<&str> = params.call_trace_ids.iter().map(String::as_str).collect();
    Ok(RequestContextRows {
        thread: thread_rows(
            conn,
            &request_context::thread_requests(
                &params.thread,
                params.before_us,
                params.ingested_before_us,
                Backend::Duckdb,
            ),
        )?,
        calls: match ids.is_empty() || traces.is_empty() {
            true => Vec::new(),
            false => thread_rows(
                conn,
                &request_context::calls_in_traces(
                    &traces,
                    &ids,
                    params.ingested_before_us,
                    Backend::Duckdb,
                ),
            )?,
        },
    })
}

fn thread_rows(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<MessageSpanRow>, DuckdbError> {
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let rows: Vec<Result<MessageSpanRow, _>> = stmt
        .query_map(values.as_slice(), parse_thread_row)?
        .collect();
    rows.into_iter()
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

/// One row of a thread read, in `request_context::THREAD_COLUMNS` order.
///
/// The unprojected fields are left at what a span that carries none would have: a composition reads the messages
/// and the facts that place them, and a projection is what keeps the rest of a span's bytes unread.
fn parse_thread_row(row: &duckdb::Row) -> Result<MessageSpanRow, duckdb::Error> {
    let span_timestamp = row
        .get::<_, Option<i64>>(2)
        .map(|micros| micros.map(micros_to_datetime))?
        .unwrap_or(chrono::DateTime::UNIX_EPOCH);
    Ok(MessageSpanRow {
        trace_id: row.get(0)?,
        span_id: row.get(1)?,
        span_timestamp,
        ingested_at: span_timestamp,
        status_code: row.get(3)?,
        messages_json: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        observation_type: row.get(5)?,
        span_name: row.get(6)?,
        scope_name: row.get(7)?,
        scope_version: row.get(8)?,
        session_id: row.get(9)?,
        span_marks: row.get(10)?,
        parent_span_id: None,
        span_end_timestamp: None,
        tool_definitions_json: "[]".to_string(),
        tool_names_json: "[]".to_string(),
        log_messages_json: "[]".to_string(),
        body_cache_key: None,
        model: None,
        provider: None,
        exception_type: None,
        exception_message: None,
        exception_stacktrace: None,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cost_total: 0.0,
        framework: None,
        response_model: None,
        response_id: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        finish_reasons: None,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_input: 0.0,
        cost_output: 0.0,
        request_thread: String::new(),
    })
}

// ============================================================================
// Helper functions
// ============================================================================

/// Parse a span row from database - just extracts fields, no transformation.
fn parse_span_row(row: &duckdb::Row) -> Result<MessageSpanRow, duckdb::Error> {
    Ok(MessageSpanRow {
        trace_id: row.get(0)?,
        span_id: row.get(1)?,
        parent_span_id: row.get(2)?,
        span_timestamp: micros_to_datetime(row.get::<_, i64>(3)?),
        span_end_timestamp: row.get::<_, Option<i64>>(4)?.map(micros_to_datetime),
        messages_json: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
        model: row.get(6)?,
        provider: row.get(7)?,
        status_code: row.get(8)?,
        exception_type: row.get(9)?,
        exception_message: row.get(10)?,
        exception_stacktrace: row.get(11)?,
        input_tokens: row.get(12)?,
        output_tokens: row.get(13)?,
        total_tokens: row.get(14)?,
        cost_total: row.get(15)?,
        tool_definitions_json: row.get::<_, Option<String>>(16)?.unwrap_or_default(),
        tool_names_json: row.get::<_, Option<String>>(17)?.unwrap_or_default(),
        log_messages_json: row
            .get::<_, Option<String>>(38)?
            .unwrap_or_else(|| "[]".to_string()),
        body_cache_key: None,
        observation_type: row.get(18)?,
        session_id: row.get(19)?,
        ingested_at: micros_to_datetime(row.get::<_, i64>(20)?),
        scope_name: row.get(21)?,
        scope_version: row.get(22)?,
        span_name: row.get(23)?,
        framework: row.get(24)?,
        response_model: row.get(25)?,
        response_id: row.get(26)?,
        temperature: row.get(27)?,
        top_p: row.get(28)?,
        max_tokens: row.get(29)?,
        finish_reasons: row.get(30)?,
        cache_read_tokens: row.get(31)?,
        cache_write_tokens: row.get(32)?,
        reasoning_tokens: row.get(33)?,
        cost_input: row.get(34)?,
        cost_output: row.get(35)?,
        request_thread: row.get::<_, Option<String>>(36)?.unwrap_or_default(),
        // Cast to `INTEGER` in the projection, because this driver's row reader has no conversion for the raw
        // `USMALLINT`; the stored column keeps its two bytes.
        span_marks: row.get::<_, i32>(37)? as u16,
    })
}

#[cfg(test)]
mod tests {
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
            serde_json::from_str(&trace.rows[0].log_messages_json).expect("a JSON array");
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
            serde_json::from_str(&bounded.rows[0].log_messages_json).unwrap();
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
        assert_eq!(alone.rows[0].log_messages_json, "[]");
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
}
