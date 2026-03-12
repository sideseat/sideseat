//! Credentials API endpoints
//!
//! Endpoints for managing LLM provider credentials at the organization level:
//! - GET    /api/v1/organizations/{org_id}/credentials
//! - POST   /api/v1/organizations/{org_id}/credentials
//! - PATCH  /api/v1/organizations/{org_id}/credentials/{id}
//! - DELETE /api/v1/organizations/{org_id}/credentials/{id}
//! - POST   /api/v1/organizations/{org_id}/credentials/{id}/test
//! - GET    /api/v1/organizations/{org_id}/credentials/{id}/permissions
//! - POST   /api/v1/organizations/{org_id}/credentials/{id}/permissions
//! - DELETE /api/v1/organizations/{org_id}/credentials/{id}/permissions/{perm_id}

pub mod types;

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::api::auth::{OrgAdmin, OrgRead};
use crate::api::extractors::ValidatedJson;
use crate::api::types::ApiError;
use crate::domain::providers::{CredentialError, CredentialService};

use types::{
    CreateCredentialRequest, CreatePermissionRequest, CredentialDto, CredentialPermissionDto,
    TestResultDto, UpdateCredentialRequest,
};

/// Path parameters for credential-specific routes
#[derive(Deserialize)]
pub struct CredentialIdPath {
    pub id: String,
}

/// Path parameters for permission-specific routes
#[derive(Deserialize)]
pub struct PermissionIdPath {
    pub id: String,
    pub perm_id: String,
}

/// Query parameters for list credentials
#[derive(Deserialize)]
pub struct ListCredentialsQuery {
    pub project_id: Option<String>,
}

/// Shared state for Credentials endpoints
#[derive(Clone)]
pub struct CredentialsState {
    pub service: Arc<CredentialService>,
}

/// Build Credentials routes
pub fn routes(service: Arc<CredentialService>) -> Router<()> {
    let state = CredentialsState { service };

    Router::new()
        .route("/", get(list_credentials).post(create_credential))
        .route("/{id}", patch(update_credential).delete(delete_credential))
        .route("/{id}/test", post(test_credential))
        .route(
            "/{id}/permissions",
            get(list_permissions).post(create_permission),
        )
        .route("/{id}/permissions/{perm_id}", delete(delete_permission))
        .with_state(state)
}

fn credential_error(e: CredentialError) -> ApiError {
    match e {
        CredentialError::NotFound => {
            ApiError::not_found("CREDENTIAL_NOT_FOUND", "Credential not found")
        }
        CredentialError::ProjectNotFound => ApiError::not_found(
            "PROJECT_NOT_FOUND",
            "Project not found or does not belong to organization",
        ),
        CredentialError::InvalidProvider(key) => {
            ApiError::bad_request("INVALID_PROVIDER", format!("Unknown provider: {key}"))
        }
        CredentialError::Secret(msg) => ApiError::internal(format!("Secret store error: {msg}")),
        CredentialError::Data(e) => ApiError::from_data(e),
    }
}

/// List credentials for an organization
#[utoipa::path(
    get,
    path = "/api/v1/organizations/{org_id}/credentials",
    tag = "credentials",
    params(
        ("org_id" = String, Path, description = "Organization ID"),
        ("project_id" = Option<String>, Query, description = "Filter by project access"),
    ),
    responses(
        (status = 200, body = Vec<CredentialDto>, description = "List of credentials"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
    ),
    security(("session" = [])),
)]
pub async fn list_credentials(
    State(state): State<CredentialsState>,
    org: OrgRead,
    Query(query): Query<ListCredentialsQuery>,
) -> Result<Json<Vec<CredentialDto>>, ApiError> {
    let creds = state
        .service
        .list_for_org(&org.org_id, query.project_id.as_deref())
        .await
        .map_err(credential_error)?;

    Ok(Json(creds.into_iter().map(CredentialDto::from).collect()))
}

