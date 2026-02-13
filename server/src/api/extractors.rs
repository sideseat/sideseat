//! Path and validation extractors for API routes
//!
//! ## HTTP Caching Strategy
//!
//! | Endpoint Type           | Cache-Control          | Additional      |
//! |-------------------------|------------------------|-----------------|
//! | OTEL list endpoints     | `no-store`             | Last-Modified   |
//! | OTEL detail endpoints   | -                      | ETag (computed) |
//! | Filter options          | `private, max-age=30`  | -               |
//! | Resource APIs           | -                      | -               |
//! | SSE                     | N/A                    | -               |

use std::ops::Deref;

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use validator::Validate;

/// Raw path extractor for project-scoped routes (internal use)
#[derive(Debug, Deserialize)]
struct ProjectPathRaw {
    project_id: String,
}

/// Validated project path extractor.
///
/// Extracts and validates `project_id` from URL path parameters.
/// Returns a 400 Bad Request if validation fails.
#[derive(Debug)]
pub struct ProjectPath {
    pub project_id: String,
}

/// Maximum length for IDs (trace_id, span_id, session_id, etc.)
pub const MAX_ID_LENGTH: usize = 256;

/// Validate project_id: 1-64 chars, alphanumeric + dash/underscore
pub fn is_valid_project_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Validate generic ID length (trace_id, span_id, session_id, etc.)
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LENGTH
}

impl<S> FromRequestParts<S> for ProjectPath
where
    S: Send + Sync,
{
    type Rejection = ValidationRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(raw) = Path::<ProjectPathRaw>::from_request_parts(parts, state)
            .await
            .map_err(ValidationRejection::Path)?;

        if !is_valid_project_id(&raw.project_id) {
            return Err(ValidationRejection::InvalidProjectId);
        }

        Ok(Self {
            project_id: raw.project_id,
        })
    }
}

// ============================================================================
// Compound Path Extractors
// ============================================================================

/// Raw path extractor for trace routes (internal use)
#[derive(Debug, Deserialize)]
struct TracePathRaw {
    project_id: String,
    trace_id: String,
}

/// Validated trace path extractor.
///
/// Extracts and validates `project_id` and `trace_id` from URL path parameters.
/// Returns a 400 Bad Request if validation fails.
#[derive(Debug)]
pub struct TracePath {
    pub project_id: String,
    pub trace_id: String,
}

impl<S> FromRequestParts<S> for TracePath
where
    S: Send + Sync,
{
    type Rejection = ValidationRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(raw) = Path::<TracePathRaw>::from_request_parts(parts, state)
            .await
            .map_err(ValidationRejection::Path)?;

        if !is_valid_project_id(&raw.project_id) {
            return Err(ValidationRejection::InvalidProjectId);
        }
        if !is_valid_id(&raw.trace_id) {
            return Err(ValidationRejection::InvalidTraceId);
        }

        Ok(Self {
            project_id: raw.project_id,
            trace_id: raw.trace_id,
        })
    }
}

/// Raw path extractor for span routes (internal use)
#[derive(Debug, Deserialize)]
struct SpanPathRaw {
    project_id: String,
    trace_id: String,
    span_id: String,
}

/// Validated span path extractor.
///
/// Extracts and validates `project_id`, `trace_id`, and `span_id` from URL path parameters.
/// Returns a 400 Bad Request if validation fails.
#[derive(Debug)]
pub struct SpanPath {
    pub project_id: String,
    pub trace_id: String,
    pub span_id: String,
}

impl<S> FromRequestParts<S> for SpanPath
where
    S: Send + Sync,
{
    type Rejection = ValidationRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(raw) = Path::<SpanPathRaw>::from_request_parts(parts, state)
            .await
            .map_err(ValidationRejection::Path)?;

        if !is_valid_project_id(&raw.project_id) {
            return Err(ValidationRejection::InvalidProjectId);
        }
        if !is_valid_id(&raw.trace_id) {
            return Err(ValidationRejection::InvalidTraceId);
        }
        if !is_valid_id(&raw.span_id) {
            return Err(ValidationRejection::InvalidSpanId);
        }

        Ok(Self {
            project_id: raw.project_id,
            trace_id: raw.trace_id,
            span_id: raw.span_id,
        })
    }
}

/// Raw path extractor for session routes (internal use)
#[derive(Debug, Deserialize)]
struct SessionPathRaw {
    project_id: String,
    session_id: String,
}

/// Validated session path extractor.
///
/// Extracts and validates `project_id` and `session_id` from URL path parameters.
/// Returns a 400 Bad Request if validation fails.
#[derive(Debug)]
pub struct SessionPath {
    pub project_id: String,
    pub session_id: String,
}

