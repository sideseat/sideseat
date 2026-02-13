//! Frontend asset embedding for production builds
//!
//! In development: Frontend runs on Vite dev server (port 5173)
//! In production: Assets are embedded in the binary from web/dist

use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::Response,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist"]
pub struct Assets;

// Hashed assets (e.g., /assets/index-abc123.js) can be cached indefinitely
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
// HTML and non-hashed files should revalidate
const CACHE_REVALIDATE: &str = "public, max-age=0, must-revalidate";

pub async fn serve_assets(uri: Uri) -> Response<Body> {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let etag = hex::encode(file.metadata.sha256_hash());
        let cache = if path.starts_with("assets/") {
            CACHE_IMMUTABLE
        } else {
            CACHE_REVALIDATE
        };

        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::CACHE_CONTROL, cache)
            .header(header::ETAG, format!("\"{}\"", etag))
            .body(Body::from(file.data.into_owned()))
            .unwrap();
    }

    // SPA routing: serve index.html for non-asset paths
    if let Some(file) = Assets::get("index.html") {
        let etag = hex::encode(file.metadata.sha256_hash());
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html")
            .header(header::CACHE_CONTROL, CACHE_REVALIDATE)
            .header(header::ETAG, format!("\"{}\"", etag))
            .body(Body::from(file.data.into_owned()))
            .unwrap();
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("404 Not Found"))
        .unwrap()
}
