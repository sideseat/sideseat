//! Organization API endpoints

pub mod types;

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;

use crate::auth::{Auth, OrgAdmin, OrgOwner, OrgRead};
use crate::extractors::{ValidatedJson, ValidatedQuery};
use crate::types::{ApiError, PaginatedResponse};
use sideseat_core::core::constants::{
    DEFAULT_ORG_ID, ORG_ROLE_ADMIN, ORG_ROLE_OWNER, RESERVED_SLUGS,
};
use sideseat_domain::cleanup::cleanup_organization;
use sideseat_domain::files::FileService;
use sideseat_domain::storage_governance::StorageGovernanceService;
use sideseat_ports::traits::{DeletionCause, DeletionRecord, DeletionScope, has_min_role_level};
use sideseat_ports::types::{LastOwnerResult, ProjectId};

/// Path parameters for member-specific routes
#[derive(Deserialize)]
pub struct MemberPath {
    /// Target user ID from path
    pub user_id: String,
}

use types::{
    AddMemberRequest, CreateOrgRequest, ListMembersQuery, ListOrgsQuery, MemberDto, OrgWithRoleDto,
    OrganizationDto, UpdateMemberRoleRequest, UpdateOrgRequest,
};

/// Shared state for Organizations API endpoints
#[derive(Clone)]
pub struct OrganizationsApiState {
    pub database: Arc<crate::dependencies::TransactionalStore>,
    pub analytics: Arc<crate::dependencies::AnalyticsStore>,
    pub file_service: Arc<FileService>,
    pub cache: Arc<crate::dependencies::SharedCache>,
    pub storage_governance: Arc<StorageGovernanceService>,
}

/// Build Organizations API routes
pub fn routes(
    database: Arc<crate::dependencies::TransactionalStore>,
    analytics: Arc<crate::dependencies::AnalyticsStore>,
    file_service: Arc<FileService>,
    cache: Arc<crate::dependencies::SharedCache>,
    storage_governance: Arc<StorageGovernanceService>,
) -> Router<()> {
    let state = OrganizationsApiState {
        database,
        analytics,
        file_service,
        cache,
        storage_governance,
    };

    Router::new()
        .route("/", get(list_organizations).post(create_org))
        .route("/{org_id}", get(get_org).put(update_org).delete(delete_org))
        .route(
            "/{org_id}/members",
            get(list_org_members).post(add_org_member),
        )
        .route(
            "/{org_id}/members/{user_id}",
            put(update_member_role).delete(remove_org_member),
        )
        .with_state(state)
}

/// List organizations for the current user
#[utoipa::path(
    get,
    path = "/api/v1/organizations",
    tag = "organizations",
    params(
        ("page" = Option<u32>, Query, description = "Page number (1-100)"),
        ("limit" = Option<u32>, Query, description = "Items per page (1-100)")
    ),
    responses(
        (status = 200, description = "List of organizations with pagination metadata")
    )
)]
pub async fn list_organizations(
    State(state): State<OrganizationsApiState>,
    auth: Auth,
    ValidatedQuery(query): ValidatedQuery<ListOrgsQuery>,
) -> Result<Json<PaginatedResponse<OrgWithRoleDto>>, ApiError> {
    let user_id = auth.require_user_id()?;
    let repo = state.database.as_ref();
    let (orgs, total) = repo
        .list_orgs_for_user(user_id, query.page, query.limit)
        .await
        .map_err(ApiError::from_data)?;

    let data: Vec<OrgWithRoleDto> = orgs.into_iter().map(OrgWithRoleDto::from).collect();

    Ok(Json(PaginatedResponse::new(
        data,
        query.page,
        query.limit,
        total,
    )))
}

