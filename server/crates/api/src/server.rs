//! API server initialization

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::response::Redirect;
use axum::routing::get;
use tokio::net::TcpListener;

use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;

use super::auth::AuthManager;
use super::auth::{AuthState, OtelAuthState, otel_auth_middleware, require_auth};
use super::dependencies::ApiDependencies;
use super::embedded;
use super::middleware::{self, AllowedOrigins};
use super::openapi::{openapi_json, swagger_ui_html};
use super::rate_limit::{KeyExtractor, RateLimitState, rate_limit_middleware};
use super::routes::otel::files::{FilesApiState, get_file, head_file};
use super::routes::{
    api_keys, auth, credentials, favorites, health, organizations, otel, otlp_collector, pricing,
    projects, users, ws,
};
use sideseat_core::constants::{AUTH_BODY_LIMIT, DEFAULT_BODY_LIMIT, OTLP_BODY_LIMIT};
use sideseat_domain::files::FileService;
use sideseat_domain::rate_limit::RateLimitBucket;

pub struct ApiServer {
    dependencies: ApiDependencies,
    auth_manager: Arc<AuthManager>,
    allowed_origins: AllowedOrigins,
}

impl ApiServer {
    pub fn new(dependencies: ApiDependencies) -> Self {
        let auth_manager = dependencies.auth.clone();
        let allowed_origins = AllowedOrigins::new(
            &dependencies.config.server.host,
            dependencies.config.server.port,
        );

        Self {
            dependencies,
            auth_manager,
            allowed_origins,
        }
    }

