//! Shared API types
//!
//! Common types used across all API endpoints including error handling,
//! pagination, and sorting.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use validator::ValidationError;

/// Parse an optional timestamp string parameter (RFC 3339 / ISO 8601 format)
pub fn parse_timestamp_param(s: &Option<String>) -> Result<Option<DateTime<Utc>>, ApiError> {
    match s {
        Some(ts) => DateTime::parse_from_rfc3339(ts)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|_| {
                ApiError::bad_request(
                    "INVALID_TIMESTAMP",
                    format!("Invalid timestamp format: {}. Use ISO 8601 format.", ts),
                )
            }),
        None => Ok(None),
    }
}

/// Maximum items per page for paginated endpoints
pub const MAX_PAGE_LIMIT: u32 = 500;
/// Maximum page number to prevent expensive OFFSET queries
pub const MAX_PAGE: u32 = 100;
/// Default page number
pub const DEFAULT_PAGE: u32 = 1;
/// Default items per page
pub const DEFAULT_LIMIT: u32 = 50;
/// Maximum delete batch size
pub const MAX_DELETE_BATCH: usize = 100;
/// Maximum ID length
pub const MAX_ID_LENGTH: usize = 256;

/// Validator function for page parameter
pub fn validate_page(page: u32) -> Result<(), ValidationError> {
    if page < 1 {
        return Err(ValidationError::new("page_min").with_message("Page must be >= 1".into()));
    }
    if page > MAX_PAGE {
        return Err(ValidationError::new("page_max").with_message(
            format!("Page must be <= {} to prevent expensive queries", MAX_PAGE).into(),
        ));
    }
    Ok(())
}

/// Validator function for limit parameter
pub fn validate_limit(limit: u32) -> Result<(), ValidationError> {
    if limit == 0 || limit > MAX_PAGE_LIMIT {
        return Err(ValidationError::new("limit_range")
            .with_message(format!("Limit must be between 1 and {}", MAX_PAGE_LIMIT).into()));
    }
    Ok(())
}

/// Validator function for delete ID lists
pub fn validate_ids_batch<T: AsRef<[String]>>(ids: T) -> Result<(), ValidationError> {
    let ids = ids.as_ref();
    if ids.is_empty() {
        return Err(ValidationError::new("ids_empty").with_message("IDs cannot be empty".into()));
    }
    if ids.len() > MAX_DELETE_BATCH {
        return Err(ValidationError::new("ids_too_many").with_message(
            format!("Cannot process more than {} IDs at once", MAX_DELETE_BATCH).into(),
        ));
    }
    for id in ids {
        if id.len() > MAX_ID_LENGTH {
            return Err(ValidationError::new("id_too_long")
                .with_message(format!("ID too long (max {} chars)", MAX_ID_LENGTH).into()));
        }
    }
    Ok(())
}

/// Standard API error response
#[derive(Debug)]
pub enum ApiError {
    BadRequest { code: String, message: String },
    NotFound { code: String, message: String },
    Unauthorized { code: String, message: String },
    Forbidden { code: String, message: String },
    Conflict { code: String, message: String },
    ServiceUnavailable { message: String },
    Internal { message: String },
}