/// Create a new organization (user becomes owner)
#[utoipa::path(
    post,
    path = "/api/v1/organizations",
    tag = "organizations",
    request_body = CreateOrgRequest,
    responses(
        (status = 201, description = "Organization created", body = OrganizationDto),
        (status = 400, description = "Invalid request or reserved slug"),
        (status = 409, description = "Slug already exists")
    )
)]
pub async fn create_org(
    State(state): State<OrganizationsApiState>,
    auth: Auth,
    ValidatedJson(body): ValidatedJson<CreateOrgRequest>,
) -> Result<(StatusCode, Json<OrganizationDto>), ApiError> {
    let user_id = auth.require_user_id()?;

    // Check reserved slugs
    if RESERVED_SLUGS.contains(&body.slug.as_str()) {
        return Err(ApiError::bad_request(
            "RESERVED_SLUG",
            format!("The slug '{}' is reserved and cannot be used", body.slug),
        ));
    }

    let repo = state.database.as_ref();

    // Create org + add owner membership atomically in a transaction
    let org = repo
        .create_organization_with_owner(&body.name, &body.slug, user_id)
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint failed") {
                ApiError::conflict(
                    "SLUG_EXISTS",
                    "An organization with this slug already exists",
                )
            } else {
                ApiError::from_data(e)
            }
        })?;

    Ok((StatusCode::CREATED, Json(OrganizationDto::from(org))))
}

/// Get a single organization by ID
#[utoipa::path(
    get,
    path = "/api/v1/organizations/{org_id}",
    tag = "organizations",
    params(
        ("org_id" = String, Path, description = "Organization ID")
    ),
    responses(
        (status = 200, description = "Organization details", body = OrganizationDto),
        (status = 403, description = "Not a member of this organization"),
        (status = 404, description = "Organization not found")
    )
)]
pub async fn get_org(
    State(state): State<OrganizationsApiState>,
    auth: OrgRead,
) -> Result<Json<OrganizationDto>, ApiError> {
    let repo = state.database.as_ref();

    let org = repo
        .get_organization(&auth.org_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| {
            ApiError::not_found(
                "ORG_NOT_FOUND",
                format!("Organization not found: {}", auth.org_id),
            )
        })?;

    Ok(Json(OrganizationDto::from(org)))
}

/// Update an organization's name (admin+ required, slug is immutable)
#[utoipa::path(
    put,
    path = "/api/v1/organizations/{org_id}",
    tag = "organizations",
    params(
        ("org_id" = String, Path, description = "Organization ID")
    ),
    request_body = UpdateOrgRequest,
    responses(
        (status = 200, description = "Organization updated", body = OrganizationDto),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Organization not found")
    )
)]
pub async fn update_org(
    State(state): State<OrganizationsApiState>,
    auth: OrgAdmin,
    ValidatedJson(body): ValidatedJson<UpdateOrgRequest>,
) -> Result<Json<OrganizationDto>, ApiError> {
    let repo = state.database.as_ref();

    let org = repo
        .update_organization(&auth.org_id, &body.name)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| {
            ApiError::not_found(
                "ORG_NOT_FOUND",
                format!("Organization not found: {}", auth.org_id),
            )
        })?;

    Ok(Json(OrganizationDto::from(org)))
}

