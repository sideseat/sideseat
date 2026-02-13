//! User API endpoints

pub mod types;

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::api::auth::Auth;
use crate::api::extractors::ValidatedJson;
use crate::api::types::ApiError;
use crate::core::constants::MAX_USER_ORGS;
use crate::data::TransactionalService;
use crate::data::types::ApiKeyScope;

use types::{UpdateUserRequest, UserDto, UserOrgDto, UserProfileResponse};

/// Shared state for Users API endpoints
#[derive(Clone)]
pub struct UsersApiState {
    pub database: Arc<TransactionalService>,
}

/// Build Users API routes
pub fn routes(database: Arc<TransactionalService>) -> Router<()> {
    let state = UsersApiState { database };

    Router::new()
        .route("/me", get(get_current_user).put(update_current_user))
        .with_state(state)
}

/// Get current user's profile with all their organizations
#[utoipa::path(
    get,
    path = "/api/v1/users/me",
    tag = "users",
    responses(
        (status = 200, description = "User profile with organizations", body = UserProfileResponse),
        (status = 404, description = "User not found")
    )
)]
pub async fn get_current_user(
    State(state): State<UsersApiState>,
    auth: Auth,
) -> Result<Json<UserProfileResponse>, ApiError> {
    auth.ctx.require_scope(ApiKeyScope::Read)?;
    let user_id = auth.require_user_id()?;

    let repo = state.database.repository();

    // Get user info
    let user = repo
        .get_user(None, user_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| ApiError::not_found("USER_NOT_FOUND", "User not found"))?;

    // Get all user's organizations (limited to MAX_USER_ORGS)
    let (orgs, _) = repo
        .list_orgs_for_user(None, user_id, 1, MAX_USER_ORGS)
        .await
        .map_err(ApiError::from_data)?;

    let organizations: Vec<UserOrgDto> = orgs.into_iter().map(UserOrgDto::from).collect();

    Ok(Json(UserProfileResponse {
        user: UserDto::from(user),
        organizations,
    }))
}

/// Update current user's profile
#[utoipa::path(
    put,
    path = "/api/v1/users/me",
    tag = "users",
    request_body = UpdateUserRequest,
    responses(
        (status = 200, description = "User profile updated", body = UserDto),
        (status = 404, description = "User not found")
    )
)]
pub async fn update_current_user(
    State(state): State<UsersApiState>,
    auth: Auth,
    ValidatedJson(body): ValidatedJson<UpdateUserRequest>,
) -> Result<Json<UserDto>, ApiError> {
    auth.ctx.require_scope(ApiKeyScope::Write)?;
    let user_id = auth.require_user_id()?;

    let repo = state.database.repository();

    let user = repo
        .update_user(None, user_id, body.display_name.as_deref())
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| ApiError::not_found("USER_NOT_FOUND", "User not found"))?;

    Ok(Json(UserDto::from(user)))
}
