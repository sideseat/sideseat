//! Rate limiting middleware for API routes

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;

use crate::data::cache::{RateLimitBucket, RateLimitResult, RateLimiter};

/// Rate limit middleware state
#[derive(Clone)]
pub struct RateLimitState {
    pub limiter: Arc<RateLimiter>,
    pub bucket: RateLimitBucket,
    pub key_extractor: KeyExtractor,
    pub bypass_header: Option<String>,
}

/// How to extract rate limit key from request
#[derive(Clone, Copy)]
pub enum KeyExtractor {
    /// Per-IP rate limiting (requires `per_ip: true` config)
    IpAddress,
    /// Per-project rate limiting (from path param)
    ProjectId,
}

/// Rate limit exceeded response
pub struct RateLimitExceeded(RateLimitResult);

impl IntoResponse for RateLimitExceeded {
    fn into_response(self) -> Response {
        let r = &self.0;

        let mut response = Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header("X-RateLimit-Limit", r.limit.to_string())
            .header("X-RateLimit-Remaining", r.remaining.to_string())
            .header("X-RateLimit-Reset", r.reset_at.to_string())
            .header(header::RETRY_AFTER, r.retry_after.unwrap_or(60).to_string())
            .body(Body::from("Rate limit exceeded"))
            .unwrap();

        // Ensure content-type is set
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );

        response
    }
}

/// Add rate limit headers to response
fn add_rate_limit_headers(response: &mut Response, result: &RateLimitResult) {
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&result.limit.to_string()) {
        headers.insert("X-RateLimit-Limit", v);
    }
    if let Ok(v) = HeaderValue::from_str(&result.remaining.to_string()) {
        headers.insert("X-RateLimit-Remaining", v);
    }
    if let Ok(v) = HeaderValue::from_str(&result.reset_at.to_string()) {
        headers.insert("X-RateLimit-Reset", v);
    }
}

/// Extract rate limit key based on configuration
fn extract_key(request: &Request, key_extractor: KeyExtractor, addr: SocketAddr) -> String {
    match key_extractor {
        KeyExtractor::IpAddress => {
            // Prefer X-Forwarded-For for proxied requests (first IP only)
            request
                .headers()
                .get("X-Forwarded-For")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| addr.ip().to_string())
        }
        KeyExtractor::ProjectId => {
            // Extract from path: /otel/{project_id}/... or /api/v1/project/{project_id}/...
            let path = request.uri().path();
            path.split('/')
                .skip_while(|s| *s != "otel" && *s != "project")
                .nth(1)
                .unwrap_or("unknown")
                .to_string()
        }
    }
}

/// Rate limiting middleware function
pub async fn rate_limit_middleware(
    State(state): State<RateLimitState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Result<Response, RateLimitExceeded> {
    // Check bypass header (for internal services)
    if let Some(ref bypass_secret) = state.bypass_header
        && let Some(header_val) = request.headers().get("X-RateLimit-Bypass")
        && header_val.to_str().ok() == Some(bypass_secret.as_str())
    {
        tracing::trace!("Rate limit bypassed via header");
        return Ok(next.run(request).await);
    }

    // Extract key based on configuration
    let key = extract_key(&request, state.key_extractor, addr);

    // Check rate limit
    let result = state.limiter.check(&state.bucket, &key).await;

    if !result.allowed {
        tracing::debug!(
            bucket = state.bucket.name,
            %key,
            "Rate limit exceeded"
        );
        return Err(RateLimitExceeded(result));
    }

    // Add rate limit headers to successful response
    let mut response = next.run(request).await;
    add_rate_limit_headers(&mut response, &result);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_extractor_variants() {
        // Just ensure the enum variants exist
        let _ = KeyExtractor::IpAddress;
        let _ = KeyExtractor::ProjectId;
    }

    #[test]
    fn test_rate_limit_exceeded_response() {
        let result = RateLimitResult {
            allowed: false,
            remaining: 0,
            limit: 100,
            reset_at: 1705593600,
            retry_after: Some(45),
        };
        let response = RateLimitExceeded(result).into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