impl ApiError {
    pub fn bad_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::BadRequest {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn not_found(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::NotFound {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn forbidden(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Forbidden {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn conflict(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Conflict {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::ServiceUnavailable {
            message: message.into(),
        }
    }

    pub fn from_duckdb(e: crate::data::duckdb::DuckdbError) -> Self {
        tracing::error!(error = %e, "DuckDB error");
        Self::Internal {
            message: "Database operation failed".to_string(),
        }
    }

    pub fn from_sqlite(e: crate::data::sqlite::SqliteError) -> Self {
        tracing::error!(error = %e, "SQLite error");
        Self::Internal {
            message: "Database operation failed".to_string(),
        }
    }

    pub fn from_data(e: crate::data::DataError) -> Self {
        tracing::error!(error = %e, "Data error");
        Self::Internal {
            message: "Database operation failed".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_type, code, message) = match self {
            Self::BadRequest { code, message } => {
                (StatusCode::BAD_REQUEST, "bad_request", code, message)
            }
            Self::NotFound { code, message } => (StatusCode::NOT_FOUND, "not_found", code, message),
            Self::Unauthorized { code, message } => {
                (StatusCode::UNAUTHORIZED, "unauthorized", code, message)
            }
            Self::Forbidden { code, message } => {
                (StatusCode::FORBIDDEN, "forbidden", code, message)
            }
            Self::Conflict { code, message } => (StatusCode::CONFLICT, "conflict", code, message),
            Self::ServiceUnavailable { message } => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                "SERVICE_UNAVAILABLE".to_string(),
                message,
            ),
            Self::Internal { message } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "INTERNAL".to_string(),
                message,
            ),
        };
        (
            status,
            Json(serde_json::json!({
                "error": error_type,
                "code": code,
                "message": message
            })),
        )
            .into_response()
    }
}

pub fn default_page() -> u32 {
    DEFAULT_PAGE
}

pub fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

/// Pagination metadata in response
#[derive(Debug, Serialize, ToSchema)]
pub struct PaginationMeta {
    pub page: u32,
    pub limit: u32,
    pub total_items: u64,
    pub total_pages: u64,
}

impl PaginationMeta {
    pub fn new(page: u32, limit: u32, total_items: u64) -> Self {
        Self {
            page,
            limit,
            total_items,
            total_pages: total_items.div_ceil(limit as u64),
        }
    }
}

/// Generic paginated response wrapper
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T> {
    pub data: Vec<T>,
    pub meta: PaginationMeta,
}

impl<T> PaginatedResponse<T> {
    pub fn new(data: Vec<T>, page: u32, limit: u32, total_items: u64) -> Self {
        Self {
            data,
            meta: PaginationMeta::new(page, limit, total_items),
        }
    }
}

/// OrderBy query parameter parsing
#[derive(Debug, Clone)]
pub struct OrderBy {
    pub column: String,
    pub direction: OrderDirection,
}

#[derive(Debug, Clone, Copy, Default, Serialize, ToSchema)]
pub enum OrderDirection {
    #[default]
    Desc,
    Asc,
}

impl OrderBy {
    pub fn parse(s: &str, allowed_columns: &[&str]) -> Result<Self, ApiError> {
        let parts: Vec<&str> = s.split(':').collect();
        let (column, direction) = match parts.as_slice() {
            [col] => (*col, OrderDirection::Desc),
            [col, "asc"] => (*col, OrderDirection::Asc),
            [col, "desc"] => (*col, OrderDirection::Desc),
            _ => {
                return Err(ApiError::bad_request(
                    "INVALID_ORDER",
                    "Invalid order_by format. Use 'column' or 'column:asc' or 'column:desc'",
                ));
            }
        };
        if !allowed_columns.contains(&column) {
            return Err(ApiError::bad_request(
                "INVALID_ORDER_COLUMN",
                format!("Cannot order by: {}", column),
            ));
        }
        Ok(Self {
            column: column.to_string(),
            direction,
        })
    }

    pub fn to_sql(&self) -> String {
        let dir = match self.direction {
            OrderDirection::Asc => "ASC",
            OrderDirection::Desc => "DESC",
        };
        format!("{} {}", self.column, dir)
    }

    /// Generate SQL with column name mapping (e.g., API aliases to DB columns)
    pub fn to_sql_mapped<F>(&self, mapper: F) -> String
    where
        F: Fn(&str) -> &str,
    {
        let dir = match self.direction {
            OrderDirection::Asc => "ASC",
            OrderDirection::Desc => "DESC",
        };
        format!("{} {}", mapper(&self.column), dir)
    }
}
