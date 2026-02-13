//! Message query operations
//!
//! Provides queries for extracting conversation messages from spans.
//! Messages are stored as raw JSON (SideML conversion happens at query time).
//!
//! This repository only handles data retrieval. All message processing
//! (filtering, deduplication, sorting, metadata harvesting) is done by
//! the feed pipeline (process_spans) in the domain layer.

use duckdb::Connection;

use crate::data::duckdb::DuckdbError;
use crate::data::types::{
    FeedMessagesParams, MessageQueryParams, MessageQueryResult, MessageSpanRow,
};
use crate::utils::time::micros_to_datetime;

/// Shared SELECT columns for all message queries.
/// Column order must match `parse_span_row()` field extraction.
const MESSAGE_SELECT_COLUMNS: &str = r#"
    trace_id,
    span_id,
    parent_span_id,
    EPOCH_US(timestamp_start) AS span_timestamp_us,
    EPOCH_US(timestamp_end) AS span_end_timestamp_us,
    messages,
    gen_ai_request_model AS model,
    gen_ai_system AS provider,
    status_code,
    exception_type,
    exception_message,
    exception_stacktrace,
    gen_ai_usage_input_tokens AS input_tokens,
    gen_ai_usage_output_tokens AS output_tokens,
    gen_ai_usage_total_tokens AS total_tokens,
    gen_ai_cost_total::DOUBLE AS cost_total,
    tool_definitions,
    tool_names,
    observation_type,
    session_id,
    EPOCH_US(ingested_at) AS ingested_at_us"#;

/// Shared content filter for message queries.
/// Includes error spans even without messages.
const MESSAGE_CONTENT_FILTER: &str =
    "(messages != '[]' OR tool_definitions != '[]' OR tool_names != '[]' OR status_code = 'ERROR')";

// ============================================================================
// Query functions - return raw unfiltered data
// ============================================================================

/// Get span rows for a span, trace, or session (unified query).
///
/// Priority: span_id > session_id > trace_id
pub fn get_messages(
    conn: &Connection,
    params: &MessageQueryParams,
) -> Result<MessageQueryResult, DuckdbError> {
    let mut conditions = vec!["project_id = ?".to_string()];
    let mut bind_values: Vec<String> = vec![params.project_id.clone()];

    if let Some(span_id) = &params.span_id {
        conditions.push("span_id = ?".to_string());
        bind_values.push(span_id.clone());
    } else if let Some(session_id) = &params.session_id {
        conditions.push(
            "trace_id IN (SELECT DISTINCT trace_id FROM otel_spans WHERE project_id = ? AND session_id = ?)".to_string()
        );
        bind_values.push(params.project_id.clone());
        bind_values.push(session_id.clone());
        conditions.push(MESSAGE_CONTENT_FILTER.to_string());
    } else if let Some(trace_id) = &params.trace_id {
        conditions.push("trace_id = ?".to_string());
        bind_values.push(trace_id.clone());
        conditions.push(MESSAGE_CONTENT_FILTER.to_string());
    }

    if let Some(from) = &params.from_timestamp {
        conditions.push("timestamp_start >= ?".to_string());
        bind_values.push(from.format("%Y-%m-%d %H:%M:%S%.6f").to_string());
    }
    if let Some(to) = &params.to_timestamp {
        conditions.push("timestamp_start < ?".to_string());
        bind_values.push(to.format("%Y-%m-%d %H:%M:%S%.6f").to_string());
    }

    let sql = format!(
        "SELECT {MESSAGE_SELECT_COLUMNS} FROM otel_spans WHERE {} ORDER BY timestamp_start ASC",
        conditions.join(" AND ")
    );

    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn duckdb::ToSql> = bind_values
        .iter()
        .map(|s| s as &dyn duckdb::ToSql)
        .collect();
    let rows: Vec<Result<MessageSpanRow, _>> =
        stmt.query_map(&*params_refs, parse_span_row)?.collect();
    let rows: Vec<MessageSpanRow> = rows.into_iter().collect::<Result<_, _>>()?;
    Ok(MessageQueryResult { rows })
}

