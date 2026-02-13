//! Authentication API endpoints

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::api::auth::AuthManager;
use crate::api::extractors::ValidatedJson;
use crate::api::middleware::AllowedOrigins;
use crate::core::constants::{DEFAULT_SESSION_TTL_DAYS, DEFAULT_USER_ID, SESSION_COOKIE_NAME};
use crate::data::TransactionalService;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ExchangeRequest {
    #[validate(length(min = 1, message = "Token cannot be empty"))]
    pub token: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExchangeResponse {
    pub success: bool,
    pub message: String,
}

/// User info in auth status response
#[derive(Debug, Serialize, ToSchema)]
pub struct UserDto {
    pub id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthStatusResponse {
    pub authenticated: bool,
    pub version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<UserDto>,
}

/// Auth state with database access
#[derive(Clone)]
pub struct AuthRoutesState {
    pub auth_manager: Arc<AuthManager>,
    pub allowed_origins: AllowedOrigins,
    pub database: Arc<TransactionalService>,
}

/// Create auth routes
pub fn routes(
    auth_manager: Arc<AuthManager>,
    allowed_origins: AllowedOrigins,
    database: Arc<TransactionalService>,
) -> Router {
    let state = AuthRoutesState {
        auth_manager,
        allowed_origins,
        database,
    };

    Router::new()
        .route("/exchange", post(exchange_token))
        .route("/status", get(auth_status))
        .route("/logout", post(logout))
        .with_state(state)
}

/// Exchange bootstrap token for JWT session
#[utoipa::path(
    post,
    path = "/api/v1/auth/exchange",
    tag = "auth",
    request_body = ExchangeRequest,
    responses(
        (status = 200, description = "Token exchanged successfully", body = ExchangeResponse),
        (status = 401, description = "Invalid bootstrap token")
    )
)]
pub async fn exchange_token(
    State(state): State<AuthRoutesState>,
    jar: CookieJar,
    ValidatedJson(request): ValidatedJson<ExchangeRequest>,
) -> Result<(CookieJar, Json<ExchangeResponse>), impl IntoResponse> {
    let jwt = match state.auth_manager.exchange_token(&request.token) {
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

    let cookie = Cookie::build((SESSION_COOKIE_NAME, jwt))
        .http_only(true)
        .same_site(SameSite::Strict)
        .path("/api")
        .max_age(time::Duration::days(DEFAULT_SESSION_TTL_DAYS as i64))
        .build();

    let response = ExchangeResponse {
        success: true,
        message: "Authentication successful".to_string(),
    };

    Ok((jar.add(cookie), Json(response)))
}

/// Check authentication status (returns user profile when authenticated)
#[utoipa::path(
    get,
    path = "/api/v1/auth/status",
    tag = "auth",
    responses(
        (status = 200, description = "Authentication status", body = AuthStatusResponse)
    )
)]
pub async fn auth_status(
    State(state): State<AuthRoutesState>,
    jar: CookieJar,
) -> Json<AuthStatusResponse> {
    const VERSION: &str = env!("CARGO_PKG_VERSION");

    // Auth disabled: return default local user
    if !state.auth_manager.is_enabled() {
        let repo = state.database.repository();
        let user = repo
            .get_user(None, DEFAULT_USER_ID)
            .await
            .ok()
            .flatten()
            .map(|u| UserDto {
                id: u.id,
                email: u.email,
                display_name: u.display_name,
            });

        return Json(AuthStatusResponse {
            authenticated: true,
            version: VERSION,
            auth_method: Some("disabled".to_string()),
            expires_at: None,
            user,
        });
    }

    // No session cookie
    let Some(cookie) = jar.get(SESSION_COOKIE_NAME) else {
        return Json(AuthStatusResponse {
            authenticated: false,
            version: VERSION,
            auth_method: None,
            expires_at: None,
            user: None,
        });
    };

    // Validate JWT and get user
    match state.auth_manager.validate_session(cookie.value()) {
        Ok(claims) => {
            let expires_at = DateTime::from_timestamp(claims.exp, 0);

            // Fetch user from database
            let repo = state.database.repository();
            let user = repo
                .get_user(None, claims.user_id())
                .await
                .ok()
                .flatten()
                .map(|u| UserDto {
                    id: u.id,
                    email: u.email,
                    display_name: u.display_name,
                });

            Json(AuthStatusResponse {
                authenticated: true,
                version: VERSION,
                auth_method: Some(claims.auth_method),
                expires_at,
                user,
            })
        }
        Err(_) => Json(AuthStatusResponse {
            authenticated: false,
            version: VERSION,
            auth_method: None,
            expires_at: None,
            user: None,
        }),
    }
}

/// Logout - clear session cookie
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    responses(
        (status = 200, description = "Logged out successfully")
    )
)]
pub async fn logout(jar: CookieJar) -> (CookieJar, Json<serde_json::Value>) {
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
