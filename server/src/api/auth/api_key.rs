//! API key authentication and validation
//!
//! This module provides functions for validating API keys in requests.
//! Keys are org-scoped and can access any project within their organization.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::core::constants::{API_KEY_TOUCH_DEBOUNCE_SECS, DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM};
use crate::data::TransactionalService;
use crate::data::cache::{CacheService, RateLimitBucket, RateLimiter};
use crate::data::types::{ApiKeyScope, ApiKeyValidation};
use crate::utils::api_key::{extract_key_from_header, hash_api_key, is_valid_api_key};

/// API key authentication error
#[derive(Debug)]
pub enum ApiKeyAuthError {
    /// No auth header provided
    Missing,
    /// Malformed auth header or key format
    InvalidFormat,
    /// Key doesn't exist or wrong org/project
    InvalidKey,
    /// Key has expired
    Expired,
    /// Key doesn't have required permission
    InsufficientScope,
    /// Too many failed auth attempts
    RateLimited,
}

impl fmt::Display for ApiKeyAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "Authorization header required"),
            Self::InvalidFormat => write!(f, "Invalid authorization header format"),
            Self::InvalidKey => write!(f, "Invalid or expired API key"),
            Self::Expired => write!(f, "API key has expired"),
            Self::InsufficientScope => write!(f, "API key lacks required permission"),
            Self::RateLimited => write!(f, "Too many failed auth attempts"),
        }
    }
}

