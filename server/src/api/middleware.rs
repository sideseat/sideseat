//! HTTP middleware (CORS, 404 handler)

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::IntoResponse;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Allowed origins configuration
#[derive(Debug, Clone)]
pub struct AllowedOrigins {
    origins: Vec<String>,
}

impl AllowedOrigins {
    /// Create allowed origins from host and port configuration
    pub fn new(host: &str, port: u16) -> Self {
        let mut origins = Vec::new();
        let dev_port = port + 1;

        origins.push(format!("http://{}:{}", host, port));
        origins.push(format!("http://{}:{}", host, dev_port));

        // Also allow localhost
        if host == "127.0.0.1" || host == "localhost" {
            origins.push(format!("http://localhost:{}", port));
            origins.push(format!("http://localhost:{}", dev_port));
            origins.push(format!("http://127.0.0.1:{}", port));
            origins.push(format!("http://127.0.0.1:{}", dev_port));
            origins.push("http://127.0.0.1".to_string());
            origins.push("http://localhost".to_string());
        }

        Self { origins }
    }

    /// Check if an origin is allowed
    pub fn is_allowed(&self, origin: &str) -> bool {
        self.origins.iter().any(|o| o == origin)
    }

    /// Get origins as HeaderValues for CORS
    fn as_header_values(&self) -> Vec<HeaderValue> {
        self.origins.iter().filter_map(|o| o.parse().ok()).collect()
    }
}

/// Create CORS layer
pub fn cors(allowed: &AllowedOrigins) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed.as_header_values()))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::ACCEPT,
            header::ORIGIN,
            header::CACHE_CONTROL,
        ])
        .allow_credentials(true)
}

const MAX_404_BODY_LOG: usize = 64 * 1024; // 64KB limit for logging

/// Handle 404 Not Found with logging
pub async fn handle_404(req: Request) -> impl IntoResponse {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return StatusCode::NOT_FOUND;
    }

    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();

    let body_bytes = match to_bytes(req.into_body(), MAX_404_BODY_LOG).await {
        Ok(bytes) => bytes,
        Err(_) => {
            tracing::debug!("[404] {} {} (failed to read body)", method, uri);
            return StatusCode::NOT_FOUND;
        }
    };

    let mut headers_map = serde_json::Map::new();
    for (name, value) in headers.iter() {
        if let Ok(value_str) = value.to_str() {
            headers_map.insert(
                name.to_string(),
                serde_json::Value::String(value_str.to_string()),
            );
        }
    }

    let body_value = if body_bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body_bytes).unwrap_or_else(|_| {
            String::from_utf8(body_bytes.to_vec())
                .map(serde_json::Value::String)
                .unwrap_or_else(|_| {
                    serde_json::Value::String(format!("<binary {} bytes>", body_bytes.len()))
                })
        })
    };

    let log_entry = serde_json::json!({
        "status": 404,
        "method": method.to_string(),
        "url": uri.to_string(),
        "headers": headers_map,
        "body": body_value,
    });

    if let Ok(pretty) = serde_json::to_string_pretty(&log_entry) {
        tracing::debug!("[404]\n{}", pretty);
    }

    StatusCode::NOT_FOUND
}
