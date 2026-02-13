//! DuckDB span repository using Appender API
//!
//! Provides high-throughput batch writes for normalized spans.

use chrono::Utc;
use duckdb::Connection;
use duckdb::params;

use crate::data::duckdb::sql_types::{SqlOptTimestamp, SqlTimestamp, SqlVec};
use crate::data::duckdb::{DuckdbError, NormalizedSpan, in_transaction};
use crate::utils::json::json_to_opt_string;

pub fn insert_batch(conn: &Connection, spans: &[NormalizedSpan]) -> Result<(), DuckdbError> {
    if spans.is_empty() {
        return Ok(());
    }

    in_transaction(conn, |conn| {
        insert_spans(conn, spans)?;
        Ok(())
    })
}

fn insert_spans(conn: &Connection, spans: &[NormalizedSpan]) -> Result<(), DuckdbError> {
    if spans.is_empty() {
        return Ok(());
    }

    let mut appender = conn.appender("otel_spans")?;

    for span in spans {
        let tags = SqlVec(&span.tags);
        let stop_sequences = SqlVec(&span.gen_ai_stop_sequences);
        let finish_reasons = SqlVec(&span.gen_ai_finish_reasons);
        let metadata = json_to_opt_string(&span.metadata);

        let timestamp_start = SqlTimestamp(span.timestamp_start);
        let timestamp_end = SqlOptTimestamp(span.timestamp_end);

        // Column order must match schema.rs CREATE TABLE definition
        appender.append_row(params![
            // IDENTITY
            span.project_id.as_deref(),
            span.trace_id.as_str(),
            span.span_id.as_str(),
            span.parent_span_id.as_deref(),
            span.trace_state.as_deref(),
            // CONTEXT (Session, User, Environment)
            span.session_id.as_deref(),
            span.user_id.as_deref(),
            span.environment.as_deref(),
            // SPAN METADATA
            span.span_name.as_str(),
            span.span_kind.as_deref(),
            span.status_code.as_deref(),
            span.status_message.as_deref(),
            span.exception_type.as_deref(),
            span.exception_message.as_deref(),
            span.exception_stacktrace.as_deref(),
            // CLASSIFICATION
            span.span_category.map(|c| c.as_str()),
            span.observation_type.map(|o| o.as_str()),
            span.framework.map(|f| f.as_str()),
            // TIMING
            timestamp_start,
            timestamp_end,
            span.duration_ms,
            SqlTimestamp(span.ingested_at.unwrap_or_else(Utc::now)),
            // GEN AI
            span.gen_ai_system.as_deref(),
            span.gen_ai_operation_name.as_deref(),
            span.gen_ai_request_model.as_deref(),
            span.gen_ai_response_model.as_deref(),
            span.gen_ai_response_id.as_deref(),
            span.gen_ai_temperature,
            span.gen_ai_top_p,
            span.gen_ai_top_k,
            span.gen_ai_max_tokens,
            span.gen_ai_frequency_penalty,
            span.gen_ai_presence_penalty,
            stop_sequences,
            finish_reasons,
            span.gen_ai_agent_id.as_deref(),
            span.gen_ai_agent_name.as_deref(),
            span.gen_ai_tool_name.as_deref(),
            span.gen_ai_tool_call_id.as_deref(),
            span.gen_ai_server_ttft_ms,
            span.gen_ai_server_request_duration_ms,
            span.gen_ai_usage_input_tokens,
            span.gen_ai_usage_output_tokens,
            span.gen_ai_usage_total_tokens,
            span.gen_ai_usage_cache_read_tokens,
            span.gen_ai_usage_cache_write_tokens,
            span.gen_ai_usage_reasoning_tokens,
            json_to_opt_string(&span.gen_ai_usage_details).as_deref(),
            span.gen_ai_cost_input,
            span.gen_ai_cost_output,
            span.gen_ai_cost_cache_read,
            span.gen_ai_cost_cache_write,
            span.gen_ai_cost_reasoning,
            span.gen_ai_cost_total,
            span.http_method.as_deref(),
            span.http_url.as_deref(),
            span.http_status_code.map(|c| c as i32),
            span.db_system.as_deref(),
            span.db_name.as_deref(),
            span.db_operation.as_deref(),
            span.db_statement.as_deref(),
            span.storage_system.as_deref(),
            span.storage_bucket.as_deref(),
            span.storage_object.as_deref(),
            span.messaging_system.as_deref(),
            span.messaging_destination.as_deref(),
            tags,
            metadata.as_deref(),
            span.input_preview.as_deref(),
            span.output_preview.as_deref(),
            // Raw messages (JSON array)
            json_to_opt_string(&span.messages)
                .as_deref()
                .unwrap_or("[]"),
            // Raw tool definitions (JSON array)
            json_to_opt_string(&span.tool_definitions)
                .as_deref()
                .unwrap_or("[]"),
            // Raw tool names (JSON array)
            json_to_opt_string(&span.tool_names)
                .as_deref()
                .unwrap_or("[]"),
            // Raw span JSON (includes attributes, resource.attributes, events, links)
            json_to_opt_string(&span.raw_span).as_deref(),
        ])?;
    }

    appender.flush()?;
    drop(appender);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::AppStorage;
    use crate::data::duckdb::DuckdbService;
    use chrono::Utc;
    use tempfile::TempDir;

    async fn create_test_service() -> (TempDir, DuckdbService) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let duckdb_dir = temp_dir.path().join("duckdb");
        tokio::fs::create_dir_all(&duckdb_dir)
            .await
            .expect("Failed to create duckdb dir");
        let storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = DuckdbService::init(&storage)
            .await
            .expect("Failed to init analytics service");
        (temp_dir, service)
    }

    #[tokio::test]
    async fn test_insert_empty_batch() {
        let (_temp_dir, analytics) = create_test_service().await;

        let conn = analytics.conn();
        let result = insert_batch(&conn, &[]);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_insert_span() {
        let (_temp_dir, analytics) = create_test_service().await;

        let span = NormalizedSpan {
            trace_id: "abc123".to_string(),
            span_id: "def456".to_string(),
            span_name: "test-span".to_string(),
            timestamp_start: Utc::now(),
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            let result = insert_batch(&conn, &[span]);
            assert!(result.is_ok());
        }

        let conn = analytics.conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'abc123'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_insert_span_with_raw_span() {
        let (_temp_dir, analytics) = create_test_service().await;

        let span = NormalizedSpan {
            trace_id: "trace1".to_string(),
            span_id: "span1".to_string(),
            span_name: "test".to_string(),
            timestamp_start: Utc::now(),
            raw_span: serde_json::json!({
                "trace_id": "trace1",
                "span_id": "span1",
                "events": [
                    {"time_unix_nano": 1000000000, "name": "test-event", "attributes": {"key": "value"}}
                ],
                "links": [
                    {"trace_id": "linked_trace", "span_id": "linked_span", "attributes": null}
                ]
            }),
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            let result = insert_batch(&conn, &[span]);
            assert!(result.is_ok());
        }

        let conn = analytics.conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'trace1'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_events_roundtrip_via_raw_span() {
        use crate::data::duckdb::repositories::query::get_events_for_span;

        let (_temp_dir, analytics) = create_test_service().await;

        let span = NormalizedSpan {
            project_id: Some("test-project".to_string()),
            trace_id: "trace-rt".to_string(),
            span_id: "span-rt".to_string(),
            span_name: "roundtrip-test".to_string(),
            timestamp_start: Utc::now(),
            raw_span: serde_json::json!({
                "trace_id": "trace-rt",
                "span_id": "span-rt",
                "events": [
                    {"timestamp": "2025-01-01T00:00:01.000000Z", "name": "event-1", "attributes": {"key1": "value1"}},
                    {"timestamp": "2025-01-01T00:00:02.000000Z", "name": "event-2", "attributes": {"key2": 42}}
                ],
                "links": []
            }),
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[span]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let events = get_events_for_span(&conn, "test-project", "trace-rt", "span-rt")
            .expect("Query should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_name, Some("event-1".to_string()));
        assert_eq!(events[1].event_name, Some("event-2".to_string()));
    }

    #[tokio::test]
    async fn test_links_roundtrip_via_raw_span() {
        use crate::data::duckdb::repositories::query::get_links_for_span;

        let (_temp_dir, analytics) = create_test_service().await;

        let span = NormalizedSpan {
            project_id: Some("test-project".to_string()),
            trace_id: "trace-rt".to_string(),
            span_id: "span-rt".to_string(),
            span_name: "roundtrip-test".to_string(),
            timestamp_start: Utc::now(),
            raw_span: serde_json::json!({
                "trace_id": "trace-rt",
                "span_id": "span-rt",
                "events": [],
                "links": [
                    {"trace_id": "linked-trace-1", "span_id": "linked-span-1", "attributes": {"relation": "parent"}},
                    {"trace_id": "linked-trace-2", "span_id": "linked-span-2", "attributes": null}
                ]
            }),
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[span]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let links = get_links_for_span(&conn, "test-project", "trace-rt", "span-rt")
            .expect("Query should succeed");

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].linked_trace_id, "linked-trace-1");
        assert_eq!(links[0].linked_span_id, "linked-span-1");
        assert_eq!(links[1].linked_trace_id, "linked-trace-2");
        assert_eq!(links[1].linked_span_id, "linked-span-2");
    }

    #[tokio::test]
    async fn test_empty_events_returns_empty_vec() {
        use crate::data::duckdb::repositories::query::get_events_for_span;

        let (_temp_dir, analytics) = create_test_service().await;

        let span = NormalizedSpan {
            project_id: Some("test-project".to_string()),
            trace_id: "trace-empty".to_string(),
            span_id: "span-empty".to_string(),
            span_name: "no-events".to_string(),
            timestamp_start: Utc::now(),
            raw_span: serde_json::json!({
                "trace_id": "trace-empty",
                "span_id": "span-empty",
                "events": [],
                "links": []
            }),
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[span]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let events = get_events_for_span(&conn, "test-project", "trace-empty", "span-empty")
            .expect("Query should succeed");

        assert!(events.is_empty());
    }
}
