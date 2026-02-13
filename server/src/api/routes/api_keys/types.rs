//! API Keys API types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::data::types::{ApiKeyRow, ApiKeyScope};

/// Default scope for API keys
fn default_scope() -> ApiKeyScope {
    ApiKeyScope::Full
}

/// Request body for creating an API key
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateApiKeyRequest {
    /// Name of the key (1-100 characters)
    #[validate(length(min = 1, max = 100, message = "Name must be 1-100 characters"))]
    pub name: String,

    /// Permission scope: read, ingest, write, full (default: full)
    #[serde(default = "default_scope")]
    pub scope: ApiKeyScope,

    /// Optional expiration timestamp (Unix seconds)
    pub expires_at: Option<i64>,
}

/// Response when creating an API key (includes full key - shown only once!)
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateApiKeyResponse {
    pub id: String,
    pub name: String,
    /// Full API key - SHOWN ONLY ONCE
    pub key: String,
    pub key_prefix: String,
    pub scope: ApiKeyScope,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// API key DTO for list responses (no full key)
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiKeyDto {
    pub id: String,
    pub name: String,
    pub key_prefix: String,
    pub scope: ApiKeyScope,
    pub created_by: Option<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl From<ApiKeyRow> for ApiKeyDto {
    fn from(row: ApiKeyRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            key_prefix: row.key_prefix,
            scope: row.scope,
            created_by: row.created_by,
            last_used_at: row
                .last_used_at
                .and_then(|ts| DateTime::from_timestamp(ts, 0)),
            created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or_else(Utc::now),
            expires_at: row
                .expires_at
                .and_then(|ts| DateTime::from_timestamp(ts, 0)),
        }
    }
}