impl<S> FromRequestParts<S> for SessionPath
where
    S: Send + Sync,
{
    type Rejection = ValidationRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(raw) = Path::<SessionPathRaw>::from_request_parts(parts, state)
            .await
            .map_err(ValidationRejection::Path)?;

        if !is_valid_project_id(&raw.project_id) {
            return Err(ValidationRejection::InvalidProjectId);
        }
        if !is_valid_id(&raw.session_id) {
            return Err(ValidationRejection::InvalidSessionId);
        }

        Ok(Self {
            project_id: raw.project_id,
            session_id: raw.session_id,
        })
    }
}

/// Validation rejection with structured error response
pub enum ValidationRejection {
    /// Failed to parse path parameters
    Path(PathRejection),
    /// Invalid project_id format
    InvalidProjectId,
    /// Invalid org_id format
    InvalidOrgId,
    /// Invalid trace_id format
    InvalidTraceId,
    /// Invalid span_id format
    InvalidSpanId,
    /// Invalid session_id format
    InvalidSessionId,
    /// Failed to parse query string
    Query(QueryRejection),
    /// Failed to parse JSON body
    Json(JsonRejection),
    /// Validation constraints not satisfied
    Validation(validator::ValidationErrors),
}

impl IntoResponse for ValidationRejection {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Path(rejection) => (
                StatusCode::BAD_REQUEST,
                "PATH_PARSE_ERROR",
                rejection.body_text(),
            ),
            Self::InvalidProjectId => (
                StatusCode::BAD_REQUEST,
                "INVALID_PROJECT_ID",
                "Invalid project_id: must be 1-64 alphanumeric chars, dashes, or underscores"
                    .to_string(),
            ),
            Self::InvalidOrgId => (
                StatusCode::BAD_REQUEST,
                "INVALID_ORG_ID",
                "Invalid org_id: must be 1-256 characters".to_string(),
            ),
            Self::InvalidTraceId => (
                StatusCode::BAD_REQUEST,
                "INVALID_TRACE_ID",
                "Invalid trace_id: must be 1-256 characters".to_string(),
            ),
            Self::InvalidSpanId => (
                StatusCode::BAD_REQUEST,
                "INVALID_SPAN_ID",
                "Invalid span_id: must be 1-256 characters".to_string(),
            ),
            Self::InvalidSessionId => (
                StatusCode::BAD_REQUEST,
                "INVALID_SESSION_ID",
                "Invalid session_id: must be 1-256 characters".to_string(),
            ),
            Self::Query(rejection) => (
                StatusCode::BAD_REQUEST,
                "QUERY_PARSE_ERROR",
                rejection.body_text(),
            ),
            Self::Json(rejection) => (
                StatusCode::BAD_REQUEST,
                "JSON_PARSE_ERROR",
                rejection.body_text(),
            ),
            Self::Validation(errors) => (
                StatusCode::BAD_REQUEST,
                "VALIDATION_ERROR",
                format_validation_errors(&errors),
            ),
        };
        (
            status,
            Json(serde_json::json!({
                "error": "bad_request",
                "code": code,
                "message": message
            })),
        )
            .into_response()
    }
}

fn format_validation_errors(errors: &validator::ValidationErrors) -> String {
    errors
        .field_errors()
        .iter()
        .flat_map(|(field, errs)| {
            errs.iter().map(move |e| {
                e.message
                    .as_ref()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| format!("{}: validation failed", field))
            })
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Query extractor with automatic validation.
///
/// Deserializes query parameters and validates them using the `validator` crate.
/// Returns a `ValidationRejection` on parse or validation failure.
#[derive(Debug)]
pub struct ValidatedQuery<T>(pub T);

impl<T> Deref for ValidatedQuery<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, T> FromRequestParts<S> for ValidatedQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate,
{
    type Rejection = ValidationRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(ValidationRejection::Query)?;
        value.validate().map_err(ValidationRejection::Validation)?;
        Ok(Self(value))
    }
}

/// JSON body extractor with automatic validation.
///
/// Deserializes JSON body and validates it using the `validator` crate.
/// Returns a `ValidationRejection` on parse or validation failure.
#[derive(Debug)]
pub struct ValidatedJson<T>(pub T);

impl<T> Deref for ValidatedJson<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, T> FromRequest<S> for ValidatedJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate,
{
    type Rejection = ValidationRejection;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state)
            .await
            .map_err(ValidationRejection::Json)?;
        value.validate().map_err(ValidationRejection::Validation)?;
        Ok(Self(value))
    }
}
