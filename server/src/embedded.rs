// Frontend asset embedding for production builds
//
// In development: Frontend runs on Vite dev server (port 5173)
// In production: Assets are embedded in the binary from web/dist

use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::Response,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist"]
pub struct Assets;

/// Serve embedded frontend assets
pub async fn serve_assets(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Default to index.html for root path
    let path = if path.is_empty() { "index.html" } else { path };

    // Try to serve the requested file
    if let Some(content) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(Body::from(content.data))
            .unwrap();
    }

    // For SPA routing: serve index.html for any non-asset path
    if let Some(content) = Assets::get("index.html") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html")
            .body(Body::from(content.data))
            .unwrap();
    }

    // Fallback 404
    Response::builder().status(StatusCode::NOT_FOUND).body(Body::from("404 Not Found")).unwrap()
}
