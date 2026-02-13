//! Project API endpoints

pub mod types;

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use crate::api::auth::{Auth, AuthContext, AuthService, ProjectFull, ProjectRead, ProjectWrite};
use crate::api::extractors::{ValidatedJson, ValidatedQuery};
use crate::api::types::{ApiError, PaginatedResponse};
use crate::core::constants::{DEFAULT_PROJECT_ID, ORG_ROLE_ADMIN, ORG_ROLE_MEMBER};
use crate::data::cache::CacheService;
use crate::data::cleanup::cleanup_project;
use crate::data::files::FileService;
use crate::data::types::ApiKeyScope;
use crate::data::{AnalyticsService, TransactionalService};

use types::{CreateProjectRequest, ListProjectsQuery, ProjectDto, UpdateProjectRequest};

/// Shared state for Projects API endpoints
#[derive(Clone)]
pub struct ProjectsApiState {
    pub database: Arc<TransactionalService>,
    pub analytics: Arc<AnalyticsService>,
    pub file_service: Arc<FileService>,
    pub cache: Arc<CacheService>,
}

/// Build Projects API routes
pub fn routes(
    database: Arc<TransactionalService>,
    analytics: Arc<AnalyticsService>,
    file_service: Arc<FileService>,
    cache: Arc<CacheService>,
) -> Router<()> {
    let state = ProjectsApiState {
        database,
        analytics,
        file_service,
        cache,
    };

    Router::new()
        .route("/", get(list_projects).post(create_project))
        .route(
            "/{project_id}",
            get(get_project).put(update_project).delete(delete_project),
        )
        .with_state(state)
}

/// List projects for the current user
/// - Without org_id: returns all projects across user's orgs
/// - With org_id: returns projects for that specific org (user must be member)
#[utoipa::path(
    get,
    path = "/api/v1/projects",
    tag = "projects",
    params(
        ("page" = Option<u32>, Query, description = "Page number (1-100)"),
        ("limit" = Option<u32>, Query, description = "Items per page (1-100)"),
        ("org_id" = Option<String>, Query, description = "Filter by organization ID")
    ),
    responses(
        (status = 200, description = "List of projects with pagination metadata")
    )
)]
pub async fn list_projects(
    State(state): State<ProjectsApiState>,
    auth: Auth,
    auth_service: axum::Extension<Arc<AuthService>>,
    ValidatedQuery(query): ValidatedQuery<ListProjectsQuery>,
) -> Result<Json<PaginatedResponse<ProjectDto>>, ApiError> {
    let repo = state.database.repository();

    // Security: require Read scope
    auth.ctx.require_scope(ApiKeyScope::Read)?;

    let (projects, total) = match (&query.org_id, &auth.ctx) {
        // Explicit org_id filter: verify access to that org
        (Some(org_id), _) => {
            auth_service
                .verify_org_access(&auth.ctx, org_id, ApiKeyScope::Read)
                .await?;
            repo.list_projects_for_org(None, org_id, query.page, query.limit)
                .await
                .map_err(ApiError::from_data)?
        }
        // API key without org_id filter: list projects in key's org
        (None, AuthContext::ApiKey { org_id, .. }) => repo
            .list_projects_for_org(None, org_id, query.page, query.limit)
            .await
            .map_err(ApiError::from_data)?,
        // Session/Local auth: list all projects across user's orgs
        (None, _) => {
            let user_id = auth.require_user_id()?;
            repo.list_projects_for_user(None, user_id, query.page, query.limit)
                .await
                .map_err(ApiError::from_data)?
        }
    };

    let data: Vec<ProjectDto> = projects.into_iter().map(ProjectDto::from).collect();

    Ok(Json(PaginatedResponse::new(
        data,
        query.page,
        query.limit,
        total,
    )))
}

