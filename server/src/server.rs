use crate::api::routes;
use crate::auth::AuthManager;
use crate::core::constants::APP_NAME;
use crate::core::utils::terminal_link;
use crate::core::{CliConfig, ConfigManager, SecretManager, StorageManager};
use crate::{Error, Result};
use crate::{embedded, middleware};
use axum::body::to_bytes;
use axum::{
    Router, extract::Request, http::StatusCode, response::IntoResponse, response::Redirect,
    routing::get,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

pub async fn start(cli_config: CliConfig) -> Result<()> {
    // Initialize storage manager first (creates directories, verifies access)
    let storage = StorageManager::init().await?;

    if storage.using_fallback() {
        tracing::warn!("Using fallback storage location: {:?}", storage.data_dir());
    } else {
        tracing::debug!("Storage initialized at: {:?}", storage.data_dir());
    }

    // Initialize config manager with storage paths and CLI arguments
    let config_manager = ConfigManager::init(&storage, &cli_config)?;
    let config = config_manager.config();

    // Log loaded config sources
    for source in config_manager.loaded_sources() {
        if let Some(ref path) = source.path {
            tracing::info!("Config loaded from {}: {}", source.name, path.display());
        }
    }

    // Initialize secret manager and auth manager
    let secrets = SecretManager::init(&storage).await?;
    let auth_manager = Arc::new(AuthManager::init(&secrets, config.auth.enabled).await?);

    // API routes under /api/v1
    let api_routes = routes::create_routes(auth_manager.clone());

    // UI routes under /ui - serve embedded frontend assets
    let ui_routes = Router::new().fallback(embedded::serve_assets);

    let app = Router::new()
        .route("/", get(|| async { Redirect::permanent("/ui") }))
        .nest("/api/v1", api_routes)
        .nest_service("/ui", ui_routes)
        .fallback(handle_404)
        .layer(TraceLayer::new_for_http())
        .layer(middleware::cors(&config.server.host, config.server.port));

    let addr = SocketAddr::from((
        config
            .server
            .host
            .parse::<std::net::Ipv4Addr>()
            .map_err(|e| Error::Config(format!("Invalid host '{}': {}", config.server.host, e)))?
            .octets(),
        config.server.port,
    ));

    println!();
    println!("  \x1b[1m\x1b[36m{}\x1b[0m \x1b[90mv{}\x1b[0m", APP_NAME, env!("CARGO_PKG_VERSION"));
    println!();

    // Show local URL (with token if auth is enabled)
    let local_url = if auth_manager.is_enabled() {
        format!(
            "http://{}:{}/ui?token={}",
            config.server.host,
            config.server.port,
            auth_manager.bootstrap_token()
        )
    } else {
        format!("http://{}:{}", config.server.host, config.server.port)
    };
    println!("  \x1b[32m➜\x1b[0m  \x1b[1mLocal:\x1b[0m    {}", terminal_link(&local_url));

    // Show network info based on bind address
    if config.server.host == "127.0.0.1" || config.server.host == "localhost" {
        println!("  \x1b[90m➜  Network:  use --host 0.0.0.0 to expose\x1b[0m");
    } else {
        // Show actual network addresses when exposed
        if let Ok(interfaces) = local_ip_address::list_afinet_netifas() {
            for (_, ip) in interfaces.iter().filter(|(_, ip)| ip.is_ipv4() && !ip.is_loopback()) {
                let network_url = format!("http://{}:{}", ip, config.server.port);
                println!(
                    "  \x1b[32m➜\x1b[0m  \x1b[1mNetwork:\x1b[0m  {}",
                    terminal_link(&network_url)
                );
            }
        }
    }

    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        Error::Internal(format!(
            "Failed to bind to {}:{}: {}",
            config.server.host, config.server.port, e
        ))
    })?;

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
