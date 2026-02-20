//! Authentication middleware

use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use serde_json::json;

use super::api_key::{ApiKeyAuthError, validate_api_key_general};
use super::context::{AuthContext, AuthService};
use super::jwt::JwtError;
use super::manager::AuthManager;
use crate::api::middleware::AllowedOrigins;
use crate::core::constants::{DEFAULT_USER_ID, SESSION_COOKIE_NAME};
use crate::data::TransactionalService;
use crate::data::cache::CacheService;
use crate::data::types::ApiKeyScope;

/// Authentication error response
#[derive(Debug)]
pub struct AuthError {
    pub status: StatusCode,
    pub error: &'static str,
    pub code: &'static str,
    pub message: String,
}

impl AuthError {
    pub fn required() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            error: "unauthorized",
            code: "AUTH_REQUIRED",
            message: "Authentication required".to_string(),
        }
    }

    pub fn expired() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            error: "unauthorized",
            code: "TOKEN_EXPIRED",
            message: "Session has expired".to_string(),
        }
    }

    pub fn invalid() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            error: "unauthorized",
            code: "TOKEN_INVALID",
            message: "Invalid session token".to_string(),
        }
    }

    pub fn invalid_api_key() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            error: "unauthorized",
            code: "API_KEY_INVALID",
            message: "Invalid or expired API key".to_string(),
        }
    }

    pub fn insufficient_scope() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            error: "forbidden",
            code: "INSUFFICIENT_SCOPE",
            message: "API key lacks required permission".to_string(),
        }
    }

    pub fn origin_not_allowed() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            error: "unauthorized",
            code: "ORIGIN_NOT_ALLOWED",
            message: "Request origin not allowed".to_string(),
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": self.error,
            "code": self.code,
            "message": self.message,
        });
        (self.status, Json(body)).into_response()
    }
}

/// Shared auth state for middleware
#[derive(Clone)]
pub struct AuthState {
    pub auth_manager: Arc<AuthManager>,
    pub allowed_origins: AllowedOrigins,
    /// Database service for API key validation
    pub database: Arc<TransactionalService>,
    /// Cache service for API key validation
    pub cache: Arc<CacheService>,
    /// API key HMAC secret
    pub api_key_secret: Vec<u8>,
}

/// Authentication middleware
///
/// Supports both API key and JWT session authentication.
/// API key takes precedence if Authorization header is present.
///
/// Injects into request extensions:
/// - `AuthContext` - unified auth context for all auth methods
/// - `Arc<AuthService>` - cached authorization service
pub async fn require_auth(
    State(state): State<AuthState>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    // Create AuthService for authorization checks (injected for extractors)
    let auth_service = Arc::new(AuthService::new(
        state.database.clone(),
        state.cache.clone(),
    ));
    request.extensions_mut().insert(auth_service);

    // ========== API KEY CHECK (runs regardless of auth mode) ==========
    if let Some(auth_header) = request.headers().get(header::AUTHORIZATION)
        && let Ok(header_str) = auth_header.to_str()
        && (header_str.starts_with("Basic ") || header_str.starts_with("Bearer "))
    {
        // Validate API key (minimum Read scope to be valid)
        let validation = validate_api_key_general(
            &state.cache,
            state.database.clone(),
            &state.api_key_secret,
            header_str,
            ApiKeyScope::Read,
        )
        .await
        .map_err(|e| match e {
            ApiKeyAuthError::InsufficientScope => AuthError::insufficient_scope(),
            _ => AuthError::invalid_api_key(),
        })?;

        // Inject unified AuthContext
        let auth_ctx = AuthContext::ApiKey {
            key_id: validation.key_id,
            org_id: validation.org_id,
            scope: validation.scope,
            created_by: validation.created_by,
        };
        request.extensions_mut().insert(auth_ctx);

        return Ok(next.run(request).await);
    }

    // ========== NO API KEY - CHECK AUTH MODE ==========
    if !state.auth_manager.is_enabled() {
        // Auth disabled: inject default context
        let auth_ctx = AuthContext::LocalDefault {
            user_id: DEFAULT_USER_ID.to_string(),
        };
        request.extensions_mut().insert(auth_ctx);

        return Ok(next.run(request).await);
    }

    // ========== AUTH ENABLED - VALIDATE JWT ==========
    // Validate Origin header for CSRF protection (with Referer fallback)
    let origin_to_check = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            request
                .headers()
                .get(header::REFERER)
                .and_then(|v| v.to_str().ok())
                .and_then(|referer| {
                    // Extract origin from Referer URL (scheme + host + port)
                    match reqwest::Url::parse(referer) {
                        Ok(u) => match u.host_str() {
                            Some(host) => {
                                let origin = match u.port() {
                                    Some(port) => format!("{}://{}:{}", u.scheme(), host, port),
                                    None => format!("{}://{}", u.scheme(), host),
                                };
                                Some(origin)
                            }
                            None => {
                                tracing::warn!(referer = %referer, "Referer URL has no host");
                                None
                            }
                        },
                        Err(_) => {
                            tracing::debug!(referer = %referer, "Failed to parse Referer URL");
                            None
                        }
                    }
                })
        });

    if let Some(origin_str) = origin_to_check
        && !state.allowed_origins.is_allowed(&origin_str)
    {
        tracing::warn!("Rejected request from disallowed origin: {}", origin_str);
        return Err(AuthError::origin_not_allowed());
    }

    let session_cookie = jar
        .get(SESSION_COOKIE_NAME)
        .ok_or_else(AuthError::required)?;
    let jwt = session_cookie.value();

    let claims = state
        .auth_manager
        .validate_session(jwt)
        .map_err(|e| match e {
            JwtError::Expired => AuthError::expired(),
            _ => AuthError::invalid(),
        })?;

    // Inject unified AuthContext
    let auth_ctx = AuthContext::Session {
        user_id: claims.user_id().to_string(),
    };
    request.extensions_mut().insert(auth_ctx);

    Ok(next.run(request).await)
}