/// Create a new credential
#[utoipa::path(
    post,
    path = "/api/v1/organizations/{org_id}/credentials",
    tag = "credentials",
    request_body = CreateCredentialRequest,
    responses(
        (status = 201, body = CredentialDto, description = "Credential created"),
        (status = 400, description = "Invalid request or unknown provider"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
    ),
    security(("session" = [])),
)]
pub async fn create_credential(
    State(state): State<CredentialsState>,
    org: OrgAdmin,
    ValidatedJson(req): ValidatedJson<CreateCredentialRequest>,
) -> Result<(StatusCode, Json<CredentialDto>), ApiError> {
    let user_id = org.auth.user_id();

    // Compute key_preview from secret_value (first 8 chars)
    let key_preview = req
        .secret_value
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(8).collect::<String>());

    let extra_config_str = req.extra_config.as_ref().map(|v| v.to_string());

    let row = state
        .service
        .create(
            &org.org_id,
            &req.provider_key,
            &req.display_name,
            req.secret_value.as_deref(),
            req.endpoint_url.as_deref(),
            extra_config_str.as_deref(),
            key_preview.as_deref(),
            user_id,
        )
        .await
        .map_err(credential_error)?;

    tracing::debug!(
        credential_id = %row.id,
        provider_key = %row.provider_key,
        org_id = %row.organization_id,
        "Credential created"
    );

    Ok((StatusCode::CREATED, Json(CredentialDto::from(row))))
}

/// Update credential metadata
#[utoipa::path(
    patch,
    path = "/api/v1/organizations/{org_id}/credentials/{id}",
    tag = "credentials",
    request_body = UpdateCredentialRequest,
    responses(
        (status = 200, body = CredentialDto, description = "Credential updated"),
        (status = 400, description = "Cannot modify a read-only credential"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Credential not found"),
    ),
    security(("session" = [])),
)]
pub async fn update_credential(
    State(state): State<CredentialsState>,
    org: OrgAdmin,
    Path(path): Path<CredentialIdPath>,
    ValidatedJson(req): ValidatedJson<UpdateCredentialRequest>,
) -> Result<Json<CredentialDto>, ApiError> {
    if path.id.starts_with("env:") || path.id.starts_with("ambient:") {
        return Err(ApiError::bad_request(
            "READ_ONLY_CREDENTIAL",
            "Cannot modify a read-only credential",
        ));
    }

    let extra_config_str = req
        .extra_config
        .as_ref()
        .map(|opt| opt.as_ref().map(|v| v.to_string()));

    let updated = state
        .service
        .update(
            &path.id,
            &org.org_id,
            req.display_name.as_deref(),
            req.endpoint_url.as_ref().map(|opt| opt.as_deref()),
            extra_config_str.as_ref().map(|opt| opt.as_deref()),
        )
        .await
        .map_err(credential_error)?
        .ok_or_else(|| ApiError::not_found("CREDENTIAL_NOT_FOUND", "Credential not found"))?;

    Ok(Json(CredentialDto::from(updated)))
}

/// Delete a credential
#[utoipa::path(
    delete,
    path = "/api/v1/organizations/{org_id}/credentials/{id}",
    tag = "credentials",
    responses(
        (status = 204, description = "Credential deleted"),
        (status = 400, description = "Cannot modify a read-only credential"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Credential not found"),
    ),
    security(("session" = [])),
)]
pub async fn delete_credential(
    State(state): State<CredentialsState>,
    org: OrgAdmin,
    Path(path): Path<CredentialIdPath>,
) -> Result<StatusCode, ApiError> {
    if path.id.starts_with("env:") || path.id.starts_with("ambient:") {
        return Err(ApiError::bad_request(
            "READ_ONLY_CREDENTIAL",
            "Cannot modify a read-only credential",
        ));
    }

    let deleted = state
        .service
        .delete(&path.id, &org.org_id)
        .await
        .map_err(credential_error)?;

    if !deleted {
        return Err(ApiError::not_found(
            "CREDENTIAL_NOT_FOUND",
            "Credential not found",
        ));
    }

    tracing::debug!(
        credential_id = %path.id,
        org_id = %org.org_id,
        "Credential deleted"
    );

    Ok(StatusCode::NO_CONTENT)
}

