use axum::http::HeaderValue;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// Create CORS layer that only allows requests from the server's own origin
///
/// This restricts cross-origin requests to only come from the same host:port
/// that the server is running on, which is the secure default for localhost.
pub fn cors(host: &str, port: u16) -> CorsLayer {
    // Build the allowed origin URL
    let origin = format!("http://{}:{}", host, port);

    // For localhost/127.0.0.1, also allow the alternate form
    let origins: Vec<HeaderValue> = if host == "127.0.0.1" {
        vec![origin.parse().unwrap(), format!("http://localhost:{}", port).parse().unwrap()]
    } else if host == "localhost" {
        vec![origin.parse().unwrap(), format!("http://127.0.0.1:{}", port).parse().unwrap()]
    } else {
        vec![origin.parse().unwrap()]
    };

    CorsLayer::new().allow_origin(AllowOrigin::list(origins)).allow_methods(Any).allow_headers(Any)
}
