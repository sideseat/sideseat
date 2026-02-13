//! Authorization extractors for Axum handlers
//!
//! These extractors combine authentication (from middleware) with authorization
//! (scope and access checks) into a single extraction step.
//!
//! # Usage
//!
//! ```no_run
//! # use axum::extract::State;
//! # use sideseat_server::api::auth::ProjectRead;
//! # use sideseat_server::api::types::ApiError;
//! # struct OtelApiState;
//! pub async fn list_traces(
//!     State(state): State<OtelApiState>,
//!     auth: ProjectRead,  // Handles auth + scope + project access check
//! ) -> Result<(), ApiError> {
//!     // auth.project_id - validated project ID from path
//!     // auth.org_id - project's organization ID
//!     // auth.auth - AuthContext for user/API key info
//!     Ok(())
//! }
//! ```

use std::marker::PhantomData;
use std::sync::Arc;

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use serde::Deserialize;

use super::context::{AuthContext, AuthService};
use crate::api::extractors::{ValidationRejection, is_valid_id, is_valid_project_id};
use crate::api::types::ApiError;
use crate::core::constants::{ORG_ROLE_ADMIN, ORG_ROLE_OWNER};
use crate::data::types::ApiKeyScope;

// ============================================================================
// Scope Markers
// ============================================================================

/// Marker trait for scope requirements
pub trait ScopeLevel: Send + Sync + 'static {
    /// The required API key scope
    const SCOPE: ApiKeyScope;
}

/// Read scope marker (query operations)
pub struct Read;
impl ScopeLevel for Read {
    const SCOPE: ApiKeyScope = ApiKeyScope::Read;
}

/// Write scope marker (mutation operations)
pub struct Write;
impl ScopeLevel for Write {
    const SCOPE: ApiKeyScope = ApiKeyScope::Write;
}

/// Full scope marker (management operations)
pub struct Full;
impl ScopeLevel for Full {
    const SCOPE: ApiKeyScope = ApiKeyScope::Full;
}

// ============================================================================
// Auth Rejection
// ============================================================================

/// Rejection type for auth extractors
pub enum AuthRejection {
    /// Path extraction or validation failed
    Path(ValidationRejection),
    /// Authorization failed
    Auth(ApiError),
    /// Auth context not available (middleware not applied)
    MissingContext,
}

impl From<ValidationRejection> for AuthRejection {
    fn from(v: ValidationRejection) -> Self {
        Self::Path(v)
    }
}

impl From<ApiError> for AuthRejection {
    fn from(e: ApiError) -> Self {
        Self::Auth(e)
    }
}

impl axum::response::IntoResponse for AuthRejection {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Path(v) => v.into_response(),
            Self::Auth(e) => e.into_response(),
            Self::MissingContext => {
                ApiError::internal("Auth context not available").into_response()
            }
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract auth context and service from request extensions.
///
/// This is used by all auth extractors to reduce boilerplate.
fn extract_auth(parts: &Parts) -> Result<(AuthContext, Arc<AuthService>), AuthRejection> {
    let auth = parts
        .extensions
        .get::<AuthContext>()
        .cloned()
        .ok_or(AuthRejection::MissingContext)?;

    let auth_service = parts
        .extensions
        .get::<Arc<AuthService>>()
        .cloned()
        .ok_or(AuthRejection::MissingContext)?;

    Ok((auth, auth_service))
}

// ============================================================================
// Project Access Extractors
// ============================================================================

/// Verified project access with parameterized scope.
///
/// Extracts project_id from path, verifies authentication and authorization
/// with the specified scope level, and provides the project's org_id.
///
/// # Type Parameters
///
/// - `Scope`: The required scope level (Read, Write, or Full)
///
/// # Example
///
/// ```no_run
/// # use sideseat_server::api::auth::{ProjectAccess, Read};
/// # use sideseat_server::api::types::ApiError;
/// pub async fn list_traces(auth: ProjectAccess<Read>) -> Result<(), ApiError> {
///     let project_id = &auth.project_id;
///     let org_id = &auth.org_id;
///     Ok(())
/// }
/// ```
pub struct ProjectAccess<Scope: ScopeLevel = Read> {
    /// The validated project ID from the path
    pub project_id: String,
    /// The project's organization ID
    pub org_id: String,
    /// The authentication context
    pub auth: AuthContext,
    _scope: PhantomData<Scope>,
}

/// Type alias for project access with Read scope
pub type ProjectRead = ProjectAccess<Read>;

/// Type alias for project access with Write scope
pub type ProjectWrite = ProjectAccess<Write>;

/// Type alias for project access with Full scope
pub type ProjectFull = ProjectAccess<Full>;

#[derive(Deserialize)]
struct ProjectParams {
    project_id: String,
}

impl<S, Scope> FromRequestParts<S> for ProjectAccess<Scope>
where
    S: Send + Sync,
    Scope: ScopeLevel,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Path(params) = Path::<ProjectParams>::from_request_parts(parts, _state)
            .await
            .map_err(|e| AuthRejection::Path(ValidationRejection::Path(e)))?;

