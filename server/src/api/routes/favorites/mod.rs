//! Favorites API endpoints

pub mod types;

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};

use types::{
    AddFavoriteResponse, CheckFavoritesRequest, CheckFavoritesResponse, EntityType,
    ListFavoritesResponse, RemoveFavoriteResponse,
};

use crate::api::auth::{ProjectRead, ProjectWrite};
use crate::api::extractors::is_valid_id;
use crate::api::types::ApiError;
use crate::core::constants::MAX_FAVORITES_PER_PROJECT;
use crate::data::TransactionalService;

/// Shared state for Favorites API endpoints
#[derive(Clone)]
pub struct FavoritesApiState {
    pub database: Arc<TransactionalService>,
}

/// Build Favorites API routes
pub fn routes(database: Arc<TransactionalService>) -> Router<()> {
    let state = FavoritesApiState { database };

    Router::new()
        .route("/check", post(check_favorites))
        // List all favorite IDs for an entity type (for "favorites only" filter)
        .route("/list/{entity_type}", get(list_favorites))
        // Simple entity routes (trace/session)
        .route(
            "/{entity_type}/{entity_id}",
            put(add_favorite_simple).delete(remove_favorite_simple),
        )
        // Composite entity routes (span with secondary_id)
        .route(
            "/{entity_type}/{entity_id}/{secondary_id}",
            put(add_favorite_composite).delete(remove_favorite_composite),
        )
        .with_state(state)
}

/// Validate entity type string
fn parse_entity_type(s: &str) -> Result<EntityType, ApiError> {
    match s {
        "trace" => Ok(EntityType::Trace),
        "session" => Ok(EntityType::Session),
        "span" => Ok(EntityType::Span),
        _ => Err(ApiError::bad_request(
            "INVALID_ENTITY_TYPE",
            "entity_type must be one of: trace, session, span",
        )),
    }
}

/// Path parameters for simple entity routes
#[derive(serde::Deserialize)]
pub struct SimpleEntityPath {
    pub entity_type: String,
    pub entity_id: String,
}

/// Path parameters for composite entity routes
#[derive(serde::Deserialize)]
pub struct CompositeEntityPath {
    pub entity_type: String,
    pub entity_id: String,
    pub secondary_id: String,
}

/// Add a favorite (simple entity: trace/session)
#[utoipa::path(
    put,
    path = "/api/v1/project/{project_id}/favorites/{entity_type}/{entity_id}",
    tag = "favorites",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("entity_type" = String, Path, description = "Entity type (trace, session)"),
        ("entity_id" = String, Path, description = "Entity ID")
    ),
    responses(
        (status = 201, description = "Favorite created", body = AddFavoriteResponse),
        (status = 200, description = "Favorite already existed", body = AddFavoriteResponse),
        (status = 403, description = "Access denied")
    )
)]
pub async fn add_favorite_simple(
    State(state): State<FavoritesApiState>,
    project: ProjectWrite,
    Path(path): Path<SimpleEntityPath>,
) -> Result<(StatusCode, Json<AddFavoriteResponse>), ApiError> {
    let user_id = project.auth.require_user_id()?;
    let entity_type = parse_entity_type(&path.entity_type)?;

    // Validate entity_id
    if !is_valid_id(&path.entity_id) {
        return Err(ApiError::bad_request(
            "INVALID_ENTITY_ID",
            "Invalid entity_id: must be 1-256 characters",
        ));
    }

    let repo = state.database.repository();

    // Check soft limit
    let count = repo
        .count_favorites(user_id, &project.project_id)
        .await
        .map_err(ApiError::from_data)?;

    if count as usize >= MAX_FAVORITES_PER_PROJECT {
        return Err(ApiError::bad_request(
            "FAVORITES_LIMIT_REACHED",
            format!(
                "Maximum {} favorites per project reached",
                MAX_FAVORITES_PER_PROJECT
            ),
        ));
    }

    let created = repo
        .add_favorite(
            user_id,
            entity_type.as_str(),
            &path.entity_id,
            None,
            &project.project_id,
        )
        .await
        .map_err(ApiError::from_data)?;

    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };

    Ok((status, Json(AddFavoriteResponse { created })))
}

