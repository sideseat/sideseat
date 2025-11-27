//! HTTP middleware (CORS, etc.)

use axum::http::{HeaderValue, Method, header};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Create CORS layer that allows requests from the server's own origin
///
/// Allows:
/// - Same host:port (server's own origin)
/// - Same host:port+1 (for dev frontend on adjacent port)
/// - Alternate localhost/127.0.0.1 forms
pub fn cors(host: &str, port: u16) -> CorsLayer {
    let mut origins: Vec<HeaderValue> = Vec::new();
    let dev_port = port + 1;

    // Add both port and port+1 for the primary host
    origins.push(format!("http://{}:{}", host, port).parse().unwrap());
    origins.push(format!("http://{}:{}", host, dev_port).parse().unwrap());

    // For localhost/127.0.0.1, also allow the alternate form
    if host == "127.0.0.1" {
        origins.push(format!("http://localhost:{}", port).parse().unwrap());
        origins.push(format!("http://localhost:{}", dev_port).parse().unwrap());
    } else if host == "localhost" {
        origins.push(format!("http://127.0.0.1:{}", port).parse().unwrap());
        origins.push(format!("http://127.0.0.1:{}", dev_port).parse().unwrap());
    }

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
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
        ])
        .allow_credentials(true)
}