/// Delete an organization (owner only, cannot delete default)
/// Performs full cleanup: analytics data, files, then transactional data
#[utoipa::path(
    delete,
    path = "/api/v1/organizations/{org_id}",
    tag = "organizations",
    params(
        ("org_id" = String, Path, description = "Organization ID")
    ),
    responses(
        (status = 204, description = "Organization deleted"),
        (status = 403, description = "Cannot delete default organization or insufficient permissions"),
        (status = 404, description = "Organization not found"),
        (status = 409, description = "A project legal hold is active or maintenance reserve is exhausted")
    )
)]
pub async fn delete_org(
    State(state): State<OrganizationsApiState>,
    auth: OrgOwner,
) -> Result<StatusCode, ApiError> {
    // Cannot delete default org
    if auth.org_id == DEFAULT_ORG_ID {
        return Err(ApiError::forbidden(
            "CANNOT_DELETE_DEFAULT",
            "The default organization cannot be deleted",
        ));
    }

    let mut project_ids = state
        .database
        .list_project_ids(&auth.org_id)
        .await
        .map_err(ApiError::from_data)?
        .into_iter()
        .map(ProjectId::from)
        .collect::<Vec<_>>();
    project_ids.sort();
    let organization_charge_id = ProjectId::from(auth.org_id.as_str());
    let mut fenced_ids = Vec::with_capacity(project_ids.len() + 1);
    fenced_ids.push(organization_charge_id.clone());
    fenced_ids.extend(project_ids.iter().cloned());
    let mut leases = Vec::with_capacity(fenced_ids.len());
    for project_id in &fenced_ids {
        match state
            .storage_governance
            .acquire_maintenance(project_id, "organization-delete")
            .await
        {
            Ok(owner) => leases.push((project_id.clone(), owner)),
            Err(error) => {
                for (project_id, owner) in &leases {
                    state
                        .storage_governance
                        .release_maintenance(project_id, owner)
                        .await;
                }
                return Err(ApiError::conflict(
                    "ORGANIZATION_MAINTENANCE_BUSY",
                    error.to_string(),
                ));
            }
        }
    }

    let deletion = async {
        for project_id in &project_ids {
            if let Some(hold) = state
                .storage_governance
                .current_hold(project_id)
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?
            {
                return Err(ApiError::conflict(
                    "LEGAL_HOLD_ACTIVE",
                    format!(
                        "Organization deletion refused: project {} has a legal hold until {}",
                        project_id, hold.hold_until
                    ),
                ));
            }
        }
        let org_journal_bytes = DeletionRecord::logical_bytes_for(
            &auth.org_id,
            DeletionCause::Requested,
            DeletionScope::Organization,
            &auth.org_id,
            None,
        );
        state
            .storage_governance
            .reserve_maintenance_under_fence(&organization_charge_id, org_journal_bytes)
            .await
            .map_err(|error| {
                ApiError::conflict("MAINTENANCE_RESERVE_EXHAUSTED", error.to_string())
            })?;
        for project_id in &project_ids {
            let journal_bytes = DeletionRecord::logical_bytes_for(
                project_id.as_str(),
                DeletionCause::Requested,
                DeletionScope::Project,
                project_id.as_str(),
                None,
            );
            state
                .storage_governance
                .reserve_maintenance_under_fence(project_id, journal_bytes)
                .await
                .map_err(|error| {
                    ApiError::conflict("MAINTENANCE_RESERVE_EXHAUSTED", error.to_string())
                })?;
        }

        cleanup_organization(
            &state.database,
            &state.analytics,
            &state.file_service,
            Some(state.cache.as_ref()),
            &auth.org_id,
        )
        .await
        .map_err(|e| ApiError::internal(format!("Failed to delete organization: {}", e)))
    }
    .await;
    for project_id in &fenced_ids {
        if let Err(error) = state.storage_governance.reconcile_project(project_id).await {
            tracing::warn!(
                project_id = %project_id,
                %error,
                "Could not reconcile storage after organization deletion"
            );
        }
    }
    for (project_id, owner) in &leases {
        state
            .storage_governance
            .release_maintenance(project_id, owner)
            .await;
    }
    let deleted = deletion?;

    if !deleted {
        return Err(ApiError::not_found(
            "ORG_NOT_FOUND",
            format!("Organization not found: {}", auth.org_id),
        ));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// List members of an organization
#[utoipa::path(
    get,
    path = "/api/v1/organizations/{org_id}/members",
    tag = "organizations",
    params(
        ("org_id" = String, Path, description = "Organization ID"),
        ("page" = Option<u32>, Query, description = "Page number (1-100)"),
        ("limit" = Option<u32>, Query, description = "Items per page (1-100)")
    ),
    responses(
        (status = 200, description = "List of members with pagination metadata"),
        (status = 403, description = "Not a member of this organization")
    )
)]
pub async fn list_org_members(
    State(state): State<OrganizationsApiState>,
    auth: OrgRead,
    ValidatedQuery(query): ValidatedQuery<ListMembersQuery>,
) -> Result<Json<PaginatedResponse<MemberDto>>, ApiError> {
    let repo = state.database.as_ref();

    let (members, total) = repo
        .list_members(&auth.org_id, query.page, query.limit)
        .await
        .map_err(ApiError::from_data)?;

    let data: Vec<MemberDto> = members.into_iter().map(MemberDto::from).collect();

    Ok(Json(PaginatedResponse::new(
        data,
        query.page,
        query.limit,
        total,
    )))
}

