//! Authentication module
//!
//! Provides secure authentication for the SideSeat API:
//!
//! - Bootstrap token authentication via terminal URL
//! - JWT session tokens stored in HttpOnly cookies
//! - Middleware for protecting API routes
//!
//! ## Usage
//!
//! ```rust,ignore
//! use sideseat::auth::{AuthManager, require_auth};
//!
//! // Initialize auth manager
//! let auth = AuthManager::init(&secrets, true).await?;
//!
//! // Get bootstrap token for terminal display
//! let token = auth.bootstrap_token();
//!
//! // Apply middleware to protected routes
//! let protected = Router::new()
//!     .route("/api/protected", get(handler))
//!     .layer(middleware::from_fn_with_state(
//!         auth.clone(),
//!         require_auth
//!     ));
//! ```

mod bootstrap;
pub mod jwt;
mod manager;
pub mod middleware;

pub use jwt::SessionClaims;
pub use manager::AuthManager;
pub use middleware::{AuthError, SESSION_COOKIE_NAME, SessionClaimsExt, require_auth};