/// Add a favorite (composite entity: span)
#[utoipa::path(
    put,
    path = "/api/v1/project/{project_id}/favorites/{entity_type}/{entity_id}/{secondary_id}",
    tag = "favorites",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("entity_type" = String, Path, description = "Entity type (span)"),
        ("entity_id" = String, Path, description = "Trace ID"),
        ("secondary_id" = String, Path, description = "Span ID")
    ),
    responses(
        (status = 201, description = "Favorite created", body = AddFavoriteResponse),
        (status = 200, description = "Favorite already existed", body = AddFavoriteResponse),
        (status = 403, description = "Access denied")
    )
)]
pub async fn add_favorite_composite(
    State(state): State<FavoritesApiState>,
    project: ProjectWrite,
    Path(path): Path<CompositeEntityPath>,
) -> Result<(StatusCode, Json<AddFavoriteResponse>), ApiError> {
    let user_id = project.auth.require_user_id()?;
    let entity_type = parse_entity_type(&path.entity_type)?;

    // Composite routes only make sense for spans
    if entity_type != EntityType::Span {
        return Err(ApiError::bad_request(
            "INVALID_ROUTE",
            "Use /{entity_type}/{entity_id} for trace and session favorites",
        ));
    }

    // Validate IDs
    if !is_valid_id(&path.entity_id) {
        return Err(ApiError::bad_request(
            "INVALID_ENTITY_ID",
            "Invalid entity_id (trace_id): must be 1-256 characters",
        ));
    }
    if !is_valid_id(&path.secondary_id) {
        return Err(ApiError::bad_request(
            "INVALID_SECONDARY_ID",
            "Invalid secondary_id (span_id): must be 1-256 characters",
        ));
    }

    let repo = state.database.repository();

    // Check soft limit
    let count = repo
        .count_favorites(user_id, &project.project_id)
        .await
        .map_err(ApiError::from_data)?;

    if count as usize >= MAX_FAVORITES_PER_PROJECT {
        return Err(ApiError::bad_request(
            "FAVORITES_LIMIT_REACHED",
            format!(
                "Maximum {} favorites per project reached",
                MAX_FAVORITES_PER_PROJECT
            ),
        ));
    }

    let created = repo
        .add_favorite(
            user_id,
            entity_type.as_str(),
            &path.entity_id,
            Some(&path.secondary_id),
            &project.project_id,
        )
        .await
        .map_err(ApiError::from_data)?;

    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };

    Ok((status, Json(AddFavoriteResponse { created })))
}

/// Remove a favorite (simple entity: trace/session)
#[utoipa::path(
    delete,
    path = "/api/v1/project/{project_id}/favorites/{entity_type}/{entity_id}",
    tag = "favorites",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("entity_type" = String, Path, description = "Entity type (trace, session)"),
        ("entity_id" = String, Path, description = "Entity ID")
    ),
    responses(
        (status = 200, description = "Favorite removed (idempotent)", body = RemoveFavoriteResponse),
        (status = 403, description = "Access denied")
    )
)]
pub async fn remove_favorite_simple(
    State(state): State<FavoritesApiState>,
    project: ProjectWrite,
    Path(path): Path<SimpleEntityPath>,
) -> Result<Json<RemoveFavoriteResponse>, ApiError> {
    let user_id = project.auth.require_user_id()?;
    let entity_type = parse_entity_type(&path.entity_type)?;

    // Validate entity_id
    if !is_valid_id(&path.entity_id) {
        return Err(ApiError::bad_request(
            "INVALID_ENTITY_ID",
            "Invalid entity_id: must be 1-256 characters",
        ));
    }

    let repo = state.database.repository();

    let removed = repo
        .remove_favorite(
            user_id,
            entity_type.as_str(),
            &path.entity_id,
            None,
            &project.project_id,
        )
        .await
        .map_err(ApiError::from_data)?;

    Ok(Json(RemoveFavoriteResponse { removed }))
}