/// Get span rows for entire project (feed API).
///
/// Uses cursor-based pagination on (ingested_at, span_id) for stable pagination.
pub fn get_project_messages(
    conn: &Connection,
    params: &FeedMessagesParams,
) -> Result<MessageQueryResult, DuckdbError> {
    let mut conditions = vec![
        "project_id = ?".to_string(),
        MESSAGE_CONTENT_FILTER.to_string(),
    ];
    let mut bind_values: Vec<String> = vec![params.project_id.clone()];

    // Cursor condition: (ingested_at, span_id) < (cursor_time, cursor_span_id)
    if let Some((cursor_time_us, cursor_span_id)) = &params.cursor {
        conditions.push("(EPOCH_US(ingested_at), span_id) < (?::BIGINT, ?)".to_string());
        bind_values.push(cursor_time_us.to_string());
        bind_values.push(cursor_span_id.clone());
    }

    // Event time filters
    if let Some(start) = &params.start_time {
        conditions.push("timestamp_start >= ?".to_string());
        bind_values.push(start.format("%Y-%m-%d %H:%M:%S%.6f").to_string());
    }
    if let Some(end) = &params.end_time {
        conditions.push("timestamp_start < ?".to_string());
        bind_values.push(end.format("%Y-%m-%d %H:%M:%S%.6f").to_string());
    }

    let sql = format!(
        "SELECT {MESSAGE_SELECT_COLUMNS} FROM otel_spans WHERE {} ORDER BY ingested_at DESC, span_id DESC LIMIT {}",
        conditions.join(" AND "),
        params.limit
    );

    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn duckdb::ToSql> = bind_values
        .iter()
        .map(|s| s as &dyn duckdb::ToSql)
        .collect();
    let rows: Vec<Result<MessageSpanRow, _>> =
        stmt.query_map(&*params_refs, parse_span_row)?.collect();
    let rows: Vec<MessageSpanRow> = rows.into_iter().collect::<Result<_, _>>()?;
    Ok(MessageQueryResult { rows })
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
        observation_type: row.get(18)?,
        session_id: row.get(19)?,
        ingested_at: micros_to_datetime(row.get::<_, i64>(20)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::AppStorage;
    use crate::data::duckdb::repositories::span::insert_batch;
    use crate::data::duckdb::{DuckdbService, NormalizedSpan};
    use chrono::{Duration, Utc};
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
            messages: serde_json::from_str(messages_json).unwrap_or_default(),
            ..Default::default()
        }
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
            project_id: project_id.to_string(),
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
            messages: serde_json::json!([]),
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            insert_batch(&conn, &[span_with_messages, span_empty]).expect("Insert should succeed");
        }

        let conn = analytics.conn();
        let params = FeedMessagesParams {
            project_id: project_id.to_string(),
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
            project_id: project_id.to_string(),
            limit: 2,
            ..Default::default()
        };
        let page1 = get_project_messages(&conn, &params).expect("Query should succeed");
        assert_eq!(page1.rows.len(), 2, "First page should have 2 rows");

        // Second page with cursor
        let last_row = page1.rows.last().unwrap();
        let cursor_time_us = last_row.ingested_at.timestamp_micros();
        let params = FeedMessagesParams {
            project_id: project_id.to_string(),
            limit: 2,
            cursor: Some((cursor_time_us, last_row.span_id.clone())),
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
            project_id: project_id.to_string(),
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
            project_id: project_id.to_string(),
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
            project_id: "nonexistent".to_string(),
            limit: 10,
            ..Default::default()
        };
        let result = get_project_messages(&conn, &params).expect("Query should succeed");

        assert!(result.rows.is_empty());
    }
}