/// Test a credential's connectivity
#[utoipa::path(
    post,
    path = "/api/v1/organizations/{org_id}/credentials/{id}/test",
    tag = "credentials",
    responses(
        (status = 200, body = TestResultDto, description = "Test result"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Credential not found"),
    ),
    security(("session" = [])),
)]
pub async fn test_credential(
    State(state): State<CredentialsState>,
    org: OrgRead,
    Path(path): Path<CredentialIdPath>,
) -> Result<Json<TestResultDto>, ApiError> {
    let result = state
        .service
        .test(&org.org_id, &path.id)
        .await
        .map_err(credential_error)?;

    Ok(Json(TestResultDto {
        success: result.success,
        latency_ms: result.latency_ms,
        error: result.error,
        model_hint: result.model_hint,
    }))
}

/// List permissions for a credential
#[utoipa::path(
    get,
    path = "/api/v1/organizations/{org_id}/credentials/{id}/permissions",
    tag = "credentials",
    responses(
        (status = 200, body = Vec<CredentialPermissionDto>, description = "List of permissions"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Credential not found"),
    ),
    security(("session" = [])),
)]
pub async fn list_permissions(
    State(state): State<CredentialsState>,
    org: OrgRead,
    Path(path): Path<CredentialIdPath>,
) -> Result<Json<Vec<CredentialPermissionDto>>, ApiError> {
    if path.id.starts_with("env:") || path.id.starts_with("ambient:") {
        return Err(ApiError::bad_request(
            "READ_ONLY_CREDENTIAL",
            "Cannot manage permissions for a read-only credential",
        ));
    }

    let perms = state
        .service
        .list_permissions(&path.id, &org.org_id)
        .await
        .map_err(credential_error)?;

    Ok(Json(
        perms
            .into_iter()
            .map(CredentialPermissionDto::from)
            .collect(),
    ))
}

/// Create a permission rule for a credential
#[utoipa::path(
    post,
    path = "/api/v1/organizations/{org_id}/credentials/{id}/permissions",
    tag = "credentials",
    request_body = CreatePermissionRequest,
    responses(
        (status = 201, body = CredentialPermissionDto, description = "Permission created"),
        (status = 400, description = "Invalid access value"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Credential or project not found"),
    ),
    security(("session" = [])),
)]
pub async fn create_permission(
    State(state): State<CredentialsState>,
    org: OrgAdmin,
    Path(path): Path<CredentialIdPath>,
    ValidatedJson(req): ValidatedJson<CreatePermissionRequest>,
) -> Result<(StatusCode, Json<CredentialPermissionDto>), ApiError> {
    if path.id.starts_with("env:") || path.id.starts_with("ambient:") {
        return Err(ApiError::bad_request(
            "READ_ONLY_CREDENTIAL",
            "Cannot manage permissions for a read-only credential",
        ));
    }

    let user_id = org.auth.user_id();
    let id = cuid2::create_id();

    let perm = state
        .service
        .create_permission(
            &id,
            &path.id,
            &org.org_id,
            req.project_id.as_deref(),
            &req.access,
            user_id,
        )
        .await
        .map_err(credential_error)?;

    Ok((
        StatusCode::CREATED,
        Json(CredentialPermissionDto::from(perm)),
    ))
}

/// Delete a permission rule
#[utoipa::path(
    delete,
    path = "/api/v1/organizations/{org_id}/credentials/{id}/permissions/{perm_id}",
    tag = "credentials",
    responses(
        (status = 204, description = "Permission deleted"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Credential or permission not found"),
    ),
    security(("session" = [])),
)]
pub async fn delete_permission(
    State(state): State<CredentialsState>,
    org: OrgAdmin,
    Path(path): Path<PermissionIdPath>,
) -> Result<StatusCode, ApiError> {
    if path.id.starts_with("env:") || path.id.starts_with("ambient:") {
        return Err(ApiError::bad_request(
            "READ_ONLY_CREDENTIAL",
            "Cannot manage permissions for a read-only credential",
        ));
    }

    let deleted = state
        .service
        .delete_permission(&path.perm_id, &path.id, &org.org_id)
        .await
        .map_err(credential_error)?;

    if !deleted {
        return Err(ApiError::not_found(
            "PERMISSION_NOT_FOUND",
            format!("Permission not found: {}", path.perm_id),
        ));
    }

    Ok(StatusCode::NO_CONTENT)
}
