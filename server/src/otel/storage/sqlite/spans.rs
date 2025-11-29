//! Span index operations

use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::otel::error::OtelError;
use crate::otel::normalize::NormalizedSpan;

/// Insert spans batch using an existing transaction (for atomic operations)
/// Uses multi-row INSERT for efficiency (up to 35 spans per statement due to SQLite variable limits)
pub async fn insert_spans_batch_with_tx(
    tx: &mut Transaction<'_, Sqlite>,
    spans: &[NormalizedSpan],
) -> Result<(), OtelError> {
    if spans.is_empty() {
        return Ok(());
    }

    // SQLite has a default limit of 999 variables per query
    // With 27 columns per span, we can insert ~37 spans per query
    // Use 35 for safety margin
    const CHUNK_SIZE: usize = 35;

    for chunk in spans.chunks(CHUNK_SIZE) {
        let placeholders: Vec<String> = chunk
            .iter()
            .map(|_| {
                "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                    .to_string()
            })
            .collect();

        let sql = format!(
            r#"
            INSERT OR REPLACE INTO spans (
                span_id, trace_id, session_id, parent_span_id, span_name, span_kind, service_name,
                detected_framework, detected_category, gen_ai_system, gen_ai_operation_name,
                gen_ai_agent_name, gen_ai_tool_name, gen_ai_request_model, gen_ai_response_model,
                start_time_ns, end_time_ns, duration_ns, time_to_first_token_ms, request_duration_ms,
                status_code, usage_input_tokens, usage_output_tokens, usage_total_tokens,
                usage_cache_read_tokens, usage_cache_write_tokens, data_json
            ) VALUES {}
            "#,
            placeholders.join(", ")
        );

        let mut query = sqlx::query(&sql);
        for span in chunk {
            // Serialize full span to JSON for data_json column
            let data_json = serde_json::to_string(span).ok();
            query = query
                .bind(&span.span_id)
                .bind(&span.trace_id)
                .bind(&span.session_id)
                .bind(&span.parent_span_id)
                .bind(&span.span_name)
                .bind(span.span_kind as i32)
                .bind(&span.service_name)
                .bind(&span.detected_framework)
                .bind(&span.detected_category)
                .bind(&span.gen_ai_system)
                .bind(&span.gen_ai_operation_name)
                .bind(&span.gen_ai_agent_name)
                .bind(&span.gen_ai_tool_name)
                .bind(&span.gen_ai_request_model)
                .bind(&span.gen_ai_response_model)
                .bind(span.start_time_unix_nano)
                .bind(span.end_time_unix_nano)
                .bind(span.duration_ns)
                .bind(span.time_to_first_token_ms)
                .bind(span.request_duration_ms)
                .bind(span.status_code as i32)
                .bind(span.usage_input_tokens)
                .bind(span.usage_output_tokens)
                .bind(span.usage_total_tokens)
                .bind(span.usage_cache_read_tokens)
                .bind(span.usage_cache_write_tokens)
                .bind(data_json);
        }

        query
            .execute(&mut **tx)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to insert spans batch: {}", e)))?;
    }

    Ok(())
}

