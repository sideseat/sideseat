//! Cursor-only chronological search across spans and logs.

use axum::Json;
use axum::extract::State;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::{Validate, ValidationError};

use super::OtelApiState;
use crate::auth::ProjectRead;
use crate::extractors::ValidatedQuery;
use crate::types::{ApiError, default_limit, parse_timestamp_param, validate_limit};
use sideseat_domain::search::{SearchHit, SearchService, parse};
use sideseat_ports::types::{
    DEFAULT_SEARCH_MAX_EXAMINED, SearchCursor, SearchQuery as PortSearchQuery, SearchRecord,
    SearchSignal,
};

#[derive(Debug, Deserialize, Validate, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    pub q: String,
    pub signal: SearchSignal,
    #[serde(default = "default_limit")]
    #[validate(custom(function = "validate_limit"))]
    pub limit: u32,
    #[serde(default = "default_max_examined")]
    #[validate(custom(function = "validate_max_examined"))]
    pub max_examined: u32,
    pub cursor: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
}

fn default_max_examined() -> u32 {
    DEFAULT_SEARCH_MAX_EXAMINED
}

fn validate_max_examined(value: u32) -> Result<(), ValidationError> {
    if value == 0 || value > 10_000 {
        return Err(ValidationError::new("max_examined")
            .with_message("max_examined must be between 1 and 10000".into()));
    }
    Ok(())
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum SearchRecordDto {
    Span {
        trace_id: String,
        span_id: String,
        timestamp: DateTime<Utc>,
        span_name: Option<String>,
        input_preview: Option<String>,
        output_preview: Option<String>,
    },
    Log {
        log_digest: String,
        ordinal: u32,
        timestamp: DateTime<Utc>,
        severity_text: Option<String>,
        body_text: Option<String>,
        trace_id: Option<String>,
        span_id: Option<String>,
    },
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SearchHitDto {
    pub indeterminate: bool,
    pub fragments: Vec<String>,
    pub record: SearchRecordDto,
}

impl From<SearchHit> for SearchHitDto {
    fn from(hit: SearchHit) -> Self {
        let record = match hit.record {
            SearchRecord::Span(row) => SearchRecordDto::Span {
                trace_id: row.trace_id,
                span_id: row.span_id,
                timestamp: row.timestamp,
                span_name: row.span_name,
                input_preview: row.input_preview,
                output_preview: row.output_preview,
            },
            SearchRecord::Log(row) => SearchRecordDto::Log {
                log_digest: row.log_digest,
                ordinal: row.ordinal,
                timestamp: row.timestamp,
                severity_text: row.severity_text,
                body_text: row.body_text,
                trace_id: row.trace_id,
                span_id: row.span_id,
            },
        };
        Self {
            indeterminate: hit.indeterminate,
            fragments: hit.fragments,
            record,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResponse {
    pub data: Vec<SearchHitDto>,
    /// The last candidate examined, even when `data` is empty.
    pub next_cursor: Option<String>,
    pub examined: u32,
    pub examination_limit_reached: bool,
    /// Writes after the first page began were observed; pagination is not snapshot-isolated.
    pub arrivals_detected: bool,
    pub index_lag_us: u64,
    #[serde(skip_serializing_if = "is_true")]
    pub search_indexing_complete: bool,
}

fn is_true(value: &bool) -> bool {
    *value
}

#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/search",
    tag = "search",
    params(
        ("project_id" = String, Path, description = "Project identifier"),
        SearchQuery
    ),
    responses((status = 200, description = "Chronological search results", body = SearchResponse))
)]
pub async fn search(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<SearchQuery>,
) -> Result<Json<SearchResponse>, ApiError> {
    let expression = parse(&query.q, query.signal)
        .map_err(|error| ApiError::bad_request("INVALID_SEARCH_QUERY", error.to_string()))?;
    let request = PortSearchQuery {
        project_id: auth.project_id,
        signal: query.signal,
        expression,
        limit: query.limit,
        max_examined: query.max_examined,
        cursor: query.cursor.as_deref().map(decode_cursor).transpose()?,
        from_timestamp: parse_timestamp_param(&query.from_timestamp)?,
        to_timestamp: parse_timestamp_param(&query.to_timestamp)?,
    };
    let page = SearchService::execute(state.analytics.as_ref(), &request)
        .await
        .map_err(ApiError::from_data)?;
    Ok(Json(SearchResponse {
        data: page.hits.into_iter().map(Into::into).collect(),
        next_cursor: page.next_cursor.as_ref().map(encode_cursor).transpose()?,
        examined: page.examined,
        examination_limit_reached: page.examination_limit_reached,
        arrivals_detected: page.arrivals_detected,
        index_lag_us: page.index_lag_us,
        search_indexing_complete: page.search_indexing_complete,
    }))
}

fn encode_cursor(cursor: &SearchCursor) -> Result<String, ApiError> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|error| ApiError::internal(error.to_string()))
}

fn decode_cursor(cursor: &str) -> Result<SearchCursor, ApiError> {
    URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| ApiError::bad_request("INVALID_CURSOR", "Search cursor is not valid base64"))
        .and_then(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::bad_request("INVALID_CURSOR", "Search cursor is malformed"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_total_order_and_start_time() {
        let cursor = SearchCursor {
            timestamp_us: 42,
            tie_breaker: "trace\0span".into(),
            ordinal: 7,
            started_at_us: 11,
        };
        assert_eq!(
            decode_cursor(&encode_cursor(&cursor).unwrap()).unwrap(),
            cursor
        );
    }
}
