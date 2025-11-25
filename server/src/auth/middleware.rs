//! Authentication middleware for Axum
//!
//! Provides request authentication via JWT session cookies.

use super::jwt::SessionClaims;
use super::manager::AuthManager;
use axum::{
    Json,
    body::Body,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use serde_json::json;
use std::sync::Arc;

/// Cookie name for the session token
pub const SESSION_COOKIE_NAME: &str = "sideseat_session";

/// Authentication error response
#[derive(Debug)]
pub struct AuthError {
    pub code: &'static str,
    pub message: String,
}

impl AuthError {
    pub fn required() -> Self {
        Self {
            code: "AUTH_REQUIRED",
            message: "Authentication required. Please authenticate via the terminal URL."
                .to_string(),
        }
    }

    pub fn expired() -> Self {
        Self {
            code: "TOKEN_EXPIRED",
            message: "Session has expired. Please re-authenticate.".to_string(),
        }
    }

    pub fn invalid() -> Self {
        Self { code: "TOKEN_INVALID", message: "Invalid session token.".to_string() }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": "unauthorized",
            "code": self.code,
            "message": self.message,
        });

        (StatusCode::UNAUTHORIZED, Json(body)).into_response()
    }
}

/// Authentication middleware
///
/// Validates the session cookie and injects claims into request extensions.
/// If authentication is disabled, all requests pass through.
pub async fn require_auth(
    State(auth_manager): State<Arc<AuthManager>>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    // If auth is disabled, pass through
    if !auth_manager.is_enabled() {
        return Ok(next.run(request).await);
    }

    // Validate Origin header for CSRF protection (if present)
    if let Some(origin) = request.headers().get(header::ORIGIN)
        && let Ok(origin_str) = origin.to_str()
    {
        // Allow same-origin requests
        // In production, this should check against configured allowed origins
        if !is_allowed_origin(origin_str) {
            tracing::warn!("Rejected request from disallowed origin: {}", origin_str);
            return Err(AuthError {
                code: "ORIGIN_NOT_ALLOWED",
                message: "Request origin not allowed".to_string(),
            });
        }
    }

    // Extract session cookie
    let session_cookie = jar.get(SESSION_COOKIE_NAME).ok_or_else(AuthError::required)?;

    let jwt = session_cookie.value();

    // Validate JWT
    let claims = auth_manager.validate_session(jwt).map_err(|e| {
        let err_msg = e.to_string();
        if err_msg.contains("expired") { AuthError::expired() } else { AuthError::invalid() }
    })?;

    // Inject claims into request extensions for handlers to access
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

/// Check if an origin is allowed
///
/// For localhost development, we allow localhost and 127.0.0.1 on any port.
fn is_allowed_origin(origin: &str) -> bool {
    // Parse the origin URL
    if let Ok(url) = url::Url::parse(origin)
        && let Some(host) = url.host_str()
    {
        // Allow localhost origins
        return host == "localhost" || host == "127.0.0.1";
    }
    false
}

/// Extension trait to extract session claims from request
pub trait SessionClaimsExt {
    fn session_claims(&self) -> Option<&SessionClaims>;
}

impl SessionClaimsExt for Request<Body> {
    fn session_claims(&self) -> Option<&SessionClaims> {
        self.extensions().get::<SessionClaims>()
    }
}

// URL parsing helper - using simple parsing to avoid adding url crate dependency
mod url {
    pub struct Url {
        host: Option<String>,
    }

    impl Url {
        pub fn parse(s: &str) -> Result<Self, ()> {
            // Simple URL parsing: http://host:port/path
            let s = s.strip_prefix("http://").or_else(|| s.strip_prefix("https://")).ok_or(())?;
            let host_port = s.split('/').next().ok_or(())?;
            let host = host_port.split(':').next().ok_or(())?;
            Ok(Self { host: Some(host.to_string()) })
        }

        pub fn host_str(&self) -> Option<&str> {
            self.host.as_deref()
        }
    }
}
