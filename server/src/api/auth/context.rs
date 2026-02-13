//! Unified authentication context and authorization service
//!
//! Provides a single `AuthContext` type that represents all authentication methods
//! and an `AuthService` with cached authorization checks.

use std::sync::Arc;
use std::time::Duration;

use crate::api::types::ApiError;
use crate::data::TransactionalService;
use crate::data::cache::{CacheKey, CacheService};
use crate::data::types::ApiKeyScope;

// ============================================================================
// Cache TTLs
// ============================================================================

/// TTL for project->org mapping (rarely changes)
const CACHE_TTL_PROJECT_ORG: Duration = Duration::from_secs(300); // 5 minutes

/// TTL for user->org membership (can change, but not frequently)
const CACHE_TTL_USER_ORG_MEMBER: Duration = Duration::from_secs(60); // 1 minute

// ============================================================================
// AuthContext
// ============================================================================

/// Unified authentication context for all auth methods
///
/// Replaces separate `UserContext` and `ApiKeyContext` with a single enum
/// that captures all authentication state needed for authorization.
#[derive(Debug, Clone)]
pub enum AuthContext {
    /// Session-authenticated user via JWT
    Session { user_id: String },
    /// API key authentication
    ApiKey {
        key_id: String,
        org_id: String,
        scope: ApiKeyScope,
        /// User who created the key (may be None if creator was deleted)
        created_by: Option<String>,
    },
    /// Default local user (--no-auth mode, no API key provided)
    LocalDefault { user_id: String },
}

impl AuthContext {
    /// Get user_id for operations that require a user identity.
    /// Returns None for orphaned API keys.
    pub fn user_id(&self) -> Option<&str> {
        match self {
            Self::Session { user_id } | Self::LocalDefault { user_id } => Some(user_id),
            Self::ApiKey { created_by, .. } => created_by.as_deref(),
        }
    }

    /// Get user_id, returning error if not available (for management APIs).
    pub fn require_user_id(&self) -> Result<&str, ApiError> {
        self.user_id()
            .ok_or_else(|| ApiError::forbidden("ORPHANED_KEY", "API key creator no longer exists"))
    }

    /// Check if this auth context has the required scope.
    /// Session and LocalDefault have implicit full access.
    pub fn has_scope(&self, required: ApiKeyScope) -> bool {
        match self {
            Self::ApiKey { scope, .. } => scope.has_permission(required),
            _ => true,
        }
    }

    /// Check scope, returning error if insufficient.
    pub fn require_scope(&self, required: ApiKeyScope) -> Result<(), ApiError> {
        if self.has_scope(required) {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "INSUFFICIENT_SCOPE",
                format!("This operation requires '{}' scope", required),
            ))
        }
    }
}

// ============================================================================
// AuthService
// ============================================================================

/// Authorization service with cached lookups
///
/// Provides efficient authorization checks with caching for:
/// - Project to organization mapping
/// - User organization membership
#[derive(Clone)]
pub struct AuthService {
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
}

impl AuthService {
    /// Create a new AuthService
    pub fn new(database: Arc<TransactionalService>, cache: Arc<CacheService>) -> Self {
        Self { database, cache }
    }

    /// Check if API key's org matches the target org.
    /// Returns Ok(()) for non-API key auth types.
    fn check_api_key_org(
        &self,
        auth: &AuthContext,
        target_org_id: &str,
        resource: &str,
    ) -> Result<(), ApiError> {
        if let AuthContext::ApiKey { org_id, .. } = auth
            && org_id != target_org_id
        {
            return Err(ApiError::forbidden(
                "ORG_MISMATCH",
                format!("API key cannot access this {}", resource),
            ));
        }
        Ok(())
    }

    /// Get project's organization ID (cached)
    async fn get_project_org_id(&self, project_id: &str) -> Result<String, ApiError> {
        let cache_key = CacheKey::project_org(project_id);

        // Try cache first
        if let Ok(Some(org_id)) = self.cache.get::<String>(&cache_key).await {
            return Ok(org_id);
        }

        // Query database
        let project = self
            .database
            .repository()
            .get_project(None, project_id)
            .await
            .map_err(ApiError::from_data)?
            .ok_or_else(|| {
                ApiError::not_found(
                    "PROJECT_NOT_FOUND",
                    format!("Project not found: {}", project_id),
                )
            })?;

        // Cache the mapping
        if let Err(e) = self
            .cache
            .set(
                &cache_key,
                &project.organization_id,
                Some(CACHE_TTL_PROJECT_ORG),
            )
            .await
        {
            tracing::warn!(project_id = %project_id, error = %e, "Failed to cache project->org mapping");
        }

        Ok(project.organization_id)
    }

