//! Authentication API endpoints
//!
//! Provides endpoints for:
//! - Token exchange (bootstrap token -> JWT session)
//! - Auth status check
//! - Logout

use crate::auth::{AuthManager, SESSION_COOKIE_NAME};
use crate::core::constants::DEFAULT_SESSION_TTL_DAYS;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Request body for token exchange
#[derive(Debug, Deserialize)]
pub struct ExchangeRequest {
    /// The bootstrap token from the terminal URL
    pub token: String,
}

/// Response for token exchange
#[derive(Debug, Serialize)]
pub struct ExchangeResponse {
    pub success: bool,
    pub message: String,
}

/// Response for auth status
#[derive(Debug, Serialize)]
pub struct AuthStatusResponse {
    pub authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// Create auth routes
pub fn create_routes(auth_manager: Arc<AuthManager>) -> Router {
    Router::new()
        .route("/exchange", post(exchange_token))
        .route("/status", get(auth_status))
        .route("/logout", post(logout))
        .with_state(auth_manager)
}

/// Exchange a bootstrap token for a JWT session token
///
/// POST /api/v1/auth/exchange
/// Body: { "token": "bootstrap_token_here" }
/// Response: 200 + Set-Cookie OR 401 { "error": "invalid_token" }
async fn exchange_token(
    State(auth_manager): State<Arc<AuthManager>>,
    jar: CookieJar,
    Json(request): Json<ExchangeRequest>,
) -> Result<(CookieJar, Json<ExchangeResponse>), impl IntoResponse> {
    // Exchange bootstrap token for JWT
    let jwt = match auth_manager.exchange_token(&request.token) {
        Ok(jwt) => jwt,
        Err(_) => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "unauthorized",
                    "code": "BOOTSTRAP_INVALID",
                    "message": "Invalid bootstrap token"
                })),
            ));
        }
    };

    // Create session cookie
    let cookie = Cookie::build((SESSION_COOKIE_NAME, jwt))
        .http_only(true)
        .same_site(SameSite::Strict)
        .path("/api")
        .max_age(time::Duration::days(DEFAULT_SESSION_TTL_DAYS as i64))
        .build();

    let response =
        ExchangeResponse { success: true, message: "Authentication successful".to_string() };

    Ok((jar.add(cookie), Json(response)))
}

/// Check authentication status
///
/// GET /api/v1/auth/status
/// Response: { "authenticated": bool, "auth_method": string?, "expires_at": string? }
async fn auth_status(
    State(auth_manager): State<Arc<AuthManager>>,
    jar: CookieJar,
) -> Json<AuthStatusResponse> {
    // If auth is disabled, always return authenticated
    if !auth_manager.is_enabled() {
        return Json(AuthStatusResponse {
            authenticated: true,
            auth_method: Some("disabled".to_string()),
            expires_at: None,
        });
    }

    // Try to get session cookie
    let Some(cookie) = jar.get(SESSION_COOKIE_NAME) else {
        return Json(AuthStatusResponse {
            authenticated: false,
            auth_method: None,
            expires_at: None,
        });
    };

    // Validate the JWT
    match auth_manager.validate_session(cookie.value()) {
        Ok(claims) => {
            let expires_at =
                chrono::DateTime::from_timestamp(claims.exp, 0).map(|dt| dt.to_rfc3339());

            Json(AuthStatusResponse {
                authenticated: true,
                auth_method: Some(claims.auth_method),
                expires_at,
            })
        }
        Err(_) => {
            Json(AuthStatusResponse { authenticated: false, auth_method: None, expires_at: None })
        }
    }
}

/// Logout - clear the session cookie
///
/// POST /api/v1/auth/logout
/// Response: 200 + Clear-Cookie
async fn logout(jar: CookieJar) -> (CookieJar, Json<serde_json::Value>) {
    // Create an expired cookie to clear the session
    let cookie = Cookie::build((SESSION_COOKIE_NAME, ""))
        .http_only(true)
        .same_site(SameSite::Strict)
        .path("/api")
        .max_age(time::Duration::seconds(0))
        .build();

    (
        jar.remove(cookie),
        Json(serde_json::json!({
            "success": true,
            "message": "Logged out successfully"
        })),
    )
}
