// Frontend asset embedding for production builds
//
// In development: Frontend runs on Vite dev server (port 5173)
// In production: Assets are embedded in the binary from web/dist
//
// NOTE: The #[folder] path must exist at compile time. We create
// a placeholder in Phase 0 to avoid compile errors before frontend is built.

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist"]
#[cfg_attr(debug_assertions, prefix = "")]
pub struct Assets;

// TODO: Add Axum handler to serve embedded assets
// use axum::response::Html;
// use axum::http::StatusCode;
//
// pub async fn serve_assets(uri: Uri) -> Result<Response> {
//     let path = uri.path().trim_start_matches('/');
//     // Serve index.html for SPA routes
//     // Serve static assets from embedded folder
// }
