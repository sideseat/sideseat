//! Project API types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::types::{default_limit, default_page, validate_limit, validate_page};
use sideseat_ports::types::{ProjectHold, ProjectRow, ProjectStorageUsage};

/// Project DTO for API responses
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectDto {
    pub id: String,
    pub organization_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ProjectRow> for ProjectDto {
    fn from(row: ProjectRow) -> Self {
        Self {
            id: row.id,
            organization_id: row.organization_id,
            name: row.name,
            created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or(DateTime::UNIX_EPOCH),
            updated_at: DateTime::from_timestamp(row.updated_at, 0).unwrap_or(DateTime::UNIX_EPOCH),
        }
    }
}

/// Request body for creating a project
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateProjectRequest {
    #[validate(length(min = 1, max = 100, message = "Name must be 1-100 characters"))]
    pub name: String,

    /// Organization ID (required - project must belong to an org)
    #[validate(length(
        min = 1,
        max = 100,
        message = "Organization ID must be 1-100 characters"
    ))]
    pub organization_id: String,
}

/// Request body for updating a project
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateProjectRequest {
    #[validate(length(min = 1, max = 100, message = "Name must be 1-100 characters"))]
    pub name: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetProjectHoldRequest {
    pub hold_until: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectHoldDto {
    pub project_id: String,
    pub hold_until: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ProjectHold> for ProjectHoldDto {
    fn from(hold: ProjectHold) -> Self {
        Self {
            project_id: hold.project_id.to_string(),
            hold_until: hold.hold_until,
            updated_at: hold.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectStorageUsageDto {
    pub logical_bytes: u64,
    pub quota_bytes: u64,
    pub maintenance_reserve_bytes: u64,
    pub ordinary_write_limit_bytes: u64,
    pub updated_at: DateTime<Utc>,
}

impl ProjectStorageUsageDto {
    pub fn new(
        usage: ProjectStorageUsage,
        quota_bytes: u64,
        maintenance_reserve_bytes: u64,
        ordinary_write_limit_bytes: u64,
    ) -> Self {
        Self {
            logical_bytes: usage.logical_bytes,
            quota_bytes,
            maintenance_reserve_bytes,
            ordinary_write_limit_bytes,
            updated_at: usage.updated_at,
        }
    }
}

/// Query params for listing projects
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ListProjectsQuery {
    #[serde(default = "default_page")]
    #[validate(custom(function = "validate_page"))]
    pub page: u32,

    #[serde(default = "default_limit")]
    #[validate(custom(function = "validate_limit"))]
    pub limit: u32,

    /// Optional organization ID filter
    pub org_id: Option<String>,
}
