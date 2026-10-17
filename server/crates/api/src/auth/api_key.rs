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

use sideseat_core::constants::{
    API_KEY_TOUCH_DEBOUNCE_SECS, DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM,
    DEFAULT_RATE_LIMIT_AUTH_UNAVAILABLE_RPM,
};
use sideseat_core::utils::api_key::{extract_key_from_header, hash_api_key, is_valid_api_key};
use sideseat_domain::rate_limit::{RateLimitBucket, RateLimiter};
use sideseat_ports::clock::Clock;
use sideseat_ports::types::{ApiKeyScope, ApiKeyValidation};

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
    /// The store the key is checked against failed: nothing is known about the key, so the request is neither
    /// accepted nor refused - it is answered 503 (gRPC `Unavailable`), which clients retry, and is not counted as
    /// a failed attempt.
    Unavailable,
}

impl ApiKeyAuthError {
    /// Whether this refusal counts towards the per-client failed-attempt limit: only a key the store answered for
    /// and found wanting. Both transports count with this, so they cannot disagree.
    pub fn counts_as_failed_attempt(&self) -> bool {
        matches!(self, Self::InvalidKey | Self::Expired)
    }

    /// The gRPC status of this refusal.
    pub fn grpc_status(&self) -> tonic::Status {
        match self {
            Self::Unavailable => {
                tonic::Status::unavailable("authentication is temporarily unavailable")
            }
            Self::RateLimited => {
                tonic::Status::resource_exhausted("too many failed authentication attempts")
            }
            Self::InsufficientScope => {
                tonic::Status::permission_denied("API key lacks required permission")
            }
            _ => tonic::Status::unauthenticated("invalid or unauthorized API key"),
        }
    }
}

/// A lookup's answer, boxed: the check runs inside handlers that must be `Send` for every lifetime.
type Lookup<'a, T> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<T, sideseat_ports::error::DataError>> + Send + 'a>,
>;

/// The two lookups an API key check makes. The transactional store answers them; a test answers them with a failure.
pub(crate) trait KeyLookups: Sync {
    fn api_key_by_hash<'a>(&'a self, key_hash: &'a str) -> Lookup<'a, Option<ApiKeyValidation>>;
    fn project<'a>(
        &'a self,
        project_id: &'a str,
    ) -> Lookup<'a, Option<sideseat_ports::types::ProjectRow>>;
}

impl KeyLookups for crate::dependencies::TransactionalStore {
    fn api_key_by_hash<'a>(&'a self, key_hash: &'a str) -> Lookup<'a, Option<ApiKeyValidation>> {
        self.get_api_key_by_hash(key_hash)
    }

    fn project<'a>(
        &'a self,
        project_id: &'a str,
    ) -> Lookup<'a, Option<sideseat_ports::types::ProjectRow>> {
        self.get_project(project_id)
    }
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
            Self::Unavailable => write!(f, "Authentication temporarily unavailable"),
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
            // The store failed: nothing is known about the key, and the client should retry.
            ApiKeyAuthError::Unavailable => {
                let body = json!({
                    "error": "service_unavailable",
                    "code": "AUTH_UNAVAILABLE",
                    "message": "Authentication is temporarily unavailable",
                });
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(header::RETRY_AFTER, "1")],
                    Json(body),
                )
                    .into_response()
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
    database: Arc<crate::dependencies::TransactionalStore>,
    api_key_secret: &[u8],
    auth_header: &str,
    project_id: &str,
    required_scope: ApiKeyScope,
    clock: &dyn Clock,
) -> Result<ApiKeyValidation, ApiKeyAuthError> {
    let (validation, key_hash) = authorize_for_project(
        database.as_ref(),
        api_key_secret,
        auth_header,
        project_id,
        required_scope,
        clock,
    )
    .await?;
    touch_if_needed(database, &validation, key_hash, clock.now().timestamp());
    Ok(validation)
}

