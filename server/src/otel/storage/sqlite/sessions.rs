//! Session storage operations

use sqlx::{Sqlite, SqlitePool, Transaction};
use std::collections::{HashMap, HashSet};

use crate::otel::error::OtelError;
use crate::otel::normalize::NormalizedSpan;

/// Session summary record
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub user_id: Option<String>,
    pub service_name: Option<String>,
    pub trace_count: i32,
    pub span_count: i32,
    pub total_input_tokens: Option<i64>,
    pub total_output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub has_errors: bool,
    pub first_seen_ns: i64,
    pub last_seen_ns: i64,
}

/// Batch upsert sessions using an existing transaction
pub async fn upsert_sessions_batch_with_tx(
    tx: &mut Transaction<'_, Sqlite>,
    spans: &[NormalizedSpan],
) -> Result<(), OtelError> {
    // Group spans by session_id
    let mut session_spans: HashMap<&str, Vec<&NormalizedSpan>> = HashMap::new();
    for span in spans {
        if let Some(ref session_id) = span.session_id {
            session_spans.entry(session_id).or_default().push(span);
        }
    }

    if session_spans.is_empty() {
        return Ok(());
    }

    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

    for (session_id, session_span_list) in session_spans {
        // Find user_id and service_name from first span that has them
        let user_id = session_span_list.iter().find_map(|s| s.user_id.as_ref());
        let service_name = session_span_list.first().map(|s| &s.service_name);

        // Count unique traces in this batch
        let unique_traces: HashSet<&str> =
            session_span_list.iter().map(|s| s.trace_id.as_str()).collect();
        let trace_count = unique_traces.len() as i32;

        // Aggregate values
        let span_count = session_span_list.len() as i32;
        let min_start = session_span_list.iter().map(|s| s.start_time_unix_nano).min().unwrap_or(0);
        let max_end = session_span_list
            .iter()
            .filter_map(|s| s.end_time_unix_nano)
            .max()
            .unwrap_or(min_start);
        let total_input: i64 = session_span_list.iter().filter_map(|s| s.usage_input_tokens).sum();
        let total_output: i64 =
            session_span_list.iter().filter_map(|s| s.usage_output_tokens).sum();
        let total_tokens: i64 = session_span_list.iter().filter_map(|s| s.usage_total_tokens).sum();
        // OTLP StatusCode: 0=UNSET, 1=OK, 2=ERROR
        let has_error = session_span_list.iter().any(|s| s.status_code == 2);

        sqlx::query(
            r#"
            INSERT INTO sessions (
                session_id, user_id, service_name,
                trace_count, span_count,
                total_input_tokens, total_output_tokens, total_tokens,
                has_errors, first_seen_ns, last_seen_ns,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(session_id) DO UPDATE SET
                user_id = COALESCE(user_id, excluded.user_id),
                service_name = COALESCE(service_name, excluded.service_name),
                trace_count = trace_count + excluded.trace_count,
                span_count = span_count + excluded.span_count,
                total_input_tokens = COALESCE(total_input_tokens, 0) + COALESCE(excluded.total_input_tokens, 0),
                total_output_tokens = COALESCE(total_output_tokens, 0) + COALESCE(excluded.total_output_tokens, 0),
                total_tokens = COALESCE(total_tokens, 0) + COALESCE(excluded.total_tokens, 0),
                has_errors = has_errors OR excluded.has_errors,
                first_seen_ns = MIN(first_seen_ns, excluded.first_seen_ns),
                last_seen_ns = MAX(last_seen_ns, excluded.last_seen_ns),
                updated_at = excluded.updated_at
            "#,
        )
        .bind(session_id)
        .bind(user_id)
        .bind(service_name)
        .bind(trace_count)
        .bind(span_count)
        .bind(if total_input > 0 { Some(total_input) } else { None })
        .bind(if total_output > 0 { Some(total_output) } else { None })
        .bind(if total_tokens > 0 { Some(total_tokens) } else { None })
        .bind(has_error)
        .bind(min_start)
        .bind(max_end)
        .bind(now)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to upsert session: {}", e)))?;
    }

    Ok(())
}

/// Get a session by ID
pub async fn get_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<SessionSummary>, OtelError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            Option<String>,
            i32,
            i32,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            bool,
            i64,
            i64,
        ),
    >(
        r#"
        SELECT session_id, user_id, service_name,
               trace_count, span_count,
               total_input_tokens, total_output_tokens, total_tokens,
               has_errors, first_seen_ns, last_seen_ns
        FROM sessions WHERE session_id = ?
        "#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get session: {}", e)))?;

    Ok(row.map(|r| SessionSummary {
        session_id: r.0,
        user_id: r.1,
        service_name: r.2,
        trace_count: r.3,
        span_count: r.4,
        total_input_tokens: r.5,
        total_output_tokens: r.6,
        total_tokens: r.7,
        has_errors: r.8,
        first_seen_ns: r.9,
        last_seen_ns: r.10,
    }))
}

