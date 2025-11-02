use crate::config::Settings;
use crate::middleware;
use crate::{Error, Result};
use axum::{Router, routing::get};
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;

pub async fn start() -> Result<()> {
    let settings = Settings::new()?;

    let app = Router::new()
        .route("/health", get(health_check))
        // TODO: Add API routes
        // TODO: Add frontend routes (embedded assets)
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

    tracing::info!("Server listening on {}", addr);

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