/// The key, checked against the store, for `project_id`: the project's organisation must be the key's.
async fn authorize_for_project<S: KeyLookups + ?Sized>(
    store: &S,
    api_key_secret: &[u8],
    auth_header: &str,
    project_id: &str,
    required_scope: ApiKeyScope,
    clock: &dyn Clock,
) -> Result<(ApiKeyValidation, String), ApiKeyAuthError> {
    let (validation, key_hash) = validate_api_key_core(store, api_key_secret, auth_header).await?;

    // Verify project belongs to the key's org
    let project = store
        .project(project_id)
        .await
        .map_err(|e| {
            tracing::error!(project_id = %project_id, error = %e, "Database error during project lookup");
            ApiKeyAuthError::Unavailable
        })?
        .ok_or(ApiKeyAuthError::InvalidKey)?;

    if project.organization_id != validation.org_id {
        tracing::debug!(project_id = %project_id, "API key org does not match project org");
        return Err(ApiKeyAuthError::InvalidKey);
    }

    check_expiry_and_scope(&validation, required_scope, clock.now().timestamp())?;
    Ok((validation, key_hash))
}

/// Validate API key for general endpoints (no org/project validation).
/// Returns key's org_id in validation result.
pub async fn validate_api_key_general(
    database: Arc<crate::dependencies::TransactionalStore>,
    api_key_secret: &[u8],
    auth_header: &str,
    required_scope: ApiKeyScope,
    clock: &dyn Clock,
) -> Result<ApiKeyValidation, ApiKeyAuthError> {
    let (validation, key_hash) =
        validate_api_key_core(database.as_ref(), api_key_secret, auth_header).await?;
    let now = clock.now().timestamp();
    check_expiry_and_scope(&validation, required_scope, now)?;
    touch_if_needed(database, &validation, key_hash, now);
    Ok(validation)
}

/// Core validation: parse header, hash, lookup (caching handled by repository)
async fn validate_api_key_core<S: KeyLookups + ?Sized>(
    database: &S,
    api_key_secret: &[u8],
    auth_header: &str,
) -> Result<(ApiKeyValidation, String), ApiKeyAuthError> {
    let key = extract_key_from_header(auth_header).ok_or(ApiKeyAuthError::InvalidFormat)?;

    if !is_valid_api_key(&key) {
        return Err(ApiKeyAuthError::InvalidFormat);
    }

    let key_hash = hash_api_key(&key, api_key_secret);

    // Repository handles caching (positive + negative)
    let validation = database
        .api_key_by_hash(&key_hash)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Database error during API key lookup");
            ApiKeyAuthError::Unavailable
        })?
        .ok_or(ApiKeyAuthError::InvalidKey)?;
    Ok((validation, key_hash))
}

/// Check if key is expired and has required scope
fn check_expiry_and_scope(
    v: &ApiKeyValidation,
    required: ApiKeyScope,
    now: i64,
) -> Result<(), ApiKeyAuthError> {
    if let Some(exp) = v.expires_at
        && now > exp
    {
        return Err(ApiKeyAuthError::Expired);
    }
    if !v.scope.has_permission(required) {
        return Err(ApiKeyAuthError::InsufficientScope);
    }
    Ok(())
}