/// List sessions with optional filters and cursor-based pagination
pub async fn list_sessions(
    pool: &SqlitePool,
    user_id: Option<&str>,
    service_name: Option<&str>,
    cursor_timestamp: Option<i64>,
    cursor_id: Option<&str>,
    limit: usize,
) -> Result<Vec<SessionSummary>, OtelError> {
    let mut sql = String::from(
        r#"
        SELECT session_id, user_id, service_name,
               trace_count, span_count,
               total_input_tokens, total_output_tokens, total_tokens,
               has_errors, first_seen_ns, last_seen_ns
        FROM sessions
        WHERE 1=1
        "#,
    );

    let mut params: Vec<String> = vec![];

    if let Some(uid) = user_id {
        sql.push_str(" AND user_id = ?");
        params.push(uid.to_string());
    }

    if let Some(svc) = service_name {
        sql.push_str(" AND service_name = ?");
        params.push(svc.to_string());
    }

    // Cursor-based pagination: get items after the cursor position
    if let (Some(ts), Some(id)) = (cursor_timestamp, cursor_id) {
        sql.push_str(" AND (last_seen_ns, session_id) < (?, ?)");
        params.push(ts.to_string());
        params.push(id.to_string());
    }

    sql.push_str(" ORDER BY last_seen_ns DESC, session_id DESC LIMIT ?");

    let mut query = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            Option<String>,
            i32,
            i32,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            bool,
            i64,
            i64,
        ),
    >(&sql);

    for param in &params {
        query = query.bind(param);
    }
    query = query.bind(limit as i64);

    let rows = query
        .fetch_all(pool)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to list sessions: {}", e)))?;

    Ok(rows
        .into_iter()
        .map(|r| SessionSummary {
            session_id: r.0,
            user_id: r.1,
            service_name: r.2,
            trace_count: r.3,
            span_count: r.4,
            total_input_tokens: r.5,
            total_output_tokens: r.6,
            total_tokens: r.7,
            has_errors: r.8,
            first_seen_ns: r.9,
            last_seen_ns: r.10,
        })
        .collect())
}

