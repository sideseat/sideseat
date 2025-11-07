use crate::api::routes;
use crate::config::Settings;
use crate::{Error, Result};
use crate::{embedded, middleware};
use axum::body::to_bytes;
use axum::{
    Router, extract::Request, http::StatusCode, response::IntoResponse, response::Redirect,
    routing::get,
};
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;

pub async fn start() -> Result<()> {
    let settings = Settings::new()?;

    // API routes under /api/v1
    let api_routes = routes::create_routes();

    // UI routes under /ui - serve embedded frontend assets
    let ui_routes = Router::new().fallback(embedded::serve_assets);

    let app = Router::new()
        .route("/", get(|| async { Redirect::permanent("/ui") }))
        .nest("/api/v1", api_routes)
        .nest_service("/ui", ui_routes)
        .fallback(handle_404)
        .layer(TraceLayer::new_for_http())
        .layer(middleware::cors());

    let addr = SocketAddr::from((
        settings
            .server
            .host
            .parse::<std::net::Ipv4Addr>()
            .map_err(|e| Error::Config(format!("Invalid host: {}", e)))?
            .octets(),
        settings.server.port,
    ));

    println!();
    println!("  \x1b[1m\x1b[36mSideSeat\x1b[0m \x1b[90mv{}\x1b[0m", env!("CARGO_PKG_VERSION"));
    println!();
    println!(
        "  \x1b[32m➜\x1b[0m  \x1b[1mLocal:\x1b[0m    \x1b[36mhttp://{}:{}\x1b[0m",
        settings.server.host, settings.server.port
    );
    println!("  \x1b[90m➜  Network:  use --host to expose\x1b[0m");
    println!();
    println!("  \x1b[1mEndpoints:\x1b[0m");
    println!("  \x1b[90m├─\x1b[0m UI:      \x1b[36m/ui\x1b[0m");
    println!("  \x1b[90m├─\x1b[0m API:     \x1b[36m/api/v1\x1b[0m");
    println!("  \x1b[90m└─\x1b[0m Health:  \x1b[36m/api/v1/health\x1b[0m");
    println!();

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| Error::Internal(format!("Failed to bind: {}", e)))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| Error::Internal(format!("Server error: {}", e)))?;

    Ok(())
}

async fn handle_404(req: Request) -> impl IntoResponse {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();

    // Extract the body
    let body_bytes = match to_bytes(req.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            println!("\x1b[31m[404]\x1b[0m Failed to read request body");
            return StatusCode::NOT_FOUND;
        }
    };

    // Build URL string
    let url = uri.to_string();

    // Convert headers to JSON object
    let mut headers_map = serde_json::Map::new();
    for (name, value) in headers.iter() {
        if let Ok(value_str) = value.to_str() {
            headers_map.insert(name.to_string(), serde_json::Value::String(value_str.to_string()));
        }
    }

    // Parse body as JSON or text
    let body_value = if body_bytes.is_empty() {
        serde_json::Value::Null
    } else {
        match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
            Ok(json) => json,
            Err(_) => {
                // Not valid JSON, try as text
                match String::from_utf8(body_bytes.to_vec()) {
                    Ok(text) => serde_json::Value::String(text),
                    Err(_) => {
                        serde_json::Value::String(format!("<binary {} bytes>", body_bytes.len()))
                    }
                }
            }
        }
    };

    // Create the complete JSON object
    let log_entry = serde_json::json!({
        "status": 404,
        "method": method.to_string(),
        "url": url,
        "headers": headers_map,
        "body": body_value,
    });

    // Print as pretty JSON
    println!("\x1b[31m[404]\x1b[0m");
    if let Ok(pretty) = serde_json::to_string_pretty(&log_entry) {
        println!("{}", pretty);
    }

    StatusCode::NOT_FOUND
}