/// Update last_used_at if not recently touched (debounced)
pub(crate) fn touch_if_needed(
    database: Arc<crate::dependencies::TransactionalStore>,
    v: &ApiKeyValidation,
    key_hash: String,
    now: i64,
) {
    let should_touch = v
        .last_used_at
        .map(|t| now - t > API_KEY_TOUCH_DEBOUNCE_SECS as i64)
        .unwrap_or(true);

    if should_touch {
        let key_id = v.key_id.clone();
        tokio::spawn(async move {
            if let Err(e) = database
                .as_ref()
                .touch_api_key(&key_id, &key_hash, API_KEY_TOUCH_DEBOUNCE_SECS)
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
    pub database: Arc<crate::dependencies::TransactionalStore>,
    pub cache: Arc<crate::dependencies::SharedCache>,
    pub api_key_secret: Vec<u8>,
    pub otel_auth_required: bool,
    pub rate_limiter: Option<Arc<RateLimiter>>,
    /// Whose forwarded-for header may be believed - see `utils::client_ip`.
    pub trusted_proxies: Arc<sideseat_core::utils::client_ip::TrustedProxies>,
    pub clock: Arc<dyn Clock>,
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
    let client_ip = get_client_ip(&request, addr, &state.trusted_proxies);

    // Check if IP is rate limited due to too many auth failures (without incrementing)
    if let (Some(limiter), Some(ip)) = (&state.rate_limiter, &client_ip) {
        let bucket = RateLimitBucket::auth_failures(DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM);
        if limiter.is_blocked(&bucket, ip).await {
            tracing::warn!(ip = %ip, "OTEL auth blocked due to too many failures");
            return Err(ApiKeyAuthError::RateLimited);
        }
    }

    // A client whose checks the store has failed too often lately is answered "unavailable" without a lookup.
    let unavailable =
        RateLimitBucket::auth_unavailable("http", DEFAULT_RATE_LIMIT_AUTH_UNAVAILABLE_RPM);
    if let (Some(limiter), Some(ip)) = (&state.rate_limiter, &client_ip)
        && limiter.is_blocked(&unavailable, ip).await
    {
        return Err(ApiKeyAuthError::Unavailable);
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
        state.database.clone(),
        &state.api_key_secret,
        auth_header,
        project_id,
        ApiKeyScope::Ingest,
        state.clock.as_ref(),
    )
    .await;

    // Track auth failures for rate limiting (increment counter on failure only)
    if let Err(ref e) = result
        && e.counts_as_failed_attempt()
        && let (Some(limiter), Some(ip)) = (&state.rate_limiter, &client_ip)
    {
        let bucket = RateLimitBucket::auth_failures(DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM);
        let _ = limiter.check(&bucket, ip).await;
        tracing::debug!(ip = %ip, "OTEL auth failure tracked");
    }
    if let Err(ApiKeyAuthError::Unavailable) = result
        && let (Some(limiter), Some(ip)) = (&state.rate_limiter, &client_ip)
    {
        let _ = limiter.check(&unavailable, ip).await;
    }

    result?;
    Ok(next.run(request).await)
}

/// Extract the client IP to attribute an auth failure to.
///
/// Delegates to [`sideseat_core::utils::client_ip::attributable_ip`], which is shared with the gRPC transport - the
/// two must agree, or the weaker one is the only one that matters. It believes `X-Forwarded-For` only when the
/// immediate peer is a configured trusted proxy; with none configured (the default) the peer is used. See that
/// module for why unconditional trust and peer-only attribution are both wrong.
fn get_client_ip(
    request: &Request,
    addr: SocketAddr,
    trusted: &sideseat_core::utils::client_ip::TrustedProxies,
) -> Option<String> {
    sideseat_core::utils::client_ip::attributable_ip(
        Some(addr.ip()),
        request
            .headers()
            .get("X-Forwarded-For")
            .and_then(|v| v.to_str().ok()),
        trusted,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;

    const NOW: i64 = 1_700_000_000;

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
        let future = NOW + 3600;
        let v = make_validation(ApiKeyScope::Full, Some(future));
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Read, NOW).is_ok());
    }

    #[test]
    fn test_check_expiry_expired() {
        let past = NOW - 3600;
        let v = make_validation(ApiKeyScope::Full, Some(past));
        let result = check_expiry_and_scope(&v, ApiKeyScope::Read, NOW);
        assert!(matches!(result, Err(ApiKeyAuthError::Expired)));
    }

    #[test]
    fn test_check_expiry_no_expiry() {
        let v = make_validation(ApiKeyScope::Full, None);
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Read, NOW).is_ok());
    }

    #[test]
    fn test_check_scope_full_grants_all() {
        let v = make_validation(ApiKeyScope::Full, None);
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Read, NOW).is_ok());
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Write, NOW).is_ok());
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Ingest, NOW).is_ok());
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Full, NOW).is_ok());
    }

    #[test]
    fn test_check_scope_read_only() {
        let v = make_validation(ApiKeyScope::Read, None);
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Read, NOW).is_ok());
        assert!(matches!(
            check_expiry_and_scope(&v, ApiKeyScope::Write, NOW),
            Err(ApiKeyAuthError::InsufficientScope)
        ));
    }

    #[test]
    fn test_check_scope_ingest_only() {
        let v = make_validation(ApiKeyScope::Ingest, None);
        assert!(check_expiry_and_scope(&v, ApiKeyScope::Ingest, NOW).is_ok());
        assert!(matches!(
            check_expiry_and_scope(&v, ApiKeyScope::Read, NOW),
            Err(ApiKeyAuthError::InsufficientScope)
        ));
    }

    fn trusted(entries: &[&str]) -> sideseat_core::utils::client_ip::TrustedProxies {
        sideseat_core::utils::client_ip::TrustedProxies::parse(
            &entries.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .expect("test entries are valid")
    }

    fn request_with_xff(value: Option<&str>) -> HttpRequest<Body> {
        let builder = HttpRequest::builder().uri("/test");
        match value {
            Some(v) => builder.header("X-Forwarded-For", v),
            None => builder,
        }
        .body(Body::empty())
        .unwrap()
    }

    #[test]
    fn test_get_client_ip_from_socket() {
        let request = request_with_xff(None);
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        let ip = get_client_ip(&request, addr, &trusted(&[]));
        assert_eq!(ip, Some("192.168.1.1".to_string()));
    }

    /// With no trusted proxy configured, a forwarded header is **ignored**.
    ///
    /// It used to be believed unconditionally, which let a direct attacker set a fresh value per request and
    /// never exhaust a bucket - so the limiter did nothing at all. Nobody vouched for this header.
    #[test]
    fn an_unvouched_xff_header_is_ignored() {
        let request = request_with_xff(Some("10.0.0.1"));
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        assert_eq!(
            get_client_ip(&request, addr, &trusted(&[])),
            Some("192.168.1.1".to_string()),
        );
    }

    /// From a trusted proxy the header names the client, so each client gets its own bucket.
    ///
    /// Attributing everything to the peer instead let one attacker behind a proxy exhaust the bucket every
    /// other client shared - an unauthenticated denial of service.
    #[test]
    fn a_trusted_proxy_xff_names_the_client() {
        let request = request_with_xff(Some("10.0.0.1"));
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        assert_eq!(
            get_client_ip(&request, addr, &trusted(&["192.168.1.1"])),
            Some("10.0.0.1".to_string()),
        );
    }

    /// The **rightmost untrusted** hop wins, not the leftmost.
    ///
    /// The leftmost is whatever the client itself put there, so choosing it let a client impersonate any
    /// address - including another client's, to exhaust their bucket.
    #[test]
    fn the_rightmost_untrusted_hop_wins() {
        let request = request_with_xff(Some("1.1.1.1, 10.0.0.2, 192.168.1.9"));
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        assert_eq!(
            get_client_ip(&request, addr, &trusted(&["192.168.1.0/24"])),
            Some("10.0.0.2".to_string()),
        );
    }

    #[test]
    fn xff_whitespace_is_tolerated() {
        let request = request_with_xff(Some("  10.0.0.1  "));
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        assert_eq!(
            get_client_ip(&request, addr, &trusted(&["192.168.1.1"])),
            Some("10.0.0.1".to_string()),
        );
    }

    /// The clock at `NOW`.
    #[derive(Debug)]
    struct AtNow;

    impl Clock for AtNow {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            chrono::DateTime::from_timestamp(NOW, 0).expect("a time")
        }
    }

    /// A store that answers each lookup as told.
    struct Lookups {
        key: Result<Option<ApiKeyValidation>, ()>,
        project: Result<Option<sideseat_ports::types::ProjectRow>, ()>,
    }

    impl KeyLookups for Lookups {
        fn api_key_by_hash<'a>(&'a self, _: &'a str) -> Lookup<'a, Option<ApiKeyValidation>> {
            let answer = self
                .key
                .clone()
                .map_err(|()| sideseat_ports::error::DataError::pool_exhausted("postgres"));
            Box::pin(async move { answer })
        }

        fn project<'a>(
            &'a self,
            _: &'a str,
        ) -> Lookup<'a, Option<sideseat_ports::types::ProjectRow>> {
            let answer = self
                .project
                .clone()
                .map_err(|()| sideseat_ports::error::DataError::pool_exhausted("postgres"));
            Box::pin(async move { answer })
        }
    }

    fn project_of(org: &str) -> sideseat_ports::types::ProjectRow {
        sideseat_ports::types::ProjectRow {
            id: "p".to_string(),
            organization_id: org.to_string(),
            name: "p".to_string(),
            created_at: 0,
            updated_at: 0,
        }
    }

    async fn authorize(lookups: &Lookups) -> Result<(ApiKeyValidation, String), ApiKeyAuthError> {
        let header = format!(
            "Bearer {}",
            sideseat_core::utils::api_key::generate_api_key()
        );
        authorize_for_project(
            lookups,
            b"secret",
            &header,
            "p",
            ApiKeyScope::Ingest,
            &AtNow,
        )
        .await
    }

    /// A store that fails while a key is checked says nothing about the key: the export is answered 503 over HTTP
    /// and `Unavailable` over gRPC - both of which exporters retry - and is not counted as a failed attempt, so a
    /// store outage cannot lock a client out. Before, it was a 401 the exporter dropped, charged to the client.
    #[tokio::test]
    async fn a_store_failure_while_checking_a_key_is_unavailable_not_unauthorized() {
        let good = Ok(Some(make_validation(ApiKeyScope::Full, None)));
        for lookups in [
            Lookups {
                key: Err(()),
                project: Ok(Some(project_of("test-org"))),
            },
            Lookups {
                key: good.clone(),
                project: Err(()),
            },
        ] {
            let refused = authorize(&lookups).await.expect_err("refused");
            assert!(
                matches!(refused, ApiKeyAuthError::Unavailable),
                "{refused:?}"
            );
            assert!(!refused.counts_as_failed_attempt());
            assert_eq!(refused.grpc_status().code(), tonic::Code::Unavailable);
            let response = refused.into_response();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(response.headers().contains_key(header::RETRY_AFTER));
        }
        // A key the store answered for and found wanting is still a 401, and counted.
        for lookups in [
            Lookups {
                key: Ok(None),
                project: Ok(Some(project_of("test-org"))),
            },
            Lookups {
                key: good.clone(),
                project: Ok(Some(project_of("another-org"))),
            },
        ] {
            let refused = authorize(&lookups).await.expect_err("refused");
            assert!(
                matches!(refused, ApiKeyAuthError::InvalidKey),
                "{refused:?}"
            );
            assert!(refused.counts_as_failed_attempt());
            assert_eq!(refused.grpc_status().code(), tonic::Code::Unauthenticated);
            assert_eq!(refused.into_response().status(), StatusCode::UNAUTHORIZED);
        }
        let lookups = Lookups {
            key: good,
            project: Ok(Some(project_of("test-org"))),
        };
        assert!(authorize(&lookups).await.is_ok());
    }

    /// Both transports decide what a refusal costs and how it is answered with the same two functions, so a store
    /// failure is a retry on HTTP and gRPC alike.
    #[test]
    fn both_transports_answer_a_refusal_alike() {
        let http = include_str!("api_key.rs");
        let http = http
            .split_once("mod tests {")
            .map_or(http, |(code, _)| code);
        let grpc = include_str!("../routes/otlp_collector/grpc.rs");
        let grpc = grpc
            .split_once("mod grpc_auth_tests {")
            .map_or(grpc, |(code, _)| code);
        assert!(http.contains("e.counts_as_failed_attempt()"));
        assert!(grpc.contains("e.counts_as_failed_attempt()"));
        assert!(grpc.contains("e.grpc_status()"));
        // Both bound what a failing store is asked: past the limit, a check is answered without a lookup.
        for (source, transport) in [(http, "\"http\""), (grpc, "\"grpc\"")] {
            let bucket = source
                .find("auth_unavailable(")
                .expect("the unavailable bucket");
            assert!(
                source[bucket..]
                    .trim_start_matches("auth_unavailable(")
                    .trim_start()
                    .starts_with(transport)
            );
            let blocked = source
                .find("is_blocked(&unavailable")
                .expect("checked before the lookup");
            let lookup = source
                .rfind("validate_api_key_for_project(")
                .expect("the lookup");
            assert!(
                blocked < lookup,
                "{transport}: the bound is checked after the lookup"
            );
            assert!(
                source.contains("limiter.check(&unavailable"),
                "{transport}: unavailable checks are not counted"
            );
        }
        assert!(!grpc.contains("Status::unauthenticated(\"invalid or unauthorized API key\")"));
    }
}