        if !is_valid_project_id(&params.project_id) {
            return Err(AuthRejection::Path(ValidationRejection::InvalidProjectId));
        }

        let (auth, auth_service) = extract_auth(parts)?;
        let org_id = auth_service
            .verify_project_access(&auth, &params.project_id, Scope::SCOPE)
            .await?;

        Ok(Self {
            project_id: params.project_id,
            org_id,
            auth,
            _scope: PhantomData,
        })
    }
}

// ============================================================================
// Trace Access Extractors
// ============================================================================

/// Verified trace access with parameterized scope.
///
/// Extracts project_id and trace_id from path, verifies authentication
/// and authorization with the specified scope level.
pub struct TraceAccess<Scope: ScopeLevel = Read> {
    pub project_id: String,
    pub trace_id: String,
    pub org_id: String,
    pub auth: AuthContext,
    _scope: PhantomData<Scope>,
}

/// Type alias for trace access with Read scope
pub type TraceRead = TraceAccess<Read>;

#[derive(Deserialize)]
struct TraceParams {
    project_id: String,
    trace_id: String,
}

impl<S, Scope> FromRequestParts<S> for TraceAccess<Scope>
where
    S: Send + Sync,
    Scope: ScopeLevel,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Path(params) = Path::<TraceParams>::from_request_parts(parts, _state)
            .await
            .map_err(|e| AuthRejection::Path(ValidationRejection::Path(e)))?;

        if !is_valid_project_id(&params.project_id) {
            return Err(AuthRejection::Path(ValidationRejection::InvalidProjectId));
        }
        if !is_valid_id(&params.trace_id) {
            return Err(AuthRejection::Path(ValidationRejection::InvalidTraceId));
        }

        let (auth, auth_service) = extract_auth(parts)?;
        let org_id = auth_service
            .verify_project_access(&auth, &params.project_id, Scope::SCOPE)
            .await?;

        Ok(Self {
            project_id: params.project_id,
            trace_id: params.trace_id,
            org_id,
            auth,
            _scope: PhantomData,
        })
    }
}

// ============================================================================
// Span Access Extractors
// ============================================================================

/// Verified span access with parameterized scope.
pub struct SpanAccess<Scope: ScopeLevel = Read> {
    pub project_id: String,
    pub trace_id: String,
    pub span_id: String,
    pub org_id: String,
    pub auth: AuthContext,
    _scope: PhantomData<Scope>,
}

/// Type alias for span access with Read scope
pub type SpanRead = SpanAccess<Read>;

#[derive(Deserialize)]
struct SpanParams {
    project_id: String,
    trace_id: String,
    span_id: String,
}

impl<S, Scope> FromRequestParts<S> for SpanAccess<Scope>
where
    S: Send + Sync,
    Scope: ScopeLevel,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Path(params) = Path::<SpanParams>::from_request_parts(parts, _state)
            .await
            .map_err(|e| AuthRejection::Path(ValidationRejection::Path(e)))?;

        if !is_valid_project_id(&params.project_id) {
            return Err(AuthRejection::Path(ValidationRejection::InvalidProjectId));
        }
        if !is_valid_id(&params.trace_id) {
            return Err(AuthRejection::Path(ValidationRejection::InvalidTraceId));
        }
        if !is_valid_id(&params.span_id) {
            return Err(AuthRejection::Path(ValidationRejection::InvalidSpanId));
        }

        let (auth, auth_service) = extract_auth(parts)?;
        let org_id = auth_service
            .verify_project_access(&auth, &params.project_id, Scope::SCOPE)
            .await?;

        Ok(Self {
            project_id: params.project_id,
            trace_id: params.trace_id,
            span_id: params.span_id,
            org_id,
            auth,
            _scope: PhantomData,
        })
    }
}

// ============================================================================
// Session Access Extractors
// ============================================================================

/// Verified session access with parameterized scope.
pub struct SessionAccess<Scope: ScopeLevel = Read> {
    pub project_id: String,
    pub session_id: String,
    pub org_id: String,
    pub auth: AuthContext,
    _scope: PhantomData<Scope>,
}

/// Type alias for session access with Read scope
pub type SessionRead = SessionAccess<Read>;

#[derive(Deserialize)]
struct SessionParams {
    project_id: String,
    session_id: String,
}

impl<S, Scope> FromRequestParts<S> for SessionAccess<Scope>
where
    S: Send + Sync,
    Scope: ScopeLevel,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Path(params) = Path::<SessionParams>::from_request_parts(parts, _state)
            .await
            .map_err(|e| AuthRejection::Path(ValidationRejection::Path(e)))?;

        if !is_valid_project_id(&params.project_id) {
            return Err(AuthRejection::Path(ValidationRejection::InvalidProjectId));
        }
        if !is_valid_id(&params.session_id) {
            return Err(AuthRejection::Path(ValidationRejection::InvalidSessionId));
        }

        let (auth, auth_service) = extract_auth(parts)?;
        let org_id = auth_service
            .verify_project_access(&auth, &params.project_id, Scope::SCOPE)
            .await?;

        Ok(Self {
            project_id: params.project_id,
            session_id: params.session_id,
            org_id,
            auth,
            _scope: PhantomData,
        })
    }
}

