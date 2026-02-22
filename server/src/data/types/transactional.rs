//! Shared transactional types for all database backends (SQLite, PostgreSQL)
//!
//! This module contains row types that are used across transactional database backends.

use serde::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

// ============================================================================
// User types
// ============================================================================

/// User row from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRow {
    pub id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

// ============================================================================
// Organization types
// ============================================================================

/// Organization row from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationRow {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Organization with user's role (for list_for_user)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgWithRole {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub role: String,
    pub created_at: i64,
    pub updated_at: i64,
}

// ============================================================================
// Project types
// ============================================================================

/// Project row from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRow {
    pub id: String,
    pub organization_id: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

// ============================================================================
// Membership types
// ============================================================================

/// Membership row from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MembershipRow {
    pub organization_id: String,
    pub user_id: String,
    pub role: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Member with user info (for list_members)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberWithUser {
    pub user_id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub role: String,
    pub joined_at: i64,
}

/// Result type for operations that may be blocked by last-owner protection
#[derive(Debug, Clone)]
pub enum LastOwnerResult<T> {
    Success(T),
    LastOwner,
    NotFound,
}

// ============================================================================
// Auth method types
// ============================================================================

/// Auth method row from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthMethodRow {
    pub id: String,
    pub user_id: String,
    pub method_type: String,
    pub provider: Option<String>,
    pub provider_id: Option<String>,
    pub credential_hash: Option<String>,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

// ============================================================================
// File types
// ============================================================================

/// File metadata row from database
#[derive(Debug, Clone)]
pub struct FileRow {
    pub id: i64,
    pub project_id: String,
    pub file_hash: String,
    pub media_type: Option<String>,
    pub size_bytes: i64,
    pub hash_algo: String,
    pub ref_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

// ============================================================================
// API Key types
// ============================================================================

/// API key permission scope
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyScope {
    /// Query API only (GET requests)
    Read,
    /// OTEL ingestion only (POST /otel/*)
    Ingest,
    /// read + ingest + modifications
    Write,
    /// Everything including key management
    #[default]
    Full,
}

impl ApiKeyScope {
    /// Check if this scope grants the required permission
    pub fn has_permission(&self, required: ApiKeyScope) -> bool {
        match (self, required) {
            // Full grants everything
            (ApiKeyScope::Full, _) => true,
            // Write grants read, ingest, and write
            (ApiKeyScope::Write, ApiKeyScope::Read | ApiKeyScope::Ingest | ApiKeyScope::Write) => {
                true
            }
            // Exact match
            (a, b) if *a == b => true,
            _ => false,
        }
    }

    /// Parse from string representation
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "read" => Some(Self::Read),
            "ingest" => Some(Self::Ingest),
            "write" => Some(Self::Write),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    /// Convert to string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Ingest => "ingest",
            Self::Write => "write",
            Self::Full => "full",
        }
    }
}

impl fmt::Display for ApiKeyScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// API key row from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRow {
    pub id: String,
    pub org_id: String,
    pub name: String,
    pub key_prefix: String,
    pub scope: ApiKeyScope,
    /// NULL if creator was deleted
    pub created_by: Option<String>,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}

/// API key validation result (for auth lookups)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyValidation {
    pub key_id: String,
    pub org_id: String,
    pub scope: ApiKeyScope,
    /// For UserContext in general API auth
    pub created_by: Option<String>,
    pub expires_at: Option<i64>,
    /// For debounce check
    pub last_used_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_last_owner_result() {
        let success: LastOwnerResult<i32> = LastOwnerResult::Success(42);
        assert!(matches!(success, LastOwnerResult::Success(42)));

        let last_owner: LastOwnerResult<i32> = LastOwnerResult::LastOwner;
        assert!(matches!(last_owner, LastOwnerResult::LastOwner));

        let not_found: LastOwnerResult<i32> = LastOwnerResult::NotFound;
        assert!(matches!(not_found, LastOwnerResult::NotFound));
    }

    #[test]
    fn test_api_key_scope_has_permission() {
        // Full grants everything
        assert!(ApiKeyScope::Full.has_permission(ApiKeyScope::Full));
        assert!(ApiKeyScope::Full.has_permission(ApiKeyScope::Write));
        assert!(ApiKeyScope::Full.has_permission(ApiKeyScope::Read));
        assert!(ApiKeyScope::Full.has_permission(ApiKeyScope::Ingest));

        // Write grants read, ingest, and write
        assert!(ApiKeyScope::Write.has_permission(ApiKeyScope::Write));
        assert!(ApiKeyScope::Write.has_permission(ApiKeyScope::Read));
        assert!(ApiKeyScope::Write.has_permission(ApiKeyScope::Ingest));
        assert!(!ApiKeyScope::Write.has_permission(ApiKeyScope::Full));

        // Read only grants read
        assert!(ApiKeyScope::Read.has_permission(ApiKeyScope::Read));
        assert!(!ApiKeyScope::Read.has_permission(ApiKeyScope::Ingest));
        assert!(!ApiKeyScope::Read.has_permission(ApiKeyScope::Write));
        assert!(!ApiKeyScope::Read.has_permission(ApiKeyScope::Full));

        // Ingest only grants ingest
        assert!(ApiKeyScope::Ingest.has_permission(ApiKeyScope::Ingest));
        assert!(!ApiKeyScope::Ingest.has_permission(ApiKeyScope::Read));
        assert!(!ApiKeyScope::Ingest.has_permission(ApiKeyScope::Write));
        assert!(!ApiKeyScope::Ingest.has_permission(ApiKeyScope::Full));
    }

    #[test]
    fn test_api_key_scope_from_str() {
        assert_eq!(ApiKeyScope::parse("read"), Some(ApiKeyScope::Read));
        assert_eq!(ApiKeyScope::parse("ingest"), Some(ApiKeyScope::Ingest));
        assert_eq!(ApiKeyScope::parse("write"), Some(ApiKeyScope::Write));
        assert_eq!(ApiKeyScope::parse("full"), Some(ApiKeyScope::Full));
        assert_eq!(ApiKeyScope::parse("READ"), Some(ApiKeyScope::Read));
        assert_eq!(ApiKeyScope::parse("invalid"), None);
    }

    #[test]
    fn test_api_key_scope_display() {
        assert_eq!(ApiKeyScope::Read.to_string(), "read");
        assert_eq!(ApiKeyScope::Ingest.to_string(), "ingest");
        assert_eq!(ApiKeyScope::Write.to_string(), "write");
        assert_eq!(ApiKeyScope::Full.to_string(), "full");
    }
}
