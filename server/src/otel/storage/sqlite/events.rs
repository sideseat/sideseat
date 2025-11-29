//! Span event index operations

use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::otel::error::OtelError;
use crate::otel::normalize::SpanEvent;

/// Insert span events batch using an existing transaction
/// Uses multi-row INSERT for efficiency (up to 50 events per statement)
pub async fn insert_events_batch_with_tx(
    tx: &mut Transaction<'_, Sqlite>,
    events: &[SpanEvent],
) -> Result<(), OtelError> {
    if events.is_empty() {
        return Ok(());
    }

    // SQLite has a limit of 999 variables per query
    // With 6 columns per event, we can insert ~160 events per query
    // Use 50 for consistency with spans
    const CHUNK_SIZE: usize = 50;

    for chunk in events.chunks(CHUNK_SIZE) {
        let placeholders: Vec<String> =
            chunk.iter().map(|_| "(?, ?, ?, ?, ?, ?)".to_string()).collect();

        let sql = format!(
            r#"INSERT INTO span_events (
                span_id, trace_id, event_name, event_time_ns,
                content_preview, attributes_json
            ) VALUES {}"#,
            placeholders.join(", ")
        );

        let mut query = sqlx::query(&sql);
        for event in chunk {
            query = query
                .bind(&event.span_id)
                .bind(&event.trace_id)
                .bind(&event.event_name)
                .bind(event.event_time_ns)
                .bind(&event.content_preview)
                .bind(&event.attributes_json);
        }

        query.execute(&mut **tx).await.map_err(|e| {
            OtelError::StorageError(format!("Failed to insert events batch: {}", e))
        })?;
    }

    Ok(())
}