    pub async fn start(self) -> Result<()> {
        let Self {
            dependencies: app,
            auth_manager,
            allowed_origins,
        } = self;

        let shutdown_rx = app.shutdown_rx.clone();

        let host = app.config.server.host.clone();
        let port = app.config.server.port;
        let addr = SocketAddr::new(host.parse()?, port);

        // Use debug directory if debug mode is enabled (directory is created in app.rs)
        let debug_path = if app.config.debug {
            Some(
                app.storage
                    .subdir(sideseat_core::storage::DataSubdir::Debug),
            )
        } else {
            None
        };

        let ui_routes = Router::new().fallback(embedded::serve_assets);

        let api_key_secret = app.api_key_secret.clone();
        // Parsed once, and a bad entry refuses startup rather than silently collapsing every client behind
        // that proxy into one rate-limit bucket - see `utils::client_ip`.
        let trusted_proxies = Arc::new(
            sideseat_core::utils::client_ip::TrustedProxies::parse(
                &app.config.rate_limit.trusted_proxies,
            )
            .map_err(|e| anyhow::anyhow!(e))?,
        );

        // Rate limiting configuration
        // - rate_limit_enabled: master switch for all rate limiting (per-project)
        // - rate_limit_per_ip: additional per-IP rate limiting (disabled by default)
        let rate_limit_enabled = app.config.rate_limit.enabled;
        let rate_limit_per_ip = app.config.rate_limit.enabled && app.config.rate_limit.per_ip;
        let rate_limiter = app.rate_limiter.clone();
        let bypass_header = app.config.rate_limit.bypass_header.clone();

        // Helper to create rate limit state
        let make_rate_limit_state =
            |bucket: RateLimitBucket, key_extractor: KeyExtractor| RateLimitState {
                limiter: rate_limiter.clone(),
                bucket,
                key_extractor,
                bypass_header: bypass_header.clone(),
                trusted_proxies: Arc::clone(&trusted_proxies),
            };

        // Build OTLP ingestion routes (rate limited by project, optionally auth required)
        let otlp_routes = otlp_collector::routes(
            &app.topics,
            debug_path,
            app.database.clone(),
            app.analytics.clone(),
            app.cache.clone(),
            Arc::clone(&app.clock),
            Arc::clone(&app.staging),
            Arc::clone(&app.storage_governance),
            // Only where the queue cannot promise durability: there the handler writes before it
            // answers. Its own extraction cache is a memo, so a second instance costs nothing but a map.
            (!app.topics.is_durable()).then(|| {
                Arc::new(
                    sideseat_ingestion::traces::TracePipeline::new(
                        app.analytics.clone(),
                        app.pricing.clone(),
                        app.topics.clone(),
                        app.files.clone(),
                        Arc::clone(&app.staging),
                    )
                    .with_storage_governance(Arc::clone(&app.storage_governance)),
                )
            }),
        )
        .layer(DefaultBodyLimit::max(OTLP_BODY_LIMIT));
        let otlp_routes = if rate_limit_enabled {
            otlp_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::ingestion(app.config.rate_limit.ingestion_rpm),
                    KeyExtractor::ProjectId,
                ),
                rate_limit_middleware,
            ))
        } else {
            otlp_routes
        };
        // Add OTEL auth middleware (validates API key when otel.auth_required=true)
        let otlp_routes = otlp_routes.layer(axum::middleware::from_fn_with_state(
            OtelAuthState {
                trusted_proxies: Arc::clone(&trusted_proxies),
                database: app.database.clone(),
                cache: app.cache.clone(),
                api_key_secret: api_key_secret.clone(),
                otel_auth_required: app.config.otel.auth_required,
                clock: app.clock.clone(),
                rate_limiter: if rate_limit_per_ip {
                    Some(app.rate_limiter.clone())
                } else {
                    None
                },
            },
            otel_auth_middleware,
        ));

        // Build auth routes (rate limited by IP - brute force protection)
        let auth_routes = auth::routes(
            auth_manager.clone(),
            allowed_origins.clone(),
            app.database.clone(),
        )
        .layer(DefaultBodyLimit::max(AUTH_BODY_LIMIT));
        let auth_routes = if rate_limit_per_ip {
            auth_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::auth(app.config.rate_limit.auth_rpm),
                    KeyExtractor::IpAddress,
                ),
                rate_limit_middleware,
            ))
        } else {
            auth_routes
        };

        // Build otel query routes (rate limited by IP if enabled)
        let otel_query_routes = otel::routes(
            app.analytics.clone(),
            app.topics.clone(),
            app.files.clone(),
            app.database.clone(),
            app.cache.clone(),
            app.clock.clone(),
            app.storage_governance.clone(),
            app.shutdown_rx.clone(),
        )
        .layer(axum::middleware::from_fn_with_state(
            AuthState {
                auth_manager: auth_manager.clone(),
                allowed_origins: allowed_origins.clone(),
                database: app.database.clone(),
                api_key_secret: api_key_secret.clone(),
                clock: app.clock.clone(),
            },
            require_auth,
        ));
        let otel_query_routes = if rate_limit_per_ip {
            otel_query_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::api(app.config.rate_limit.api_rpm),
                    KeyExtractor::IpAddress,
                ),
                rate_limit_middleware,
            ))
        } else {
            otel_query_routes
        };

        // Build projects routes (rate limited by IP if enabled)
        let projects_routes = projects::routes(
            app.database.clone(),
            app.analytics.clone(),
            app.files.clone(),
            app.cache.clone(),
            app.storage_governance.clone(),
        )
        .layer(axum::middleware::from_fn_with_state(
            AuthState {
                auth_manager: auth_manager.clone(),
                allowed_origins: allowed_origins.clone(),
                database: app.database.clone(),
                api_key_secret: api_key_secret.clone(),
                clock: app.clock.clone(),
            },
            require_auth,
        ));
        let projects_routes = if rate_limit_per_ip {
            projects_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::api(app.config.rate_limit.api_rpm),
                    KeyExtractor::IpAddress,
                ),
                rate_limit_middleware,
            ))
        } else {
            projects_routes
        };

        // Build organizations routes (rate limited by IP if enabled)
        let organizations_routes = organizations::routes(
            app.database.clone(),
            app.analytics.clone(),
            app.files.clone(),
            app.cache.clone(),
            app.storage_governance.clone(),
        )
        .layer(axum::middleware::from_fn_with_state(
            AuthState {
                auth_manager: auth_manager.clone(),
                allowed_origins: allowed_origins.clone(),
                database: app.database.clone(),
                api_key_secret: api_key_secret.clone(),
                clock: app.clock.clone(),
            },
            require_auth,
        ));
        let organizations_routes = if rate_limit_per_ip {
            organizations_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::api(app.config.rate_limit.api_rpm),
                    KeyExtractor::IpAddress,
                ),
                rate_limit_middleware,
            ))
        } else {
            organizations_routes
        };

        // Build users routes (rate limited by IP if enabled)
        let users_routes =
            users::routes(app.database.clone()).layer(axum::middleware::from_fn_with_state(
                AuthState {
                    auth_manager: auth_manager.clone(),
                    allowed_origins: allowed_origins.clone(),
                    database: app.database.clone(),
                    api_key_secret: api_key_secret.clone(),
                    clock: app.clock.clone(),
                },
                require_auth,
            ));
        let users_routes = if rate_limit_per_ip {
            users_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::api(app.config.rate_limit.api_rpm),
                    KeyExtractor::IpAddress,
                ),
                rate_limit_middleware,
            ))
        } else {
            users_routes
        };

        // Build pricing routes (rate limited by IP if enabled)
        let pricing_routes =
            pricing::routes(app.pricing.clone()).layer(axum::middleware::from_fn_with_state(
                AuthState {
                    auth_manager: auth_manager.clone(),
                    allowed_origins: allowed_origins.clone(),
                    database: app.database.clone(),
                    api_key_secret: api_key_secret.clone(),
                    clock: app.clock.clone(),
                },
                require_auth,
            ));
        let pricing_routes = if rate_limit_per_ip {
            pricing_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::api(app.config.rate_limit.api_rpm),
                    KeyExtractor::IpAddress,
                ),
                rate_limit_middleware,
            ))
        } else {
            pricing_routes
        };

        // Build favorites routes (rate limited by IP if enabled)
        let favorites_routes =
            favorites::routes(app.database.clone()).layer(axum::middleware::from_fn_with_state(
                AuthState {
                    auth_manager: auth_manager.clone(),
                    allowed_origins: allowed_origins.clone(),
                    database: app.database.clone(),
                    api_key_secret: api_key_secret.clone(),
                    clock: app.clock.clone(),
                },
                require_auth,
            ));
        let favorites_routes = if rate_limit_per_ip {
            favorites_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::api(app.config.rate_limit.api_rpm),
                    KeyExtractor::IpAddress,
                ),
                rate_limit_middleware,
            ))
        } else {
            favorites_routes
        };

        // Build API keys routes (rate limited by IP if enabled)
        let api_keys_routes = api_keys::routes(
            app.database.clone(),
            app.cache.clone(),
            api_key_secret.clone(),
        )
        .layer(axum::middleware::from_fn_with_state(
            AuthState {
                auth_manager: auth_manager.clone(),
                allowed_origins: allowed_origins.clone(),
                database: app.database.clone(),
                api_key_secret: api_key_secret.clone(),
                clock: app.clock.clone(),
            },
            require_auth,
        ));
        let api_keys_routes = if rate_limit_per_ip {
            api_keys_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::api(app.config.rate_limit.api_rpm),
                    KeyExtractor::IpAddress,
                ),
                rate_limit_middleware,
            ))
        } else {
            api_keys_routes
        };

        // Build credentials routes (rate limited by IP if enabled)
        let credentials_routes =
            credentials::routes(app.credentials.clone(), app.credential_tester.clone()).layer(
                axum::middleware::from_fn_with_state(
                    AuthState {
                        auth_manager: auth_manager.clone(),
                        allowed_origins: allowed_origins.clone(),
                        database: app.database.clone(),
                        api_key_secret: api_key_secret.clone(),
                        clock: app.clock.clone(),
                    },
                    require_auth,
                ),
            );
        let credentials_routes = if rate_limit_per_ip {
            credentials_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::api(app.config.rate_limit.api_rpm),
                    KeyExtractor::IpAddress,
                ),
                rate_limit_middleware,
            ))
        } else {
            credentials_routes
        };

        // Build files routes (rate limited by project)
        let api_files_routes =
            files_routes(app.files.clone()).layer(axum::middleware::from_fn_with_state(
                AuthState {
                    auth_manager: auth_manager.clone(),
                    allowed_origins: allowed_origins.clone(),
                    database: app.database.clone(),
                    api_key_secret: api_key_secret.clone(),
                    clock: app.clock.clone(),
                },
                require_auth,
            ));
        let api_files_routes = if rate_limit_enabled {
            api_files_routes.layer(axum::middleware::from_fn_with_state(
                make_rate_limit_state(
                    RateLimitBucket::files(app.config.rate_limit.files_rpm),
                    KeyExtractor::ProjectId,
                ),
                rate_limit_middleware,
            ))
        } else {
            api_files_routes
        };

        // MCP reads project data, so it shares the authenticated project
        // boundary and per-IP API rate limit.
        let mcp_routes = if app.config.mcp.enabled {
            let ct = super::mcp::cancellation_token_from_shutdown(app.shutdown_rx.clone());
            let mcp = super::mcp::routes(app.analytics.clone(), app.clock.clone(), ct).layer(
                axum::middleware::from_fn_with_state(
                    AuthState {
                        auth_manager: auth_manager.clone(),
                        allowed_origins: allowed_origins.clone(),
                        database: app.database.clone(),
                        api_key_secret: api_key_secret.clone(),
                        clock: app.clock.clone(),
                    },
                    require_auth,
                ),
            );
            let mcp = if rate_limit_per_ip {
                mcp.layer(axum::middleware::from_fn_with_state(
                    make_rate_limit_state(
                        RateLimitBucket::api(app.config.rate_limit.api_rpm),
                        KeyExtractor::IpAddress,
                    ),
                    rate_limit_middleware,
                ))
            } else {
                mcp
            };
            Some(mcp)
        } else {
            None
        };

        // Registration state is process-local, so multi-instance deployments need
        // sticky routing for SDK runtime features.
        if app.config.database.transactional.sharing() == sideseat_core::config::Sharing::Shared {
            tracing::warn!(
                "ws: the SDK registration directory is per-process while its AG-UI routing is \
                 cross-instance, so presence and agent invocation are single-instance features. With a \
                 shared database an SDK connected to another instance reads as `registration_not_found` \
                 here, and GET /registrations shows only this instance's SDKs. Pin SDK connections to one \
                 instance if you use them."
            );
        }
        let (ws_routes, ws_state) = ws::routes(
            app.topics.clone(),
            app.registrations.clone(),
            app.shutdown_rx.clone(),
            Arc::clone(&app.clock),
        );
        // AG-UI run-agent endpoint shares the WS state (same registrations
        // store, same topics) so HTTP→WS bridging routes correctly.
        let agui_routes = super::routes::agui::routes(ws_state);

        let router = Router::new()
            .route("/", get(|| async { Redirect::temporary("/ui") }))
            .route("/api/v1/health", get(health::health))
            .route("/api/openapi.json", get(openapi_json))
            .route("/api/docs", get(swagger_ui_html))
            .route("/api/docs/", get(swagger_ui_html))
            .nest("/ui", ui_routes)
            .nest("/otel/{project_id}/v1", otlp_routes)
            .nest("/api/v1/auth", auth_routes)
            .nest("/api/v1/project/{project_id}/otel", otel_query_routes)
            .nest("/api/v1/projects", projects_routes)
            .nest("/api/v1/organizations", organizations_routes)
            .nest("/api/v1/users", users_routes)
            .nest("/api/v1/pricing", pricing_routes)
            .nest("/api/v1/project/{project_id}/favorites", favorites_routes)
            .nest("/api/v1/organizations/{org_id}/api-keys", api_keys_routes)
            .nest(
                "/api/v1/organizations/{org_id}/credentials",
                credentials_routes,
            )
            .nest("/api/v1/project/{project_id}/files", api_files_routes)
            // SDK runtime routes carry agent manifests and control messages, so
            // they share the authenticated project boundary.
            .nest(
                "/api/v1",
                ws_routes.layer(axum::middleware::from_fn_with_state(
                    AuthState {
                        auth_manager: auth_manager.clone(),
                        allowed_origins: allowed_origins.clone(),
                        database: app.database.clone(),
                        api_key_secret: api_key_secret.clone(),
                        clock: app.clock.clone(),
                    },
                    require_auth,
                )),
            )
            .nest(
                "/api/v1",
                agui_routes.layer(axum::middleware::from_fn_with_state(
                    AuthState {
                        auth_manager: auth_manager.clone(),
                        allowed_origins: allowed_origins.clone(),
                        database: app.database.clone(),
                        api_key_secret: api_key_secret.clone(),
                        clock: app.clock.clone(),
                    },
                    require_auth,
                )),
            );

        let router = if let Some(mcp) = mcp_routes {
            router.nest("/api/v1/projects/{project_id}/mcp", mcp)
        } else {
            router
        };

        let router = router
            .fallback(middleware::handle_404)
            .layer(CompressionLayer::new())
            // Decompress request bodies. gzip is part of OTLP/HTTP and clients use it: the
            // OpenTelemetry JS exporter compresses above a size threshold, and the Claude Code
            // CLI compresses too. Without this layer those requests reached the decoder as raw
            // gzip bytes and were rejected with 400 "Failed to decode JSON request" - so the
            // documented no-SDK JavaScript path failed for every payload large enough to be
            // compressed, while small ones happened to work.
            .layer(RequestDecompressionLayer::new())
            .layer(middleware::cors(&allowed_origins))
            .layer(DefaultBodyLimit::max(DEFAULT_BODY_LIMIT));

        let listener = TcpListener::bind(addr).await?;
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let mut shutdown_rx = shutdown_rx;
            let _ = shutdown_rx.wait_for(|&triggered| triggered).await;
        })
        .await?;

        Ok(())
    }
}

/// Build files API routes
fn files_routes(file_service: Arc<FileService>) -> Router<()> {
    let state = FilesApiState { file_service };

    Router::new()
        .route("/{hash}", get(get_file).head(head_file))
        .with_state(state)
}