    /// Check if user is a member of organization (cached)
    async fn is_user_org_member(&self, user_id: &str, org_id: &str) -> Result<bool, ApiError> {
        let cache_key = CacheKey::user_org_member(user_id, org_id);

        // Try cache first
        if let Ok(Some(is_member)) = self.cache.get::<bool>(&cache_key).await {
            return Ok(is_member);
        }

        // Query database using get_membership (returns Option<MembershipRow>)
        let membership = self
            .database
            .repository()
            .get_membership(None, org_id, user_id)
            .await
            .map_err(ApiError::from_data)?;

        let is_member = membership.is_some();

        // Cache the result
        if let Err(e) = self
            .cache
            .set(&cache_key, &is_member, Some(CACHE_TTL_USER_ORG_MEMBER))
            .await
        {
            tracing::warn!(user_id = %user_id, org_id = %org_id, error = %e, "Failed to cache org membership");
        }

        Ok(is_member)
    }

    /// Verify access to a project and return its org_id.
    ///
    /// This is the primary authorization check for project-scoped routes.
    /// It verifies:
    /// 1. The required scope (for API keys)
    /// 2. The project exists
    /// 3. The user/key has access to the project's organization
    pub async fn verify_project_access(
        &self,
        auth: &AuthContext,
        project_id: &str,
        required_scope: ApiKeyScope,
    ) -> Result<String, ApiError> {
        // Check scope first (fast, no DB)
        auth.require_scope(required_scope)?;

        // Get project's org (cached)
        let project_org_id = self.get_project_org_id(project_id).await?;

        // Verify access based on auth type
        match auth {
            AuthContext::Session { user_id } => {
                if !self.is_user_org_member(user_id, &project_org_id).await? {
                    return Err(ApiError::forbidden(
                        "ACCESS_DENIED",
                        "Not a member of project's organization",
                    ));
                }
            }
            AuthContext::ApiKey { .. } => {
                self.check_api_key_org(auth, &project_org_id, "project")?;
            }
            AuthContext::LocalDefault { .. } => {
                // No restrictions in local mode
            }
        }

        Ok(project_org_id)
    }

    /// Verify access to an organization.
    ///
    /// This is the primary authorization check for org-scoped routes.
    pub async fn verify_org_access(
        &self,
        auth: &AuthContext,
        org_id: &str,
        required_scope: ApiKeyScope,
    ) -> Result<(), ApiError> {
        // Check scope first
        auth.require_scope(required_scope)?;

        // Verify access based on auth type
        match auth {
            AuthContext::Session { user_id } => {
                if !self.is_user_org_member(user_id, org_id).await? {
                    return Err(ApiError::forbidden(
                        "ACCESS_DENIED",
                        "Not a member of organization",
                    ));
                }
            }
            AuthContext::ApiKey { .. } => {
                self.check_api_key_org(auth, org_id, "organization")?;
            }
            AuthContext::LocalDefault { .. } => {
                // No restrictions
            }
        }

        Ok(())
    }

    /// Verify access to an organization with minimum role requirement.
    ///
    /// Used for routes that require admin or owner role.
    pub async fn verify_org_role(
        &self,
        auth: &AuthContext,
        org_id: &str,
        required_scope: ApiKeyScope,
        min_role: &str,
    ) -> Result<(), ApiError> {
        // Check scope first
        auth.require_scope(required_scope)?;

        // Verify access based on auth type
        match auth {
            AuthContext::Session { user_id } => {
                self.check_user_org_role(user_id, org_id, min_role).await?;
            }
            AuthContext::ApiKey { created_by, .. } => {
                self.check_api_key_org(auth, org_id, "organization")?;
                // For role checks, use the key creator's role
                let user_id = created_by.as_ref().ok_or_else(|| {
                    ApiError::forbidden("ORPHANED_KEY", "API key creator no longer exists")
                })?;
                self.check_user_org_role(user_id, org_id, min_role).await?;
            }
            AuthContext::LocalDefault { .. } => {
                // No restrictions in local mode
            }
        }

        Ok(())
    }

    /// Check if user has minimum role in organization
    async fn check_user_org_role(
        &self,
        user_id: &str,
        org_id: &str,
        min_role: &str,
    ) -> Result<(), ApiError> {
        use crate::data::traits::has_min_role_level;

        let membership = self
            .database
            .repository()
            .get_membership(None, org_id, user_id)
            .await
            .map_err(ApiError::from_data)?
            .ok_or_else(|| ApiError::forbidden("ACCESS_DENIED", "Not a member of organization"))?;

        if !has_min_role_level(&membership.role, min_role) {
            return Err(ApiError::forbidden(
                "INSUFFICIENT_ROLE",
                format!("{} role or higher required", min_role),
            ));
        }

        Ok(())
    }
}