/// Get events for a span
pub async fn get_events_by_span(
    pool: &SqlitePool,
    span_id: &str,
) -> Result<Vec<EventIndex>, OtelError> {
    let rows = sqlx::query_as::<_, EventIndex>(
        r#"
        SELECT id, span_id, trace_id, event_name, event_time_ns,
               content_preview, attributes_json
        FROM span_events WHERE span_id = ?
        ORDER BY event_time_ns
        "#,
    )
    .bind(span_id)
    .fetch_all(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get events: {}", e)))?;

    Ok(rows)
}

/// Event index record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EventIndex {
    pub id: i64,
    pub span_id: String,
    pub trace_id: String,
    pub event_name: String,
    pub event_time_ns: i64,
    pub content_preview: Option<String>,
    pub attributes_json: Option<String>,
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

    fn create_test_event(span_id: &str, trace_id: &str, event_name: &str) -> SpanEvent {
        SpanEvent {
            span_id: span_id.to_string(),
            trace_id: trace_id.to_string(),
            event_time_ns: 1000000000,
            event_name: event_name.to_string(),
            content_preview: Some(r#"{"key": "value"}"#.to_string()),
            attributes_json: r#"{"key": "value"}"#.to_string(),
        }
    }

    async fn insert_test_trace_and_span(pool: &SqlitePool, trace_id: &str, span_id: &str) {
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

        sqlx::query(
            "INSERT INTO spans (span_id, trace_id, span_name, service_name, detected_framework, start_time_ns, status_code) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(span_id)
        .bind(trace_id)
        .bind("test-span")
        .bind("test-service")
        .bind("unknown")
        .bind(now)
        .bind(0)
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn test_event_index_struct() {
        let event = EventIndex {
            id: 1,
            span_id: "span-1".to_string(),
            trace_id: "trace-1".to_string(),
            event_name: "gen_ai.user.message".to_string(),
            event_time_ns: 1000000000,
            content_preview: Some("Hello".to_string()),
            attributes_json: Some("{}".to_string()),
        };
        assert_eq!(event.id, 1);
        assert_eq!(event.span_id, "span-1");
        assert_eq!(event.trace_id, "trace-1");
        assert_eq!(event.event_name, "gen_ai.user.message");
        assert_eq!(event.event_time_ns, 1000000000);
    }

    #[test]
    fn test_event_index_clone() {
        let event = EventIndex {
            id: 1,
            span_id: "span-1".to_string(),
            trace_id: "trace-1".to_string(),
            event_name: "test".to_string(),
            event_time_ns: 1000,
            content_preview: None,
            attributes_json: None,
        };
        let cloned = event.clone();
        assert_eq!(cloned.id, event.id);
        assert_eq!(cloned.span_id, event.span_id);
    }

    #[tokio::test]
    async fn test_insert_events_batch_empty() {
        let pool = setup_test_db().await;
        let mut tx = pool.begin().await.unwrap();

        let result = insert_events_batch_with_tx(&mut tx, &[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_insert_events_batch_single() {
        let pool = setup_test_db().await;
        insert_test_trace_and_span(&pool, "trace-1", "span-1").await;

        let events = vec![create_test_event("span-1", "trace-1", "event-1")];

        let mut tx = pool.begin().await.unwrap();
        insert_events_batch_with_tx(&mut tx, &events).await.unwrap();
        tx.commit().await.unwrap();

        let result = get_events_by_span(&pool, "span-1").await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].event_name, "event-1");
    }

    #[tokio::test]
    async fn test_insert_events_batch_multiple() {
        let pool = setup_test_db().await;
        insert_test_trace_and_span(&pool, "trace-1", "span-1").await;

        let events = vec![
            create_test_event("span-1", "trace-1", "event-1"),
            create_test_event("span-1", "trace-1", "event-2"),
            create_test_event("span-1", "trace-1", "event-3"),
        ];

        let mut tx = pool.begin().await.unwrap();
        insert_events_batch_with_tx(&mut tx, &events).await.unwrap();
        tx.commit().await.unwrap();

        let result = get_events_by_span(&pool, "span-1").await.unwrap();
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn test_get_events_by_span_empty() {
        let pool = setup_test_db().await;

        let result = get_events_by_span(&pool, "nonexistent").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_get_events_by_span_ordered_by_time() {
        let pool = setup_test_db().await;
        insert_test_trace_and_span(&pool, "trace-1", "span-1").await;

        let mut event1 = create_test_event("span-1", "trace-1", "first");
        event1.event_time_ns = 1000;

        let mut event2 = create_test_event("span-1", "trace-1", "second");
        event2.event_time_ns = 3000;

        let mut event3 = create_test_event("span-1", "trace-1", "third");
        event3.event_time_ns = 2000;

        let events = vec![event1, event2, event3];

        let mut tx = pool.begin().await.unwrap();
        insert_events_batch_with_tx(&mut tx, &events).await.unwrap();
        tx.commit().await.unwrap();

        let result = get_events_by_span(&pool, "span-1").await.unwrap();
        assert_eq!(result.len(), 3);
        // Should be ordered by event_time_ns
        assert_eq!(result[0].event_name, "first");
        assert_eq!(result[1].event_name, "third");
        assert_eq!(result[2].event_name, "second");
    }

    #[tokio::test]
    async fn test_events_different_spans() {
        let pool = setup_test_db().await;
        insert_test_trace_and_span(&pool, "trace-1", "span-1").await;

        // Insert second span for same trace
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        sqlx::query(
            "INSERT INTO spans (span_id, trace_id, span_name, service_name, detected_framework, start_time_ns, status_code) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind("span-2")
        .bind("trace-1")
        .bind("test-span-2")
        .bind("test-service")
        .bind("unknown")
        .bind(now)
        .bind(0)
        .execute(&pool)
        .await
        .unwrap();

        let events = vec![
            create_test_event("span-1", "trace-1", "event-for-span1"),
            create_test_event("span-2", "trace-1", "event-for-span2"),
        ];

        let mut tx = pool.begin().await.unwrap();
        insert_events_batch_with_tx(&mut tx, &events).await.unwrap();
        tx.commit().await.unwrap();

        let span1_events = get_events_by_span(&pool, "span-1").await.unwrap();
        assert_eq!(span1_events.len(), 1);
        assert_eq!(span1_events[0].event_name, "event-for-span1");

        let span2_events = get_events_by_span(&pool, "span-2").await.unwrap();
        assert_eq!(span2_events.len(), 1);
        assert_eq!(span2_events[0].event_name, "event-for-span2");
    }
}
