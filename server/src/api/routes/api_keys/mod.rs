//! API Keys API endpoints
//!
//! Endpoints for managing API keys at the organization level:
//! - POST /api/v1/organizations/{org_id}/api-keys - Create key (returns plaintext once)
//! - GET /api/v1/organizations/{org_id}/api-keys - List keys (metadata only)
//! - DELETE /api/v1/organizations/{org_id}/api-keys/{id} - Delete key

pub mod types;

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::api::auth::OrgFull;
use crate::api::extractors::ValidatedJson;
use crate::api::types::ApiError;
use crate::data::TransactionalService;
use crate::data::cache::CacheService;
use crate::utils::api_key::{generate_api_key, hash_api_key, key_prefix};

use types::{ApiKeyDto, CreateApiKeyRequest, CreateApiKeyResponse};

/// Path parameters for key-specific routes
#[derive(Deserialize)]
pub struct KeyIdPath {
    pub id: String,
}

/// Shared state for API Keys endpoints
#[derive(Clone)]
pub struct ApiKeysState {
    pub database: Arc<TransactionalService>,
    pub cache: Arc<CacheService>,
    pub api_key_secret: Vec<u8>,
}

/// Build API Keys routes
pub fn routes(
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
    api_key_secret: Vec<u8>,
) -> Router<()> {
    let state = ApiKeysState {
        database,
        cache,
        api_key_secret,
    };

    Router::new()
        .route("/", get(list_api_keys).post(create_api_key))
        .route("/{id}", axum::routing::delete(delete_api_key))
        .with_state(state)
}

/// Create a new API key
///
/// Returns the full plaintext key - shown only once!
#[utoipa::path(
    post,
    path = "/api/v1/organizations/{org_id}/api-keys",
    tag = "api-keys",
    request_body = CreateApiKeyRequest,
    responses(
        (status = 201, body = CreateApiKeyResponse, description = "Key created"),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
        (status = 409, description = "Key limit (100) reached"),
    ),
    security(("session" = [])),
)]
pub async fn create_api_key(
    State(state): State<ApiKeysState>,
    org: OrgFull,
    ValidatedJson(req): ValidatedJson<CreateApiKeyRequest>,
) -> Result<(StatusCode, Json<CreateApiKeyResponse>), ApiError> {
    let user_id = org.auth.require_user_id()?;
    let repo = state.database.repository();

    // Generate key and hash
    let key = generate_api_key();
    let key_hash = hash_api_key(&key, &state.api_key_secret);
    let prefix = key_prefix(&key);

    // Create in DB (checks limit internally)
    let row = repo
        .create_api_key(
            Some(&state.cache),
            &org.org_id,
            &req.name,
            &key_hash,
            &prefix,
            req.scope,
            user_id,
            req.expires_at,
        )
        .await
        .map_err(|e| {
            if let crate::data::error::DataError::Conflict(msg) = &e
                && msg.contains("Maximum")
            {
                return ApiError::conflict(
                    "KEY_LIMIT_REACHED",
                    "Maximum 100 keys per organization",
                );
            }
            ApiError::from_data(e)
        })?;

    tracing::debug!(
        key_id = %row.id,
        key_prefix = %row.key_prefix,
        org_id = %row.org_id,
        user_id = %user_id,
        "API key created"
    );

    // Return response with plaintext key (shown only once!)
    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse {
            id: row.id,
            name: row.name,
            key, // Plaintext - never stored, never shown again
            key_prefix: row.key_prefix,
            scope: row.scope,
            created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or_else(Utc::now),
            expires_at: row
                .expires_at
                .and_then(|ts| DateTime::from_timestamp(ts, 0)),
        }),
    ))
}

/// List all API keys for an organization (metadata only, no full keys)
#[utoipa::path(
    get,
    path = "/api/v1/organizations/{org_id}/api-keys",
    tag = "api-keys",
    responses(
        (status = 200, body = Vec<ApiKeyDto>, description = "List of API keys"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Organization not found"),
    ),
    security(("session" = [])),
)]
pub async fn list_api_keys(
    State(state): State<ApiKeysState>,
    org: OrgFull,
) -> Result<Json<Vec<ApiKeyDto>>, ApiError> {
    let repo = state.database.repository();

    let keys = repo
        .list_api_keys(Some(&state.cache), &org.org_id)
        .await
        .map_err(ApiError::from_data)?;

    let dtos: Vec<ApiKeyDto> = keys.into_iter().map(ApiKeyDto::from).collect();

    Ok(Json(dtos))
}

/// Delete an API key
#[utoipa::path(
    delete,
    path = "/api/v1/organizations/{org_id}/api-keys/{id}",
    tag = "api-keys",
    responses(
        (status = 204, description = "Key deleted"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Key not found"),
    ),
    security(("session" = [])),
)]
pub async fn delete_api_key(
    State(state): State<ApiKeysState>,
    org: OrgFull,
    Path(path): Path<KeyIdPath>,
) -> Result<StatusCode, ApiError> {
    let repo = state.database.repository();

    let deleted = repo
        .delete_api_key(Some(&state.cache), &path.id, &org.org_id)
        .await
        .map_err(ApiError::from_data)?;

    if !deleted {
        return Err(ApiError::not_found(
            "KEY_NOT_FOUND",
            format!("API key not found: {}", path.id),
        ));
    }

    if let Some(user_id) = org.auth.user_id() {
        tracing::debug!(
            key_id = %path.id,
            org_id = %org.org_id,
            user_id = %user_id,
            "API key deleted"
        );
    }

    Ok(StatusCode::NO_CONTENT)
}
