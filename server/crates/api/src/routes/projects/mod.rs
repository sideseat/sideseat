//! Project API endpoints

pub mod types;

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use crate::auth::{Auth, AuthContext, AuthService, ProjectFull, ProjectRead, ProjectWrite};
use crate::extractors::{ValidatedJson, ValidatedQuery};
use crate::types::{ApiError, PaginatedResponse};
use sideseat_core::constants::{DEFAULT_PROJECT_ID, ORG_ROLE_ADMIN, ORG_ROLE_MEMBER};
use sideseat_domain::cleanup::cleanup_project;
use sideseat_domain::files::FileService;
use sideseat_domain::storage_governance::StorageGovernanceService;
use sideseat_ports::traits::{DeletionCause, DeletionRecord, DeletionScope};
use sideseat_ports::types::ApiKeyScope;

use types::{
    CreateProjectRequest, ListProjectsQuery, ProjectDto, ProjectHoldDto, ProjectStorageUsageDto,
    SetProjectHoldRequest, UpdateProjectRequest,
};

/// Shared state for Projects API endpoints
#[derive(Clone)]
pub struct ProjectsApiState {
    pub database: Arc<crate::dependencies::TransactionalStore>,
    pub analytics: Arc<crate::dependencies::AnalyticsStore>,
    pub file_service: Arc<FileService>,
    pub cache: Arc<crate::dependencies::SharedCache>,
    pub storage_governance: Arc<StorageGovernanceService>,
}

/// Build Projects API routes
pub fn routes(
    database: Arc<crate::dependencies::TransactionalStore>,
    analytics: Arc<crate::dependencies::AnalyticsStore>,
    file_service: Arc<FileService>,
    cache: Arc<crate::dependencies::SharedCache>,
    storage_governance: Arc<StorageGovernanceService>,
) -> Router<()> {
    let state = ProjectsApiState {
        database,
        analytics,
        file_service,
        cache,
        storage_governance,
    };

    Router::new()
        .route("/", get(list_projects).post(create_project))
        .route(
            "/{project_id}",
            get(get_project).put(update_project).delete(delete_project),
        )
        .route(
            "/{project_id}/hold",
            get(get_project_hold)
                .put(set_project_hold)
                .delete(clear_project_hold),
        )
        .route("/{project_id}/storage", get(get_project_storage))
        .with_state(state)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project_id}/hold",
    tag = "projects",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    responses(
        (status = 200, description = "Current project legal hold", body = ProjectHoldDto),
        (status = 403, description = "Not authorized to read the project"),
        (status = 404, description = "No legal hold exists for the project")
    )
)]
pub async fn get_project_hold(
    State(state): State<ProjectsApiState>,
    project: ProjectRead,
) -> Result<Json<ProjectHoldDto>, ApiError> {
    state
        .storage_governance
        .current_hold(&project.project_id)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
        .map(ProjectHoldDto::from)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("LEGAL_HOLD_NOT_FOUND", "No active legal hold"))
}