impl IntoResponse for ApiKeyAuthError {
    fn into_response(self) -> Response {
        match self {
            // Scope error returns 403 Forbidden (key is valid but lacks permission)
            ApiKeyAuthError::InsufficientScope => {
                let body = json!({
                    "error": "forbidden",
                    "code": "INSUFFICIENT_SCOPE",
                    "message": "API key lacks required permission",
                });
                (StatusCode::FORBIDDEN, Json(body)).into_response()
            }
            // Rate limited returns 429 Too Many Requests
            ApiKeyAuthError::RateLimited => {
                let body = json!({
                    "error": "rate_limited",
                    "code": "AUTH_RATE_LIMITED",
                    "message": "Too many failed authentication attempts",
                });
                (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response()
            }
            // All other errors return 401 (don't leak info about key validity)
            _ => {
                let body = json!({
                    "error": "unauthorized",
                    "code": "API_KEY_INVALID",
                    "message": "Invalid or expired API key",
                });
                (StatusCode::UNAUTHORIZED, Json(body)).into_response()
            }
        }
    }
}

/// Validate API key for project-scoped endpoints.
/// Verifies the project belongs to the key's organization.
pub async fn validate_api_key_for_project(
    cache: &CacheService,
    database: Arc<TransactionalService>,
    api_key_secret: &[u8],
    auth_header: &str,
    project_id: &str,
    required_scope: ApiKeyScope,
) -> Result<ApiKeyValidation, ApiKeyAuthError> {
    let validation = validate_api_key_core(cache, &database, api_key_secret, auth_header).await?;

    // Verify project belongs to the key's org
    let project = database
        .repository()
        .get_project(Some(cache), project_id)
        .await
        .map_err(|e| {
            tracing::error!(project_id = %project_id, error = %e, "Database error during project lookup");
            ApiKeyAuthError::InvalidKey
        })?
        .ok_or(ApiKeyAuthError::InvalidKey)?;

    if project.organization_id != validation.org_id {
        tracing::debug!(project_id = %project_id, "API key org does not match project org");
        return Err(ApiKeyAuthError::InvalidKey);
    }

    check_expiry_and_scope(&validation, required_scope)?;
    touch_if_needed(database, &validation);
    Ok(validation)
}

/// Validate API key for general endpoints (no org/project validation).
/// Returns key's org_id in validation result.
pub async fn validate_api_key_general(
    cache: &CacheService,
    database: Arc<TransactionalService>,
    api_key_secret: &[u8],
    auth_header: &str,
    required_scope: ApiKeyScope,
) -> Result<ApiKeyValidation, ApiKeyAuthError> {
    let validation = validate_api_key_core(cache, &database, api_key_secret, auth_header).await?;
    check_expiry_and_scope(&validation, required_scope)?;
    touch_if_needed(database, &validation);
    Ok(validation)
}

/// Core validation: parse header, hash, lookup (caching handled by repository)
async fn validate_api_key_core(
    cache: &CacheService,
    database: &TransactionalService,
    api_key_secret: &[u8],
    auth_header: &str,
) -> Result<ApiKeyValidation, ApiKeyAuthError> {
    let key = extract_key_from_header(auth_header).ok_or(ApiKeyAuthError::InvalidFormat)?;

    if !is_valid_api_key(&key) {
        return Err(ApiKeyAuthError::InvalidFormat);
    }

    let key_hash = hash_api_key(&key, api_key_secret);

    // Repository handles caching (positive + negative)
    database
        .repository()
        .get_api_key_by_hash(Some(cache), &key_hash)
        .await
        .map_err(|_| ApiKeyAuthError::InvalidKey)?
        .ok_or(ApiKeyAuthError::InvalidKey)
}

/// Check if key is expired and has required scope
fn check_expiry_and_scope(
    v: &ApiKeyValidation,
    required: ApiKeyScope,
) -> Result<(), ApiKeyAuthError> {
    if let Some(exp) = v.expires_at
        && chrono::Utc::now().timestamp() > exp
    {
        return Err(ApiKeyAuthError::Expired);
    }
    if !v.scope.has_permission(required) {
        return Err(ApiKeyAuthError::InsufficientScope);
    }
    Ok(())
}

/// Update last_used_at if not recently touched (debounced)
pub(crate) fn touch_if_needed(database: Arc<TransactionalService>, v: &ApiKeyValidation) {
    let should_touch = v
        .last_used_at
        .map(|t| chrono::Utc::now().timestamp() - t > API_KEY_TOUCH_DEBOUNCE_SECS as i64)
        .unwrap_or(true);

    if should_touch {
        let key_id = v.key_id.clone();
        tokio::spawn(async move {
            if let Err(e) = database
                .repository()
                .touch_api_key(&key_id, API_KEY_TOUCH_DEBOUNCE_SECS)
                .await
            {
                tracing::warn!(key_id = %key_id, error = %e, "Failed to update API key last_used_at");
            }
        });
    }
}

/// State for OTEL auth middleware
#[derive(Clone)]
pub struct OtelAuthState {
    pub database: Arc<TransactionalService>,
    pub cache: Arc<CacheService>,
    pub api_key_secret: Vec<u8>,
    pub otel_auth_required: bool,
    pub rate_limiter: Option<Arc<RateLimiter>>,
}

/// OTEL ingestion auth middleware
///
/// When `otel_auth_required` is true, validates that requests have a valid API key
/// with `ingest` scope and that the key's org owns the target project.
///
/// Also tracks auth failures per IP for brute force protection.
pub async fn otel_auth_middleware(
    State(state): State<OtelAuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Result<Response, ApiKeyAuthError> {
    // Skip if OTEL auth not required
    if !state.otel_auth_required {
        return Ok(next.run(request).await);
    }

    // Get client IP for rate limiting (prefer X-Forwarded-For for proxied requests)
    let client_ip = get_client_ip(&request, addr);

    // Check if IP is rate limited due to too many auth failures (without incrementing)
    if let (Some(limiter), Some(ip)) = (&state.rate_limiter, &client_ip) {
        let bucket = RateLimitBucket::auth_failures(DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM);
        if limiter.is_blocked(&bucket, ip).await {
            tracing::warn!(ip = %ip, "OTEL auth blocked due to too many failures");
            return Err(ApiKeyAuthError::RateLimited);
        }
    }

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or(ApiKeyAuthError::Missing)?;

    // Extract project_id from path: /otel/{project_id}/v1/...
    let project_id = request
        .uri()
        .path()
        .strip_prefix("/otel/")
        .and_then(|p| p.split('/').next())
        .ok_or(ApiKeyAuthError::InvalidFormat)?;

    // OTEL ingestion requires 'ingest' scope
    // Key's org must own the project
    let result = validate_api_key_for_project(
        &state.cache,
        state.database.clone(),
        &state.api_key_secret,
        auth_header,
        project_id,
        ApiKeyScope::Ingest,
    )
    .await;

    // Track auth failures for rate limiting (increment counter on failure only)
    if let Err(ref e) = result
        && matches!(e, ApiKeyAuthError::InvalidKey | ApiKeyAuthError::Expired)
        && let (Some(limiter), Some(ip)) = (&state.rate_limiter, &client_ip)
    {
        let bucket = RateLimitBucket::auth_failures(DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM);
        let _ = limiter.check(&bucket, ip).await;
        tracing::debug!(ip = %ip, "OTEL auth failure tracked");
    }

    result?;
    Ok(next.run(request).await)
}

/// Extract client IP from request headers or connection info
///
/// # Security Note
///
/// This function trusts the X-Forwarded-For header, which can be spoofed by
/// malicious clients if the server is directly exposed to the internet.
/// This is acceptable here because:
/// 1. This IP is only used for auth failure rate limiting (defense-in-depth)
/// 2. The primary protection is API key entropy (2^259 bits)
/// 3. Typical deployments are behind a trusted reverse proxy that overwrites XFF
///
/// For stricter rate limiting, deploy behind a reverse proxy that sets
/// X-Forwarded-For correctly and strips client-provided values.
fn get_client_ip(request: &Request, addr: SocketAddr) -> Option<String> {
    // Prefer X-Forwarded-For for proxied requests (first IP only)
    request
        .headers()
        .get("X-Forwarded-For")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| Some(addr.ip().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;

    fn make_validation(scope: ApiKeyScope, expires_at: Option<i64>) -> ApiKeyValidation {
        ApiKeyValidation {
            key_id: "test-key".to_string(),
            org_id: "test-org".to_string(),
            scope,
            created_by: Some("test-user".to_string()),
            expires_at,
            last_used_at: None,
        }
    }

    #[test]
    fn test_api_key_auth_error_display() {
        assert_eq!(
            ApiKeyAuthError::Missing.to_string(),
            "Authorization header required"
        );
        assert_eq!(
            ApiKeyAuthError::InvalidFormat.to_string(),
            "Invalid authorization header format"
        );
        assert_eq!(
            ApiKeyAuthError::InvalidKey.to_string(),
            "Invalid or expired API key"
        );
        assert_eq!(ApiKeyAuthError::Expired.to_string(), "API key has expired");
        assert_eq!(
            ApiKeyAuthError::InsufficientScope.to_string(),
            "API key lacks required permission"
        );
        assert_eq!(
            ApiKeyAuthError::RateLimited.to_string(),
            "Too many failed auth attempts"
        );
    }

    #[test]
    fn test_check_expiry_valid() {
        let future = chrono::Utc::now().timestamp() + 3600; // 1 hour from now
        let v = make_validation(ApiKeyScope::Full, Some(future));
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Read).is_ok());
    }

    #[test]
    fn test_check_expiry_expired() {
        let past = chrono::Utc::now().timestamp() - 3600; // 1 hour ago
        let v = make_validation(ApiKeyScope::Full, Some(past));
        let result = check_expiry_and_scope(&v, ApiKeyScope::Read);
        assert!(matches!(result, Err(ApiKeyAuthError::Expired)));
    }

    #[test]
    fn test_check_expiry_no_expiry() {
        let v = make_validation(ApiKeyScope::Full, None);
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Read).is_ok());
    }

    #[test]
    fn test_check_scope_full_grants_all() {
        let v = make_validation(ApiKeyScope::Full, None);
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Read).is_ok());
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Write).is_ok());
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Ingest).is_ok());
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Full).is_ok());
    }

    #[test]
    fn test_check_scope_read_only() {
        let v = make_validation(ApiKeyScope::Read, None);
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Read).is_ok());
        assert!(matches!(
            check_expiry_and_scope(&v, ApiKeyScope::Write),
            Err(ApiKeyAuthError::InsufficientScope)
        ));
    }

    #[test]
    fn test_check_scope_ingest_only() {
        let v = make_validation(ApiKeyScope::Ingest, None);
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Ingest).is_ok());
        assert!(matches!(
            check_expiry_and_scope(&v, ApiKeyScope::Read),
            Err(ApiKeyAuthError::InsufficientScope)
        ));
    }

    #[test]
    fn test_get_client_ip_from_socket() {
        let request = HttpRequest::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        let ip = get_client_ip(&request, addr);
        assert_eq!(ip, Some("192.168.1.1".to_string()));
    }

    #[test]
    fn test_get_client_ip_from_xff_header() {
        let request = HttpRequest::builder()
            .uri("/test")
            .header("X-Forwarded-For", "10.0.0.1")
            .body(Body::empty())
            .unwrap();
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        let ip = get_client_ip(&request, addr);
        assert_eq!(ip, Some("10.0.0.1".to_string()));
    }

    #[test]
    fn test_get_client_ip_from_xff_multiple() {
        let request = HttpRequest::builder()
            .uri("/test")
            .header("X-Forwarded-For", "10.0.0.1, 10.0.0.2, 10.0.0.3")
            .body(Body::empty())
            .unwrap();
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        let ip = get_client_ip(&request, addr);
        assert_eq!(ip, Some("10.0.0.1".to_string()));
    }

    #[test]
    fn test_get_client_ip_xff_with_whitespace() {
        let request = HttpRequest::builder()
            .uri("/test")
            .header("X-Forwarded-For", "  10.0.0.1  , 10.0.0.2")
            .body(Body::empty())
            .unwrap();
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        let ip = get_client_ip(&request, addr);
        assert_eq!(ip, Some("10.0.0.1".to_string()));
    }
}
