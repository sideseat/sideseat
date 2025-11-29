//! Session query endpoint handlers

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::otel::query::Cursor;
use crate::otel::storage::sqlite::{SessionSummary, delete_session, get_session, list_sessions};
use crate::otel::{OtelError, OtelManager};

/// Create session routes
pub fn create_routes(otel: Arc<OtelManager>) -> Router {
    Router::new()
        .route("/", get(get_sessions))
        .route("/{session_id}", get(get_session_detail).delete(handle_delete_session))
        .route("/{session_id}/traces", get(get_session_traces))
        .with_state(otel)
}

/// Query params for session listing
#[derive(Debug, Deserialize)]
pub struct SessionQueryParams {
    pub user_id: Option<String>,
    pub service: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

/// Session list response
#[derive(Debug, Serialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionDto>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Session DTO for API responses
#[derive(Debug, Serialize)]
pub struct SessionDto {
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
    pub duration_ns: i64,
}

impl From<SessionSummary> for SessionDto {
    fn from(s: SessionSummary) -> Self {
        Self {
            session_id: s.session_id,
            user_id: s.user_id,
            service_name: s.service_name,
            trace_count: s.trace_count,
            span_count: s.span_count,
            total_input_tokens: s.total_input_tokens,
            total_output_tokens: s.total_output_tokens,
            total_tokens: s.total_tokens,
            has_errors: s.has_errors,
            first_seen_ns: s.first_seen_ns,
            last_seen_ns: s.last_seen_ns,
            duration_ns: s.last_seen_ns - s.first_seen_ns,
        }
    }
}

/// GET /sessions - List sessions with filters
pub async fn get_sessions(
    State(otel): State<Arc<OtelManager>>,
    Query(params): Query<SessionQueryParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).min(100);
    let cursor = params.cursor.as_ref().and_then(|c| Cursor::decode(c));

    match list_sessions(
        &otel.pool,
        params.user_id.as_deref(),
        params.service.as_deref(),
        cursor.as_ref().map(|c| c.timestamp),
        cursor.as_ref().map(|c| c.id.as_str()),
        limit + 1, // Fetch one extra to detect if more pages exist
    )
    .await
    {
        Ok(sessions) => {
            let has_more = sessions.len() > limit;
            let sessions: Vec<SessionSummary> = sessions.into_iter().take(limit).collect();

            let next_cursor = if has_more {
                sessions.last().map(|s| {
                    Cursor { timestamp: s.last_seen_ns, id: s.session_id.clone() }.encode()
                })
            } else {
                None
            };

            let response = SessionListResponse {
                sessions: sessions.into_iter().map(|s| s.into()).collect(),
                next_cursor,
                has_more,
            };
            Json(response).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// GET /sessions/{session_id} - Get single session
pub async fn get_session_detail(
    State(otel): State<Arc<OtelManager>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match get_session(&otel.pool, &session_id).await {
        Ok(Some(session)) => {
            let dto: SessionDto = session.into();
            Json(dto).into_response()
        }
        Ok(None) => OtelError::SessionNotFound(session_id).into_response(),
        Err(e) => e.into_response(),
    }
}

/// DELETE /sessions/{session_id} - Delete a session and all associated data
pub async fn handle_delete_session(
    State(otel): State<Arc<OtelManager>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match delete_session(&otel.pool, &session_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => OtelError::SessionNotFound(session_id).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Query params for session traces listing
#[derive(Debug, Deserialize)]
pub struct SessionTracesQueryParams {
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

/// Trace summary for session traces response
#[derive(Debug, Serialize)]
pub struct SessionTraceDto {
    pub trace_id: String,
    pub root_span_name: Option<String>,
    pub span_count: i32,
    pub start_time_ns: i64,
    pub duration_ns: Option<i64>,
    pub has_errors: bool,
}

/// Session traces list response
#[derive(Debug, Serialize)]
pub struct SessionTracesResponse {
    pub traces: Vec<SessionTraceDto>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// GET /sessions/{session_id}/traces - Get traces for a session
pub async fn get_session_traces(
    State(otel): State<Arc<OtelManager>>,
    Path(session_id): Path<String>,
    Query(params): Query<SessionTracesQueryParams>,
) -> impl IntoResponse {
    let pool = &otel.pool;
    let limit = params.limit.unwrap_or(50).min(100);
    let cursor = params.cursor.as_ref().and_then(|c| Cursor::decode(c));

    // Build query with cursor-based pagination
    let mut sql = String::from(
        r#"
        SELECT trace_id, root_span_name, span_count, start_time_ns, duration_ns, has_errors
        FROM traces
        WHERE session_id = ?
        "#,
    );

    if cursor.is_some() {
        sql.push_str(" AND (start_time_ns, trace_id) < (?, ?)");
    }
    sql.push_str(" ORDER BY start_time_ns DESC, trace_id DESC LIMIT ?");

    let mut query =
        sqlx::query_as::<_, (String, Option<String>, i32, i64, Option<i64>, bool)>(&sql)
            .bind(&session_id);

    if let Some(ref c) = cursor {
        query = query.bind(c.timestamp).bind(&c.id);
    }
    query = query.bind((limit + 1) as i64);

    let result = query.fetch_all(pool).await;

    match result {
        Ok(rows) => {
            let has_more = rows.len() > limit;
            let rows: Vec<_> = rows.into_iter().take(limit).collect();

            let next_cursor = if has_more {
                rows.last().map(|r| Cursor { timestamp: r.3, id: r.0.clone() }.encode())
            } else {
                None
            };

            let traces: Vec<SessionTraceDto> = rows
                .into_iter()
                .map(|r| SessionTraceDto {
                    trace_id: r.0,
                    root_span_name: r.1,
                    span_count: r.2,
                    start_time_ns: r.3,
                    duration_ns: r.4,
                    has_errors: r.5,
                })
                .collect();

            let response = SessionTracesResponse { traces, next_cursor, has_more };
            Json(response).into_response()
        }
        Err(e) => {
            OtelError::StorageError(format!("Failed to get session traces: {}", e)).into_response()
        }
    }
}