// ============================================================================
// Organization Access Extractors
// ============================================================================

/// Verified organization access with parameterized scope.
pub struct OrgAccess<Scope: ScopeLevel = Read> {
    pub org_id: String,
    pub auth: AuthContext,
    _scope: PhantomData<Scope>,
}

/// Type alias for org access with Read scope
pub type OrgRead = OrgAccess<Read>;

/// Type alias for org access with Write scope
pub type OrgWrite = OrgAccess<Write>;

/// Type alias for org access with Full scope
pub type OrgFull = OrgAccess<Full>;

#[derive(Deserialize)]
struct OrgParams {
    org_id: String,
}

impl<S, Scope> FromRequestParts<S> for OrgAccess<Scope>
where
    S: Send + Sync,
    Scope: ScopeLevel,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Path(params) = Path::<OrgParams>::from_request_parts(parts, _state)
            .await
            .map_err(|e| AuthRejection::Path(ValidationRejection::Path(e)))?;

        if !is_valid_id(&params.org_id) {
            return Err(AuthRejection::Path(ValidationRejection::InvalidOrgId));
        }

        let (auth, auth_service) = extract_auth(parts)?;
        auth_service
            .verify_org_access(&auth, &params.org_id, Scope::SCOPE)
            .await?;

        Ok(Self {
            org_id: params.org_id,
            auth,
            _scope: PhantomData,
        })
    }
}

// ============================================================================
// Role-Based Organization Access Extractors
// ============================================================================

/// Marker trait for role requirements
pub trait RoleLevel: Send + Sync + 'static {
    /// The minimum required role
    const ROLE: &'static str;
    /// The required API key scope
    const SCOPE: ApiKeyScope;
}

/// Admin role marker
pub struct Admin;
impl RoleLevel for Admin {
    const ROLE: &'static str = ORG_ROLE_ADMIN;
    const SCOPE: ApiKeyScope = ApiKeyScope::Full;
}

/// Owner role marker
pub struct Owner;
impl RoleLevel for Owner {
    const ROLE: &'static str = ORG_ROLE_OWNER;
    const SCOPE: ApiKeyScope = ApiKeyScope::Full;
}

/// Verified organization access with role requirement.
///
/// Extracts org_id from path, verifies authentication, scope, and minimum role.
///
/// # Example
///
/// ```no_run
/// # use sideseat_server::api::auth::OrgAdmin;
/// # use sideseat_server::api::types::ApiError;
/// pub async fn update_org(auth: OrgAdmin) -> Result<(), ApiError> {
///     let org_id = &auth.org_id;
///     Ok(())
/// }
/// ```
pub struct OrgRole<Role: RoleLevel> {
    pub org_id: String,
    pub auth: AuthContext,
    _role: PhantomData<Role>,
}

/// Type alias for org access requiring admin role
pub type OrgAdmin = OrgRole<Admin>;

/// Type alias for org access requiring owner role
pub type OrgOwner = OrgRole<Owner>;

impl<S, Role> FromRequestParts<S> for OrgRole<Role>
where
    S: Send + Sync,
    Role: RoleLevel,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Path(params) = Path::<OrgParams>::from_request_parts(parts, _state)
            .await
            .map_err(|e| AuthRejection::Path(ValidationRejection::Path(e)))?;

        if !is_valid_id(&params.org_id) {
            return Err(AuthRejection::Path(ValidationRejection::InvalidOrgId));
        }

        let (auth, auth_service) = extract_auth(parts)?;
        auth_service
            .verify_org_role(&auth, &params.org_id, Role::SCOPE, Role::ROLE)
            .await?;

        Ok(Self {
            org_id: params.org_id,
            auth,
            _role: PhantomData,
        })
    }
}

// ============================================================================
// Simple Auth Extractor (no path parameters)
// ============================================================================

/// Simple authenticated context extractor.
///
/// Use for routes that need authentication but don't have resource IDs in path.
/// Example: `GET /api/v1/organizations` (list user's orgs)
///
/// # Example
///
/// ```no_run
/// # use sideseat_server::api::auth::Auth;
/// # use sideseat_server::api::types::ApiError;
/// pub async fn list_organizations(auth: Auth) -> Result<(), ApiError> {
///     let user_id = auth.require_user_id()?;
///     Ok(())
/// }
/// ```
pub struct Auth {
    pub ctx: AuthContext,
}

impl Auth {
    /// Get user_id, returning error if not available
    pub fn require_user_id(&self) -> Result<&str, ApiError> {
        self.ctx.require_user_id()
    }

    /// Get user_id if available
    pub fn user_id(&self) -> Option<&str> {
        self.ctx.user_id()
    }
}

impl<S> FromRequestParts<S> for Auth
where
    S: Send + Sync,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let ctx = parts
            .extensions
            .get::<AuthContext>()
            .cloned()
            .ok_or(AuthRejection::MissingContext)?;

        Ok(Self { ctx })
    }
}