/// Create a new project (member+ role required in target org)
#[utoipa::path(
    post,
    path = "/api/v1/projects",
    tag = "projects",
    request_body = CreateProjectRequest,
    responses(
        (status = 201, description = "Project created", body = ProjectDto),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Not a member of target organization")
    )
)]
pub async fn create_project(
    State(state): State<ProjectsApiState>,
    auth: Auth,
    auth_service: axum::Extension<Arc<AuthService>>,
    ValidatedJson(body): ValidatedJson<CreateProjectRequest>,
) -> Result<(StatusCode, Json<ProjectDto>), ApiError> {
    let repo = state.database.repository();

    // Verify access to target org with member role and write scope
    auth_service
        .verify_org_role(
            &auth.ctx,
            &body.organization_id,
            ApiKeyScope::Write,
            ORG_ROLE_MEMBER,
        )
        .await?;

    let project = repo
        .create_project(None, &body.organization_id, &body.name)
        .await
        .map_err(ApiError::from_data)?;

    Ok((StatusCode::CREATED, Json(ProjectDto::from(project))))
}

/// Get a single project by ID
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project_id}",
    tag = "projects",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    responses(
        (status = 200, description = "Project details", body = ProjectDto),
        (status = 403, description = "Not a member of project's organization"),
        (status = 404, description = "Project not found")
    )
)]
pub async fn get_project(
    State(state): State<ProjectsApiState>,
    project: ProjectRead,
) -> Result<Json<ProjectDto>, ApiError> {
    let repo = state.database.repository();

    let project_row = repo
        .get_project(None, &project.project_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| {
            ApiError::not_found(
                "PROJECT_NOT_FOUND",
                format!("Project not found: {}", project.project_id),
            )
        })?;

    Ok(Json(ProjectDto::from(project_row)))
}

/// Update a project's name (member+ role required)
#[utoipa::path(
    put,
    path = "/api/v1/projects/{project_id}",
    tag = "projects",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    request_body = UpdateProjectRequest,
    responses(
        (status = 200, description = "Project updated", body = ProjectDto),
        (status = 403, description = "Cannot update default project or insufficient permissions"),
        (status = 404, description = "Project not found")
    )
)]
pub async fn update_project(
    State(state): State<ProjectsApiState>,
    project: ProjectWrite,
    auth_service: axum::Extension<Arc<AuthService>>,
    ValidatedJson(body): ValidatedJson<UpdateProjectRequest>,
) -> Result<Json<ProjectDto>, ApiError> {
    // Cannot update default project
    if project.project_id == DEFAULT_PROJECT_ID {
        return Err(ApiError::forbidden(
            "CANNOT_UPDATE_DEFAULT",
            "The default project cannot be renamed",
        ));
    }

    // Verify member role in project's org
    auth_service
        .verify_org_role(
            &project.auth,
            &project.org_id,
            ApiKeyScope::Write,
            ORG_ROLE_MEMBER,
        )
        .await?;

    let repo = state.database.repository();
    let project_row = repo
        .update_project(None, &project.project_id, &body.name)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| {
            ApiError::not_found(
                "PROJECT_NOT_FOUND",
                format!("Project not found: {}", project.project_id),
            )
        })?;

    Ok(Json(ProjectDto::from(project_row)))
}

/// Delete a project and all its OTEL data (admin+ role required)
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project_id}",
    tag = "projects",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    responses(
        (status = 204, description = "Project deleted"),
        (status = 403, description = "Cannot delete default project or insufficient permissions"),
        (status = 404, description = "Project not found")
    )
)]
pub async fn delete_project(
    State(state): State<ProjectsApiState>,
    project: ProjectFull,
    auth_service: axum::Extension<Arc<AuthService>>,
) -> Result<StatusCode, ApiError> {
    // Cannot delete default project
    if project.project_id == DEFAULT_PROJECT_ID {
        return Err(ApiError::forbidden(
            "CANNOT_DELETE_DEFAULT",
            "The default project cannot be deleted",
        ));
    }

    // Verify admin role in project's org
    auth_service
        .verify_org_role(
            &project.auth,
            &project.org_id,
            ApiKeyScope::Full,
            ORG_ROLE_ADMIN,
        )
        .await?;

    // Cleanup: analytics traces + files + API key caches + transactional DB
    let deleted = cleanup_project(
        &state.database,
        &state.analytics,
        &state.file_service,
        Some(&state.cache),
        &project.project_id,
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;

    if !deleted {
        return Err(ApiError::not_found(
            "PROJECT_NOT_FOUND",
            format!("Project not found: {}", project.project_id),
        ));
    }

    Ok(StatusCode::NO_CONTENT)
}
