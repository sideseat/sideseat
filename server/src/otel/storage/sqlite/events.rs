//! Span event index operations

use sqlx::SqlitePool;

use crate::otel::error::OtelError;
use crate::otel::normalize::SpanEvent;

/// Insert a span event into the index
pub async fn insert_event(pool: &SqlitePool, event: &SpanEvent) -> Result<(), OtelError> {
    sqlx::query(
        r#"
        INSERT INTO span_events (span_id, trace_id, event_name, event_time_ns)
        VALUES (?, ?, ?, ?)
        "#,
    )
    .bind(&event.span_id)
    .bind(&event.trace_id)
    .bind(&event.event_name)
    .bind(event.event_time_ns)
    .execute(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to insert event: {}", e)))?;

    Ok(())
}

/// Get events for a span
pub async fn get_events_by_span(
    pool: &SqlitePool,
    span_id: &str,
) -> Result<Vec<EventIndex>, OtelError> {
    let rows = sqlx::query_as::<_, EventIndex>(
        r#"
        SELECT id, span_id, trace_id, event_name, event_time_ns
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
}