#[utoipa::path(
    put,
    path = "/api/v1/projects/{project_id}/hold",
    tag = "projects",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    request_body = SetProjectHoldRequest,
    responses(
        (status = 200, description = "Legal hold set and existing signal rows patched", body = ProjectHoldDto),
        (status = 400, description = "Invalid hold deadline"),
        (status = 403, description = "Organization administrator access required"),
        (status = 409, description = "Legal hold could not be established")
    )
)]
pub async fn set_project_hold(
    State(state): State<ProjectsApiState>,
    project: ProjectFull,
    auth_service: axum::Extension<Arc<AuthService>>,
    ValidatedJson(body): ValidatedJson<SetProjectHoldRequest>,
) -> Result<Json<ProjectHoldDto>, ApiError> {
    auth_service
        .verify_org_role(
            &project.auth,
            &project.org_id,
            ApiKeyScope::Full,
            ORG_ROLE_ADMIN,
        )
        .await?;
    let hold = state
        .storage_governance
        .set_hold(&project.project_id, body.hold_until)
        .await
        .map_err(|error| ApiError::conflict("LEGAL_HOLD_FAILED", error.to_string()))?;
    Ok(Json(ProjectHoldDto::from(hold)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project_id}/hold",
    tag = "projects",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    responses(
        (status = 204, description = "Legal hold cleared"),
        (status = 403, description = "Organization administrator access required"),
        (status = 404, description = "No legal hold exists for the project")
    )
)]
pub async fn clear_project_hold(
    State(state): State<ProjectsApiState>,
    project: ProjectFull,
    auth_service: axum::Extension<Arc<AuthService>>,
) -> Result<StatusCode, ApiError> {
    auth_service
        .verify_org_role(
            &project.auth,
            &project.org_id,
            ApiKeyScope::Full,
            ORG_ROLE_ADMIN,
        )
        .await?;
    if !state
        .storage_governance
        .clear_hold(&project.project_id)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
    {
        return Err(ApiError::not_found(
            "LEGAL_HOLD_NOT_FOUND",
            "No legal hold exists for this project",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project_id}/storage",
    tag = "projects",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    responses(
        (status = 200, description = "Reconciled logical storage usage and quota limits", body = ProjectStorageUsageDto),
        (status = 403, description = "Not authorized to read the project")
    )
)]
pub async fn get_project_storage(
    State(state): State<ProjectsApiState>,
    project: ProjectRead,
) -> Result<Json<ProjectStorageUsageDto>, ApiError> {
    let usage = state
        .storage_governance
        .reconcile_project(&project.project_id)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(Json(ProjectStorageUsageDto::new(
        usage,
        state.storage_governance.quota_bytes(),
        state.storage_governance.maintenance_reserve_bytes(),
        state.storage_governance.ordinary_limit_bytes(),
    )))
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
    let repo = state.database.as_ref();

    // Security: require Read scope
    auth.ctx.require_scope(ApiKeyScope::Read)?;

    let (projects, total) = match (&query.org_id, &auth.ctx) {
        // Explicit org_id filter: verify access to that org
        (Some(org_id), _) => {
            auth_service
                .verify_org_access(&auth.ctx, org_id, ApiKeyScope::Read)
                .await?;
            repo.list_projects_for_org(org_id, query.page, query.limit)
                .await
                .map_err(ApiError::from_data)?
        }
        // API key without org_id filter: list projects in key's org
        (None, AuthContext::ApiKey { org_id, .. }) => repo
            .list_projects_for_org(org_id, query.page, query.limit)
            .await
            .map_err(ApiError::from_data)?,
        // Session/Local auth: list all projects across user's orgs
        (None, _) => {
            let user_id = auth.require_user_id()?;
            repo.list_projects_for_user(user_id, query.page, query.limit)
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
    let repo = state.database.as_ref();

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
        .create_project(&body.organization_id, &body.name)
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
    let repo = state.database.as_ref();

    let project_row = repo
        .get_project(&project.project_id)
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

    let repo = state.database.as_ref();
    let project_row = repo
        .update_project(&project.project_id, &body.name)
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
        (status = 404, description = "Project not found"),
        (status = 409, description = "Project legal hold is active or maintenance reserve is exhausted")
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

    let lease = state
        .storage_governance
        .acquire_maintenance(&project.project_id, "project-delete")
        .await
        .map_err(|error| ApiError::conflict("PROJECT_MAINTENANCE_BUSY", error.to_string()))?;
    let deletion = async {
        if let Some(hold) = state
            .storage_governance
            .current_hold(&project.project_id)
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?
        {
            return Err(ApiError::conflict(
                "LEGAL_HOLD_ACTIVE",
                format!(
                    "Project deletion refused: legal hold is active until {}",
                    hold.hold_until
                ),
            ));
        }
        let journal_bytes = DeletionRecord::logical_bytes_for(
            project.project_id.as_str(),
            DeletionCause::Requested,
            DeletionScope::Project,
            project.project_id.as_str(),
            None,
        );
        state
            .storage_governance
            .reserve_maintenance_under_fence(&project.project_id, journal_bytes)
            .await
            .map_err(|error| {
                ApiError::conflict("MAINTENANCE_RESERVE_EXHAUSTED", error.to_string())
            })?;

        cleanup_project(
            &state.database,
            &state.analytics,
            &state.file_service,
            Some(state.cache.as_ref()),
            &project.project_id,
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
    }
    .await;
    if let Err(error) = state
        .storage_governance
        .reconcile_project(&project.project_id)
        .await
    {
        tracing::warn!(
            project_id = %project.project_id,
            %error,
            "Could not reconcile storage after project deletion"
        );
    }
    state
        .storage_governance
        .release_maintenance(&project.project_id, &lease)
        .await;
    let deleted = deletion?;

    if !deleted {
        return Err(ApiError::not_found(
            "PROJECT_NOT_FOUND",
            format!("Project not found: {}", project.project_id),
        ));
    }

    Ok(StatusCode::NO_CONTENT)
}