/// Remove a favorite (composite entity: span)
#[utoipa::path(
    delete,
    path = "/api/v1/project/{project_id}/favorites/{entity_type}/{entity_id}/{secondary_id}",
    tag = "favorites",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("entity_type" = String, Path, description = "Entity type (span)"),
        ("entity_id" = String, Path, description = "Trace ID"),
        ("secondary_id" = String, Path, description = "Span ID")
    ),
    responses(
        (status = 200, description = "Favorite removed (idempotent)", body = RemoveFavoriteResponse),
        (status = 403, description = "Access denied")
    )
)]
pub async fn remove_favorite_composite(
    State(state): State<FavoritesApiState>,
    project: ProjectWrite,
    Path(path): Path<CompositeEntityPath>,
) -> Result<Json<RemoveFavoriteResponse>, ApiError> {
    let user_id = project.auth.require_user_id()?;
    let entity_type = parse_entity_type(&path.entity_type)?;

    // Composite routes only make sense for spans
    if entity_type != EntityType::Span {
        return Err(ApiError::bad_request(
            "INVALID_ROUTE",
            "Use /{entity_type}/{entity_id} for trace and session favorites",
        ));
    }

    // Validate IDs
    if !is_valid_id(&path.entity_id) {
        return Err(ApiError::bad_request(
            "INVALID_ENTITY_ID",
            "Invalid entity_id (trace_id): must be 1-256 characters",
        ));
    }
    if !is_valid_id(&path.secondary_id) {
        return Err(ApiError::bad_request(
            "INVALID_SECONDARY_ID",
            "Invalid secondary_id (span_id): must be 1-256 characters",
        ));
    }

    let repo = state.database.repository();

    let removed = repo
        .remove_favorite(
            user_id,
            entity_type.as_str(),
            &path.entity_id,
            Some(&path.secondary_id),
            &project.project_id,
        )
        .await
        .map_err(ApiError::from_data)?;

    Ok(Json(RemoveFavoriteResponse { removed }))
}

/// Batch check if entities are favorited
#[utoipa::path(
    post,
    path = "/api/v1/project/{project_id}/favorites/check",
    tag = "favorites",
    request_body = CheckFavoritesRequest,
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    responses(
        (status = 200, description = "Check results", body = CheckFavoritesResponse),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Access denied")
    )
)]
pub async fn check_favorites(
    State(state): State<FavoritesApiState>,
    project: ProjectRead,
    Json(body): Json<CheckFavoritesRequest>,
) -> Result<Json<CheckFavoritesResponse>, ApiError> {
    let user_id = project.auth.require_user_id()?;

    // Validate fields based on entity_type
    body.validate()
        .map_err(|msg| ApiError::bad_request("VALIDATION_ERROR", msg))?;

    let repo = state.database.repository();

    let favorites = match body.entity_type {
        EntityType::Trace | EntityType::Session => {
            let ids = body.ids.unwrap_or_default();
            repo.check_favorites(
                user_id,
                body.entity_type.as_str(),
                &ids,
                &project.project_id,
            )
            .await
            .map_err(ApiError::from_data)?
        }
        EntityType::Span => {
            let spans = body.spans.unwrap_or_default();
            let span_pairs: Vec<(String, String)> =
                spans.into_iter().map(|s| (s.trace_id, s.span_id)).collect();
            let result = repo
                .check_span_favorites(user_id, &span_pairs, &project.project_id)
                .await
                .map_err(ApiError::from_data)?;
            // Convert tuples to "trace_id:span_id" format for consistency
            result
                .into_iter()
                .map(|(trace_id, span_id)| format!("{}:{}", trace_id, span_id))
                .collect()
        }
    };

    Ok(Json(CheckFavoritesResponse {
        favorites: favorites.into_iter().collect(),
    }))
}

/// Path parameters for list endpoint
#[derive(serde::Deserialize)]
pub struct ListFavoritesPath {
    pub entity_type: String,
}

/// List all favorite IDs for an entity type (for "favorites only" filter)
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/favorites/list/{entity_type}",
    tag = "favorites",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("entity_type" = String, Path, description = "Entity type (trace, session, span)")
    ),
    responses(
        (status = 200, description = "List of favorite IDs", body = ListFavoritesResponse),
        (status = 403, description = "Access denied")
    )
)]
pub async fn list_favorites(
    State(state): State<FavoritesApiState>,
    project: ProjectRead,
    Path(path): Path<ListFavoritesPath>,
) -> Result<Json<ListFavoritesResponse>, ApiError> {
    let user_id = project.auth.require_user_id()?;
    let entity_type = parse_entity_type(&path.entity_type)?;

    let repo = state.database.repository();

    let favorites = repo
        .list_favorite_ids(user_id, entity_type.as_str(), &project.project_id)
        .await
        .map_err(ApiError::from_data)?;

    Ok(Json(ListFavoritesResponse { favorites }))
}
