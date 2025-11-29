//! HTTP and gRPC server implementation

mod banner;
mod embedded;
mod grpc;
mod handlers;
mod middleware;

use crate::api::routes;
use crate::auth::AuthManager;
use crate::core::{CliConfig, ConfigManager, SecretManager, StorageManager};
use crate::otel::OtelManager;
use crate::sqlite::DatabaseManager;
use crate::{Error, Result};
use axum::{Router, response::Redirect, routing::get};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::watch;
use tower_http::trace::TraceLayer;

pub async fn start(cli_config: CliConfig) -> Result<()> {
    // Storage must be initialized first - creates directories and verifies write access
    let storage = StorageManager::init().await?;

    if storage.using_fallback() {
        tracing::warn!("Using fallback storage location: {:?}", storage.data_dir());
    } else {
        tracing::debug!("Storage initialized at: {:?}", storage.data_dir());
    }

    let config_manager = ConfigManager::init(&storage, &cli_config)?;
    let config = config_manager.config();

    for source in config_manager.loaded_sources() {
        if let Some(ref path) = source.path {
            tracing::info!("Config loaded from {}: {}", source.name, path.display());
        }
    }

    let secrets = SecretManager::init(&storage).await?;
    let auth_manager = Arc::new(AuthManager::init(&secrets, config.auth.enabled).await?);

    // Create shutdown channel for background tasks
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Initialize global database manager (before OTel)
    let db = Arc::new(
        DatabaseManager::init(storage.data_dir())
            .await
            .map_err(|e| Error::Internal(format!("Failed to initialize database: {}", e)))?,
    );

    // Start periodic WAL checkpoint task
    db.start_checkpoint_task(shutdown_rx.clone());

    let otel_manager = if config.otel.enabled {
        match OtelManager::init(config.otel.clone(), db.pool().clone()).await {
            Ok(otel) => {
                tracing::debug!("OTel collector enabled");
                Some(Arc::new(otel))
            }
            Err(e) => {
                tracing::error!("Failed to initialize OTel manager: {}", e);
                None
            }
        }
    } else {
        tracing::debug!("OTel collector disabled");
        None
    };

    let api_routes = routes::create_routes(auth_manager.clone(), otel_manager.clone());
    let ui_routes = Router::new().fallback(embedded::serve_assets);

    // OTel collector at /otel - separate from API, no auth required for OTLP compatibility
    let otel_collector_routes =
        otel_manager.as_ref().map(|otel| crate::api::otel::create_collector_routes(otel.clone()));

    let mut app = Router::new()
        .route("/", get(|| async { Redirect::permanent("/ui") }))
        .nest("/api/v1", api_routes)
        .nest_service("/ui", ui_routes);

    if let Some(routes) = otel_collector_routes {
        app = app.nest("/otel", routes);
    }

    let app = app
        .fallback(handlers::handle_404)
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

    let grpc_handle = if let Some(ref otel) = otel_manager {
        if config.otel.grpc.enabled {
            let grpc_addr = format!("{}:{}", config.server.host, config.otel.grpc.port);
            match grpc::start_grpc_server(otel.clone(), &grpc_addr).await {
                Ok(handle) => Some(handle),
                Err(e) => {
                    tracing::error!("Failed to start gRPC server: {}", e);
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    banner::print_banner(
        &config.server.host,
        config.server.port,
        config.otel.grpc.port,
        auth_manager.is_enabled(),
        auth_manager.bootstrap_token(),
        otel_manager.is_some(),
        grpc_handle.is_some(),
    );

    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        Error::Internal(format!(
            "Failed to bind to {}:{}: {}",
            config.server.host, config.server.port, e
        ))
    })?;

    let http_server = axum::serve(listener, app);

    // Graceful shutdown on Ctrl+C (SIGINT) or SIGTERM
    let otel_for_shutdown = otel_manager.clone();

    // Create shutdown signal that handles both SIGINT and SIGTERM
    let shutdown_signal = async {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    println!("\n  \x1b[33m➜\x1b[0m  Shutting down...");
                }
                _ = sigterm.recv() => {
                    println!("\n  \x1b[33m➜\x1b[0m  Shutting down (SIGTERM)...");
                }
            }
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.ok();
            println!("\n  \x1b[33m➜\x1b[0m  Shutting down...");
        }
    };

    tokio::select! {
        result = http_server => {
            result.map_err(|e| Error::Internal(format!("Server error: {}", e)))?;
        }
        _ = shutdown_signal => {}
    }

    // Graceful shutdown sequence:
    // 1. Signal background tasks to stop
    let _ = shutdown_tx.send(true);

    // 2. Flush OTel data
    if let Some(otel) = otel_for_shutdown
        && let Err(e) = otel.shutdown().await
    {
        tracing::error!("OTel shutdown error: {}", e);
    }

    // 3. Final WAL checkpoint
    if let Err(e) = db.checkpoint().await {
        tracing::warn!("Failed to checkpoint WAL on shutdown: {}", e);
    }

    if let Some(handle) = grpc_handle {
        handle.abort();
    }

    Ok(())
}