/// Hard delete a session and all associated traces
pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<bool, OtelError> {
    use super::traces::delete_trace;

    // Start transaction for atomic delete
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to begin transaction: {}", e)))?;

    // Check if session exists first
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM sessions WHERE session_id = ?")
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to check session: {}", e)))?;

    if exists.is_none() {
        return Ok(false);
    }

    // Get all trace_ids for this session
    let trace_ids: Vec<String> =
        sqlx::query_scalar("SELECT trace_id FROM traces WHERE session_id = ?")
            .bind(session_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to get trace ids: {}", e)))?;

    tx.commit()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to commit transaction: {}", e)))?;

    // Delete all traces for this session (this handles all cascading deletes)
    for trace_id in &trace_ids {
        delete_trace(pool, trace_id).await?;
    }

    // Delete the session
    let result = sqlx::query("DELETE FROM sessions WHERE session_id = ?")
        .bind(session_id)
        .execute(pool)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to delete session: {}", e)))?;

    Ok(result.rows_affected() > 0)
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

    fn create_test_span(trace_id: &str, span_id: &str, session_id: Option<&str>) -> NormalizedSpan {
        NormalizedSpan {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            session_id: session_id.map(String::from),
            start_time_unix_nano: 1000000000,
            end_time_unix_nano: Some(2000000000),
            service_name: "test-service".to_string(),
            span_name: "test-span".to_string(),
            status_code: 0,
            ..Default::default()
        }
    }

    #[test]
    fn test_session_summary_fields() {
        let session = SessionSummary {
            session_id: "test-session".to_string(),
            user_id: Some("user-123".to_string()),
            service_name: Some("test-service".to_string()),
            trace_count: 5,
            span_count: 25,
            total_input_tokens: Some(1000),
            total_output_tokens: Some(500),
            total_tokens: Some(1500),
            has_errors: false,
            first_seen_ns: 1000000000,
            last_seen_ns: 2000000000,
        };

        assert_eq!(session.session_id, "test-session");
        assert_eq!(session.user_id, Some("user-123".to_string()));
        assert_eq!(session.trace_count, 5);
        assert_eq!(session.span_count, 25);
        assert!(!session.has_errors);
    }

    #[test]
    fn test_session_summary_clone() {
        let session = SessionSummary {
            session_id: "test".to_string(),
            user_id: None,
            service_name: None,
            trace_count: 1,
            span_count: 1,
            total_input_tokens: None,
            total_output_tokens: None,
            total_tokens: None,
            has_errors: false,
            first_seen_ns: 1000,
            last_seen_ns: 2000,
        };
        let cloned = session.clone();
        assert_eq!(cloned.session_id, session.session_id);
    }

    #[tokio::test]
    async fn test_upsert_sessions_batch_empty() {
        let pool = setup_test_db().await;
        let mut tx = pool.begin().await.unwrap();

        let result = upsert_sessions_batch_with_tx(&mut tx, &[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upsert_sessions_batch_no_session_id() {
        let pool = setup_test_db().await;
        let spans = vec![create_test_span("trace-1", "span-1", None)];

        let mut tx = pool.begin().await.unwrap();
        upsert_sessions_batch_with_tx(&mut tx, &spans).await.unwrap();
        tx.commit().await.unwrap();

        // No sessions should be created
        let sessions = list_sessions(&pool, None, None, None, None, 100).await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_upsert_sessions_batch_single() {
        let pool = setup_test_db().await;
        let spans = vec![create_test_span("trace-1", "span-1", Some("session-1"))];

        let mut tx = pool.begin().await.unwrap();
        upsert_sessions_batch_with_tx(&mut tx, &spans).await.unwrap();
        tx.commit().await.unwrap();

        let session = get_session(&pool, "session-1").await.unwrap();
        assert!(session.is_some());
        let s = session.unwrap();
        assert_eq!(s.session_id, "session-1");
        assert_eq!(s.trace_count, 1);
        assert_eq!(s.span_count, 1);
    }

    #[tokio::test]
    async fn test_upsert_sessions_batch_aggregates_spans() {
        let pool = setup_test_db().await;
        let spans = vec![
            create_test_span("trace-1", "span-1", Some("session-1")),
            create_test_span("trace-1", "span-2", Some("session-1")),
            create_test_span("trace-2", "span-3", Some("session-1")),
        ];

        let mut tx = pool.begin().await.unwrap();
        upsert_sessions_batch_with_tx(&mut tx, &spans).await.unwrap();
        tx.commit().await.unwrap();

        let session = get_session(&pool, "session-1").await.unwrap().unwrap();
        assert_eq!(session.span_count, 3);
        assert_eq!(session.trace_count, 2); // 2 unique traces
    }

    #[tokio::test]
    async fn test_upsert_sessions_batch_with_tokens() {
        let pool = setup_test_db().await;
        let mut span1 = create_test_span("trace-1", "span-1", Some("session-1"));
        span1.usage_input_tokens = Some(100);
        span1.usage_output_tokens = Some(50);
        span1.usage_total_tokens = Some(150);

        let mut span2 = create_test_span("trace-1", "span-2", Some("session-1"));
        span2.usage_input_tokens = Some(200);
        span2.usage_output_tokens = Some(100);
        span2.usage_total_tokens = Some(300);

        let mut tx = pool.begin().await.unwrap();
        upsert_sessions_batch_with_tx(&mut tx, &[span1, span2]).await.unwrap();
        tx.commit().await.unwrap();

        let session = get_session(&pool, "session-1").await.unwrap().unwrap();
        assert_eq!(session.total_input_tokens, Some(300));
        assert_eq!(session.total_output_tokens, Some(150));
        assert_eq!(session.total_tokens, Some(450));
    }

    #[tokio::test]
    async fn test_upsert_sessions_batch_with_error() {
        let pool = setup_test_db().await;
        let mut span = create_test_span("trace-1", "span-1", Some("session-1"));
        span.status_code = 2; // ERROR

        let mut tx = pool.begin().await.unwrap();
        upsert_sessions_batch_with_tx(&mut tx, &[span]).await.unwrap();
        tx.commit().await.unwrap();

        let session = get_session(&pool, "session-1").await.unwrap().unwrap();
        assert!(session.has_errors);
    }

    #[tokio::test]
    async fn test_get_session_not_found() {
        let pool = setup_test_db().await;
        let session = get_session(&pool, "nonexistent").await.unwrap();
        assert!(session.is_none());
    }

    #[tokio::test]
    async fn test_list_sessions_empty() {
        let pool = setup_test_db().await;
        let sessions = list_sessions(&pool, None, None, None, None, 100).await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_list_sessions_with_data() {
        let pool = setup_test_db().await;
        let spans = vec![
            create_test_span("trace-1", "span-1", Some("session-1")),
            create_test_span("trace-2", "span-2", Some("session-2")),
        ];

        let mut tx = pool.begin().await.unwrap();
        upsert_sessions_batch_with_tx(&mut tx, &spans).await.unwrap();
        tx.commit().await.unwrap();

        let sessions = list_sessions(&pool, None, None, None, None, 100).await.unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_list_sessions_filter_by_service() {
        let pool = setup_test_db().await;
        let mut span1 = create_test_span("trace-1", "span-1", Some("session-1"));
        span1.service_name = "service-a".to_string();

        let mut span2 = create_test_span("trace-2", "span-2", Some("session-2"));
        span2.service_name = "service-b".to_string();

        let mut tx = pool.begin().await.unwrap();
        upsert_sessions_batch_with_tx(&mut tx, &[span1, span2]).await.unwrap();
        tx.commit().await.unwrap();

        let sessions =
            list_sessions(&pool, None, Some("service-a"), None, None, 100).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "session-1");
    }

    #[tokio::test]
    async fn test_list_sessions_with_limit() {
        let pool = setup_test_db().await;
        let spans = vec![
            create_test_span("trace-1", "span-1", Some("session-1")),
            create_test_span("trace-2", "span-2", Some("session-2")),
            create_test_span("trace-3", "span-3", Some("session-3")),
        ];

        let mut tx = pool.begin().await.unwrap();
        upsert_sessions_batch_with_tx(&mut tx, &spans).await.unwrap();
        tx.commit().await.unwrap();

        let sessions = list_sessions(&pool, None, None, None, None, 2).await.unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_session_not_found() {
        let pool = setup_test_db().await;
        let result = delete_session(&pool, "nonexistent").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_delete_session_success() {
        let pool = setup_test_db().await;
        let spans = vec![create_test_span("trace-1", "span-1", Some("session-1"))];

        let mut tx = pool.begin().await.unwrap();
        upsert_sessions_batch_with_tx(&mut tx, &spans).await.unwrap();
        tx.commit().await.unwrap();

        // Verify session exists
        assert!(get_session(&pool, "session-1").await.unwrap().is_some());

        // Delete session
        let result = delete_session(&pool, "session-1").await.unwrap();
        assert!(result);

        // Verify session is gone
        assert!(get_session(&pool, "session-1").await.unwrap().is_none());
    }
}
