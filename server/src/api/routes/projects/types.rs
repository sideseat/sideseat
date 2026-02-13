//! Project API types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::api::types::{default_limit, default_page, validate_limit, validate_page};
use crate::data::types::ProjectRow;

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
            created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or_else(Utc::now),
            updated_at: DateTime::from_timestamp(row.updated_at, 0).unwrap_or_else(Utc::now),
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
