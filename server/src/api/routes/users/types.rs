//! User API types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::data::types::{OrgWithRole, UserRow};

/// User DTO for API responses
#[derive(Debug, Serialize, ToSchema)]
pub struct UserDto {
    pub id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<UserRow> for UserDto {
    fn from(row: UserRow) -> Self {
        Self {
            id: row.id,
            email: row.email,
            display_name: row.display_name,
            created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or_else(Utc::now),
            updated_at: DateTime::from_timestamp(row.updated_at, 0).unwrap_or_else(Utc::now),
        }
    }
}

/// Organization membership info for /users/me response
#[derive(Debug, Serialize, ToSchema)]
pub struct UserOrgDto {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub role: String,
}

impl From<OrgWithRole> for UserOrgDto {
    fn from(row: OrgWithRole) -> Self {
        Self {
            id: row.id,
            name: row.name,
            slug: row.slug,
            role: row.role,
        }
    }
}

/// Response for GET /users/me - user profile with all their orgs
#[derive(Debug, Serialize, ToSchema)]
pub struct UserProfileResponse {
    pub user: UserDto,
    pub organizations: Vec<UserOrgDto>,
}

/// Request body for updating user profile
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateUserRequest {
    #[validate(length(max = 100, message = "Display name must be at most 100 characters"))]
    pub display_name: Option<String>,
}
