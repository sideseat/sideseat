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
        FROM sessions WHERE session_id = ? AND deleted_at IS NULL
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
        WHERE deleted_at IS NULL
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

/// Soft delete a session
pub async fn soft_delete_session(pool: &SqlitePool, session_id: &str) -> Result<bool, OtelError> {
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

    let result = sqlx::query(
        "UPDATE sessions SET deleted_at = ?, updated_at = ? WHERE session_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(now)
    .bind(session_id)
    .execute(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to delete session: {}", e)))?;

    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