/// Get a single span by ID
pub async fn get_span_by_id(
    pool: &SqlitePool,
    span_id: &str,
) -> Result<Option<SpanIndex>, OtelError> {
    let row = sqlx::query_as::<_, SpanIndex>(
        r#"
        SELECT span_id, trace_id, session_id, parent_span_id, span_name, span_kind, service_name,
               detected_framework, detected_category, gen_ai_system, gen_ai_operation_name,
               gen_ai_agent_name, gen_ai_tool_name, gen_ai_request_model, gen_ai_response_model,
               start_time_ns, end_time_ns, duration_ns, time_to_first_token_ms, request_duration_ms,
               status_code, usage_input_tokens, usage_output_tokens, usage_total_tokens,
               usage_cache_read_tokens, usage_cache_write_tokens, data_json
        FROM spans WHERE span_id = ?
        "#,
    )
    .bind(span_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get span: {}", e)))?;

    Ok(row)
}

/// Span index record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SpanIndex {
    pub span_id: String,
    pub trace_id: String,
    pub session_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub span_name: String,
    pub span_kind: i32,
    pub service_name: String,
    pub detected_framework: String,
    pub detected_category: Option<String>,
    // Gen AI fields
    pub gen_ai_system: Option<String>,
    pub gen_ai_operation_name: Option<String>,
    pub gen_ai_agent_name: Option<String>,
    pub gen_ai_tool_name: Option<String>,
    pub gen_ai_request_model: Option<String>,
    pub gen_ai_response_model: Option<String>,
    // Timing
    pub start_time_ns: i64,
    pub end_time_ns: Option<i64>,
    pub duration_ns: Option<i64>,
    // Performance metrics
    pub time_to_first_token_ms: Option<i64>,
    pub request_duration_ms: Option<i64>,
    // Status
    pub status_code: i32,
    // Token usage
    pub usage_input_tokens: Option<i64>,
    pub usage_output_tokens: Option<i64>,
    pub usage_total_tokens: Option<i64>,
    pub usage_cache_read_tokens: Option<i64>,
    pub usage_cache_write_tokens: Option<i64>,
    // Full data
    pub data_json: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::schema::SCHEMA;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(SCHEMA).execute(&pool).await.unwrap();
        pool
    }

    fn create_test_span(trace_id: &str, span_id: &str) -> NormalizedSpan {
        NormalizedSpan {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: None,
            start_time_unix_nano: 1000000000,
            end_time_unix_nano: Some(2000000000),
            duration_ns: Some(1000000000),
            service_name: "test-service".to_string(),
            span_name: "test-span".to_string(),
            span_kind: 1,
            status_code: 0,
            detected_framework: "unknown".to_string(),
            detected_category: Some("llm".to_string()),
            attributes_json: "{}".to_string(),
            ..Default::default()
        }
    }

    async fn insert_test_trace(pool: &SqlitePool, trace_id: &str) {
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        sqlx::query(
            "INSERT INTO traces (trace_id, service_name, detected_framework, span_count, start_time_ns, has_errors, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(trace_id)
        .bind("test-service")
        .bind("unknown")
        .bind(1)
        .bind(now)
        .bind(false)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn test_span_index_struct() {
        let span = SpanIndex {
            span_id: "span-1".to_string(),
            trace_id: "trace-1".to_string(),
            session_id: None,
            parent_span_id: None,
            span_name: "test-span".to_string(),
            span_kind: 1,
            service_name: "test-service".to_string(),
            detected_framework: "strands".to_string(),
            detected_category: Some("llm".to_string()),
            gen_ai_system: Some("anthropic".to_string()),
            gen_ai_operation_name: Some("chat".to_string()),
            gen_ai_agent_name: Some("assistant".to_string()),
            gen_ai_tool_name: None,
            gen_ai_request_model: Some("claude-3".to_string()),
            gen_ai_response_model: Some("claude-3".to_string()),
            start_time_ns: 1000,
            end_time_ns: Some(2000),
            duration_ns: Some(1000),
            time_to_first_token_ms: Some(100),
            request_duration_ms: Some(500),
            status_code: 0,
            usage_input_tokens: Some(100),
            usage_output_tokens: Some(200),
            usage_total_tokens: Some(300),
            usage_cache_read_tokens: Some(50),
            usage_cache_write_tokens: Some(25),
            data_json: Some("{}".to_string()),
        };
        assert_eq!(span.span_id, "span-1");
        assert_eq!(span.trace_id, "trace-1");
        assert_eq!(span.service_name, "test-service");
        assert_eq!(span.detected_framework, "strands");
        assert_eq!(span.gen_ai_system, Some("anthropic".to_string()));
        assert_eq!(span.usage_total_tokens, Some(300));
    }

    #[test]
    fn test_span_index_clone() {
        let span = SpanIndex {
            span_id: "span-1".to_string(),
            trace_id: "trace-1".to_string(),
            session_id: None,
            parent_span_id: None,
            span_name: "test".to_string(),
            span_kind: 0,
            service_name: "svc".to_string(),
            detected_framework: "unknown".to_string(),
            detected_category: None,
            gen_ai_system: None,
            gen_ai_operation_name: None,
            gen_ai_agent_name: None,
            gen_ai_tool_name: None,
            gen_ai_request_model: None,
            gen_ai_response_model: None,
            start_time_ns: 1000,
            end_time_ns: None,
            duration_ns: None,
            time_to_first_token_ms: None,
            request_duration_ms: None,
            status_code: 0,
            usage_input_tokens: None,
            usage_output_tokens: None,
            usage_total_tokens: None,
            usage_cache_read_tokens: None,
            usage_cache_write_tokens: None,
            data_json: None,
        };
        let cloned = span.clone();
        assert_eq!(cloned.span_id, span.span_id);
        assert_eq!(cloned.trace_id, span.trace_id);
    }

    #[tokio::test]
    async fn test_insert_spans_batch_empty() {
        let pool = setup_test_db().await;
        let mut tx = pool.begin().await.unwrap();

        let result = insert_spans_batch_with_tx(&mut tx, &[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_insert_spans_batch_single() {
        let pool = setup_test_db().await;
        insert_test_trace(&pool, "trace-1").await;

        let spans = vec![create_test_span("trace-1", "span-1")];

        let mut tx = pool.begin().await.unwrap();
        insert_spans_batch_with_tx(&mut tx, &spans).await.unwrap();
        tx.commit().await.unwrap();

        let result = get_span_by_id(&pool, "span-1").await.unwrap();
        assert!(result.is_some());
        let span = result.unwrap();
        assert_eq!(span.span_id, "span-1");
        assert_eq!(span.trace_id, "trace-1");
        assert_eq!(span.service_name, "test-service");
    }

    #[tokio::test]
    async fn test_insert_spans_batch_multiple() {
        let pool = setup_test_db().await;
        insert_test_trace(&pool, "trace-1").await;

        let spans = vec![
            create_test_span("trace-1", "span-1"),
            create_test_span("trace-1", "span-2"),
            create_test_span("trace-1", "span-3"),
        ];

        let mut tx = pool.begin().await.unwrap();
        insert_spans_batch_with_tx(&mut tx, &spans).await.unwrap();
        tx.commit().await.unwrap();

        assert!(get_span_by_id(&pool, "span-1").await.unwrap().is_some());
        assert!(get_span_by_id(&pool, "span-2").await.unwrap().is_some());
        assert!(get_span_by_id(&pool, "span-3").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_get_span_by_id_not_found() {
        let pool = setup_test_db().await;

        let result = get_span_by_id(&pool, "nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_insert_span_with_parent() {
        let pool = setup_test_db().await;
        insert_test_trace(&pool, "trace-1").await;

        let mut parent = create_test_span("trace-1", "parent-span");
        parent.parent_span_id = None;

        let mut child = create_test_span("trace-1", "child-span");
        child.parent_span_id = Some("parent-span".to_string());

        let spans = vec![parent, child];

        let mut tx = pool.begin().await.unwrap();
        insert_spans_batch_with_tx(&mut tx, &spans).await.unwrap();
        tx.commit().await.unwrap();

        let child_span = get_span_by_id(&pool, "child-span").await.unwrap().unwrap();
        assert_eq!(child_span.parent_span_id, Some("parent-span".to_string()));
    }

    #[tokio::test]
    async fn test_insert_span_with_gen_ai_fields() {
        let pool = setup_test_db().await;
        insert_test_trace(&pool, "trace-1").await;

        let mut span = create_test_span("trace-1", "span-1");
        span.gen_ai_system = Some("anthropic".to_string());
        span.gen_ai_operation_name = Some("chat".to_string());
        span.gen_ai_agent_name = Some("assistant".to_string());
        span.gen_ai_tool_name = Some("search".to_string());
        span.gen_ai_request_model = Some("claude-3-opus".to_string());
        span.gen_ai_response_model = Some("claude-3-opus".to_string());
        span.usage_input_tokens = Some(100);
        span.usage_output_tokens = Some(200);
        span.usage_total_tokens = Some(300);

        let mut tx = pool.begin().await.unwrap();
        insert_spans_batch_with_tx(&mut tx, &[span]).await.unwrap();
        tx.commit().await.unwrap();

        let result = get_span_by_id(&pool, "span-1").await.unwrap().unwrap();
        assert_eq!(result.gen_ai_system, Some("anthropic".to_string()));
        assert_eq!(result.gen_ai_request_model, Some("claude-3-opus".to_string()));
        assert_eq!(result.usage_input_tokens, Some(100));
        assert_eq!(result.usage_output_tokens, Some(200));
    }

    #[tokio::test]
    async fn test_insert_span_with_session() {
        let pool = setup_test_db().await;

        // Create session first
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        sqlx::query(
            "INSERT INTO sessions (session_id, service_name, trace_count, span_count, first_seen_ns, last_seen_ns, has_errors, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind("session-123")
        .bind("test-service")
        .bind(1)
        .bind(1)
        .bind(now)
        .bind(now)
        .bind(false)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        insert_test_trace(&pool, "trace-1").await;

        let mut span = create_test_span("trace-1", "span-1");
        span.session_id = Some("session-123".to_string());

        let mut tx = pool.begin().await.unwrap();
        insert_spans_batch_with_tx(&mut tx, &[span]).await.unwrap();
        tx.commit().await.unwrap();

        let result = get_span_by_id(&pool, "span-1").await.unwrap().unwrap();
        assert_eq!(result.session_id, Some("session-123".to_string()));
    }

    #[tokio::test]
    async fn test_insert_span_replace_existing() {
        let pool = setup_test_db().await;
        insert_test_trace(&pool, "trace-1").await;

        // Insert initial span
        let mut span1 = create_test_span("trace-1", "span-1");
        span1.span_name = "original".to_string();

        let mut tx = pool.begin().await.unwrap();
        insert_spans_batch_with_tx(&mut tx, &[span1]).await.unwrap();
        tx.commit().await.unwrap();

        // Insert updated span with same ID
        let mut span2 = create_test_span("trace-1", "span-1");
        span2.span_name = "updated".to_string();

        let mut tx = pool.begin().await.unwrap();
        insert_spans_batch_with_tx(&mut tx, &[span2]).await.unwrap();
        tx.commit().await.unwrap();

        let result = get_span_by_id(&pool, "span-1").await.unwrap().unwrap();
        assert_eq!(result.span_name, "updated");
    }

    #[tokio::test]
    async fn test_span_data_json_serialization() {
        let pool = setup_test_db().await;
        insert_test_trace(&pool, "trace-1").await;

        let span = create_test_span("trace-1", "span-1");

        let mut tx = pool.begin().await.unwrap();
        insert_spans_batch_with_tx(&mut tx, &[span]).await.unwrap();
        tx.commit().await.unwrap();

        let result = get_span_by_id(&pool, "span-1").await.unwrap().unwrap();
        // data_json should contain serialized span
        assert!(result.data_json.is_some());
        let data: serde_json::Value =
            serde_json::from_str(result.data_json.as_ref().unwrap()).unwrap();
        assert_eq!(data["span_id"], "span-1");
    }
}
