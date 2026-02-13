//! Authentication module

pub mod api_key;
mod context;
mod extractors;
pub mod jwt;
mod manager;
pub mod middleware;

// Unified auth system
pub use context::{AuthContext, AuthService};
pub use extractors::{
    Admin, Auth, AuthRejection, Full, OrgAccess, OrgAdmin, OrgFull, OrgOwner, OrgRead, OrgRole,
    OrgWrite, Owner, ProjectAccess, ProjectFull, ProjectRead, ProjectWrite, Read, RoleLevel,
    ScopeLevel, SessionAccess, SessionRead, SpanAccess, SpanRead, TraceAccess, TraceRead, Write,
};

// OTEL auth middleware (for ingestion routes)
pub use api_key::{ApiKeyAuthError, OtelAuthState, otel_auth_middleware};

// Other exports
pub use jwt::SessionClaims;
pub use manager::AuthManager;
pub use middleware::{AuthError, AuthState, require_auth};