/// Add or update a member in an organization (admin+ required)
#[utoipa::path(
    post,
    path = "/api/v1/organizations/{org_id}/members",
    tag = "organizations",
    params(
        ("org_id" = String, Path, description = "Organization ID")
    ),
    request_body = AddMemberRequest,
    responses(
        (status = 201, description = "Member added", body = MemberDto),
        (status = 200, description = "Member role updated", body = MemberDto),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "User not found")
    )
)]
pub async fn add_org_member(
    State(state): State<OrganizationsApiState>,
    auth: OrgAdmin,
    ValidatedJson(body): ValidatedJson<AddMemberRequest>,
) -> Result<(StatusCode, Json<MemberDto>), ApiError> {
    let user_id = auth.auth.require_user_id()?;
    let repo = state.database.as_ref();

    // Get current user's exact role for owner-specific logic
    let current_membership = repo
        .get_membership(&auth.org_id, user_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| {
            ApiError::forbidden("NOT_A_MEMBER", "You are not a member of this organization")
        })?;

    // Admin cannot assign owner role
    if body.role == ORG_ROLE_OWNER && current_membership.role != ORG_ROLE_OWNER {
        return Err(ApiError::forbidden(
            "CANNOT_ASSIGN_OWNER",
            "Only owners can assign the owner role",
        ));
    }

    // Verify user exists before adding
    repo.get_user(&body.user_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| ApiError::not_found("USER_NOT_FOUND", "User not found"))?;

    // Check if already a member (for correct status code)
    let is_new_member = repo
        .get_membership(&auth.org_id, &body.user_id)
        .await
        .map_err(ApiError::from_data)?
        .is_none();

    // Add/update member (upsert)
    repo.add_member(&auth.org_id, &body.user_id, &body.role)
        .await
        .map_err(ApiError::from_data)?;

    // Fetch the member with user info (efficient single-row query)
    let member = repo
        .get_member_with_user(&auth.org_id, &body.user_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| ApiError::internal("Failed to fetch created member"))?;

    let status = if is_new_member {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };

    Ok((status, Json(MemberDto::from(member))))
}

/// Update a member's role (admin+ required)
#[utoipa::path(
    put,
    path = "/api/v1/organizations/{org_id}/members/{user_id}",
    tag = "organizations",
    params(
        ("org_id" = String, Path, description = "Organization ID"),
        ("user_id" = String, Path, description = "User ID")
    ),
    request_body = UpdateMemberRoleRequest,
    responses(
        (status = 200, description = "Member role updated", body = MemberDto),
        (status = 403, description = "Insufficient permissions or last owner protection"),
        (status = 404, description = "Member not found")
    )
)]
pub async fn update_member_role(
    State(state): State<OrganizationsApiState>,
    auth: OrgAdmin,
    Path(target): Path<MemberPath>,
    ValidatedJson(body): ValidatedJson<UpdateMemberRoleRequest>,
) -> Result<Json<MemberDto>, ApiError> {
    let user_id = auth.auth.require_user_id()?;
    let repo = state.database.as_ref();

    // Get current user's exact role for owner-specific logic
    let current_membership = repo
        .get_membership(&auth.org_id, user_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| {
            ApiError::forbidden("NOT_A_MEMBER", "You are not a member of this organization")
        })?;

    // Get target user's current role
    let target_membership = repo
        .get_membership(&auth.org_id, &target.user_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| ApiError::not_found("MEMBER_NOT_FOUND", "Member not found"))?;

    // Admin cannot modify owner
    if target_membership.role == ORG_ROLE_OWNER && current_membership.role != ORG_ROLE_OWNER {
        return Err(ApiError::forbidden(
            "CANNOT_MODIFY_OWNER",
            "Only owners can modify other owners",
        ));
    }

    // Admin cannot assign owner role
    if body.role == ORG_ROLE_OWNER && current_membership.role != ORG_ROLE_OWNER {
        return Err(ApiError::forbidden(
            "CANNOT_ASSIGN_OWNER",
            "Only owners can assign the owner role",
        ));
    }

    // Update role atomically with last-owner protection
    match repo
        .update_role_atomic(&auth.org_id, &target.user_id, &body.role)
        .await
        .map_err(ApiError::from_data)?
    {
        LastOwnerResult::Success(_) => {}
        LastOwnerResult::LastOwner => {
            return Err(ApiError::forbidden(
                "LAST_OWNER",
                "Cannot demote the last owner. Assign another owner first.",
            ));
        }
        LastOwnerResult::NotFound => {
            return Err(ApiError::not_found("MEMBER_NOT_FOUND", "Member not found"));
        }
    }

    // Fetch the member with user info (efficient single-row query)
    let member = repo
        .get_member_with_user(&auth.org_id, &target.user_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| ApiError::internal("Failed to fetch updated member"))?;

    Ok(Json(MemberDto::from(member)))
}

