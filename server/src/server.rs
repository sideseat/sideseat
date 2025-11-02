use crate::config::Settings;
use crate::{Error, Result};
use crate::{embedded, middleware};
use axum::{Router, response::Redirect, routing::get};
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;

pub async fn start() -> Result<()> {
    let settings = Settings::new()?;

    // API routes under /api/v1
    let api_routes = Router::new().route("/health", get(health_check));

    // UI routes under /ui - serve embedded frontend assets
    let ui_routes = Router::new().fallback(embedded::serve_assets);

    let app = Router::new()
        .route("/", get(|| async { Redirect::permanent("/ui") }))
        .nest("/api/v1", api_routes)
        .nest_service("/ui", ui_routes)
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

async fn health_check() -> &'static str {
    "OK"
}
