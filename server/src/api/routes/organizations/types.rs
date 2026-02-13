//! Organization API types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::api::types::{default_limit, default_page, validate_limit, validate_page};
use crate::core::constants::{
    ORG_ROLE_ADMIN, ORG_ROLE_MEMBER, ORG_ROLE_OWNER, ORG_ROLE_VIEWER, ORG_SLUG_MAX_LEN,
    ORG_SLUG_MIN_LEN,
};
use crate::data::types::{MemberWithUser, OrgWithRole, OrganizationRow};

/// Organization DTO for API responses
#[derive(Debug, Serialize, ToSchema)]
pub struct OrganizationDto {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<OrganizationRow> for OrganizationDto {
    fn from(row: OrganizationRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            slug: row.slug,
            created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or_else(Utc::now),
            updated_at: DateTime::from_timestamp(row.updated_at, 0).unwrap_or_else(Utc::now),
        }
    }
}

/// Organization with user's role DTO
#[derive(Debug, Serialize, ToSchema)]
pub struct OrgWithRoleDto {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<OrgWithRole> for OrgWithRoleDto {
    fn from(row: OrgWithRole) -> Self {
        Self {
            id: row.id,
            name: row.name,
            slug: row.slug,
            role: row.role,
            created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or_else(Utc::now),
            updated_at: DateTime::from_timestamp(row.updated_at, 0).unwrap_or_else(Utc::now),
        }
    }
}

/// Member DTO for API responses
#[derive(Debug, Serialize, ToSchema)]
pub struct MemberDto {
    pub user_id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

impl From<MemberWithUser> for MemberDto {
    fn from(row: MemberWithUser) -> Self {
        Self {
            user_id: row.user_id,
            email: row.email,
            display_name: row.display_name,
            role: row.role,
            joined_at: DateTime::from_timestamp(row.joined_at, 0).unwrap_or_else(Utc::now),
        }
    }
}

/// Request body for creating an organization
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateOrgRequest {
    #[validate(length(min = 1, max = 100, message = "Name must be 1-100 characters"))]
    pub name: String,

    #[validate(custom(function = "validate_slug"))]
    pub slug: String,
}

/// Request body for updating an organization
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateOrgRequest {
    #[validate(length(min = 1, max = 100, message = "Name must be 1-100 characters"))]
    pub name: String,
}

/// Request body for adding/updating a member
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct AddMemberRequest {
    #[validate(length(min = 1, max = 100, message = "User ID must be 1-100 characters"))]
    pub user_id: String,

    #[validate(custom(function = "validate_role"))]
    pub role: String,
}

/// Request body for updating a member's role
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateMemberRoleRequest {
    #[validate(custom(function = "validate_role"))]
    pub role: String,
}

/// Query params for listing organizations
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ListOrgsQuery {
    #[serde(default = "default_page")]
    #[validate(custom(function = "validate_page"))]
    pub page: u32,

    #[serde(default = "default_limit")]
    #[validate(custom(function = "validate_limit"))]
    pub limit: u32,
}

/// Query params for listing members
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ListMembersQuery {
    #[serde(default = "default_page")]
    #[validate(custom(function = "validate_page"))]
    pub page: u32,

    #[serde(default = "default_limit")]
    #[validate(custom(function = "validate_limit"))]
    pub limit: u32,
}

/// Validate organization slug
fn validate_slug(slug: &str) -> Result<(), validator::ValidationError> {
    let len = slug.len();

    if !(ORG_SLUG_MIN_LEN..=ORG_SLUG_MAX_LEN).contains(&len) {
        return Err(validator::ValidationError::new("slug_length")
            .with_message(std::borrow::Cow::Borrowed("Slug must be 1-50 characters")));
    }

    // Check pattern: ASCII lowercase alphanumeric and dashes
    // Uses bytes directly (no allocation) since we validate ASCII-only
    let bytes = slug.as_bytes();
    let is_alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();

    // First and last must be alphanumeric
    let first_valid = is_alnum(bytes[0]);
    let last_valid = is_alnum(bytes[len - 1]);
    // All chars must be alphanumeric or dash
    let all_valid = bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');

    if !first_valid || !last_valid || !all_valid {
        return Err(validator::ValidationError::new("slug_format")
            .with_message(std::borrow::Cow::Borrowed(
                "Slug must contain only lowercase letters, numbers, and dashes (cannot start or end with dash)",
            )));
    }

    Ok(())
}

/// Validate role
fn validate_role(role: &str) -> Result<(), validator::ValidationError> {
    if role == ORG_ROLE_VIEWER
        || role == ORG_ROLE_MEMBER
        || role == ORG_ROLE_ADMIN
        || role == ORG_ROLE_OWNER
    {
        Ok(())
    } else {
        Err(
            validator::ValidationError::new("invalid_role").with_message(
                std::borrow::Cow::Borrowed("Role must be one of: viewer, member, admin, owner"),
            ),
        )
    }
}