/// Remove a member from an organization (admin+ or self-removal)
#[utoipa::path(
    delete,
    path = "/api/v1/organizations/{org_id}/members/{user_id}",
    tag = "organizations",
    params(
        ("org_id" = String, Path, description = "Organization ID"),
        ("user_id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 204, description = "Member removed"),
        (status = 403, description = "Insufficient permissions or last owner protection"),
        (status = 404, description = "Member not found")
    )
)]
pub async fn remove_org_member(
    State(state): State<OrganizationsApiState>,
    auth: OrgRead,
    Path(target): Path<MemberPath>,
) -> Result<StatusCode, ApiError> {
    // For removal, require Full scope if using API key
    auth.auth
        .require_scope(sideseat_ports::types::ApiKeyScope::Full)?;

    let user_id = auth.auth.require_user_id()?;
    let repo = state.database.as_ref();

    // Get current user's role
    let current_membership = repo
        .get_membership(&auth.org_id, user_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| {
            ApiError::forbidden("NOT_A_MEMBER", "You are not a member of this organization")
        })?;

    // Self-removal is allowed for non-last-owners
    let is_self_removal = user_id == target.user_id;

    if !is_self_removal {
        // Must be admin+ to remove others
        if !has_min_role_level(&current_membership.role, ORG_ROLE_ADMIN) {
            return Err(ApiError::forbidden(
                "INSUFFICIENT_PERMISSIONS",
                "Admin role or higher required to remove members",
            ));
        }
    }

    // Get target user's current role
    let target_membership = repo
        .get_membership(&auth.org_id, &target.user_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| ApiError::not_found("MEMBER_NOT_FOUND", "Member not found"))?;

    // Admin cannot remove owner (unless self-removing)
    if target_membership.role == ORG_ROLE_OWNER
        && current_membership.role != ORG_ROLE_OWNER
        && !is_self_removal
    {
        return Err(ApiError::forbidden(
            "CANNOT_REMOVE_OWNER",
            "Only owners can remove other owners",
        ));
    }

    // Remove member atomically with last-owner protection
    match repo
        .remove_member_atomic(&auth.org_id, &target.user_id)
        .await
        .map_err(ApiError::from_data)?
    {
        LastOwnerResult::Success(()) => Ok(StatusCode::NO_CONTENT),
        LastOwnerResult::LastOwner => Err(ApiError::forbidden(
            "LAST_OWNER",
            "Cannot remove the last owner. Assign another owner first.",
        )),
        LastOwnerResult::NotFound => {
            Err(ApiError::not_found("MEMBER_NOT_FOUND", "Member not found"))
        }
    }
}
