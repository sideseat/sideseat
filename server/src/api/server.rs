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

use super::auth::AuthManager;
use super::auth::{AuthState, OtelAuthState, otel_auth_middleware, require_auth};
use super::embedded;
use super::middleware::{self, AllowedOrigins};
use super::openapi::{openapi_json, swagger_ui_html};
use super::rate_limit::{KeyExtractor, RateLimitState, rate_limit_middleware};
use super::routes::otel::files::{FilesApiState, get_file, head_file};
use super::routes::{
    api_keys, auth, favorites, health, organizations, otel, otlp_collector, pricing, projects,
    users,
};
use crate::core::CoreApp;
use crate::core::constants::{AUTH_BODY_LIMIT, DEFAULT_BODY_LIMIT, OTLP_BODY_LIMIT};
use crate::data::cache::RateLimitBucket;
use crate::data::files::FileService;

pub struct ApiServer {
    app: CoreApp,
    auth_manager: Arc<AuthManager>,
    allowed_origins: AllowedOrigins,
}

impl ApiServer {
    pub fn new(app: CoreApp) -> Self {
        let auth_manager = app.auth.clone();
        let allowed_origins = AllowedOrigins::new(&app.config.server.host, app.config.server.port);

        Self {
            app,
            auth_manager,
            allowed_origins,
        }
    }

    /// Returns CoreApp for graceful shutdown
    pub async fn start(self) -> Result<CoreApp> {
        let Self {
            app,
            auth_manager,
            allowed_origins,
        } = self;

        // Clone shutdown before moving app
        let shutdown = app.shutdown.clone();

        let host = app.config.server.host.clone();
        let port = app.config.server.port;
        let addr = SocketAddr::new(host.parse()?, port);

        // Use debug directory if debug mode is enabled (directory is created in app.rs)
        let debug_path = if app.config.debug {
            Some(app.storage.subdir(crate::core::storage::DataSubdir::Debug))
        } else {
            None
        };

        let ui_routes = Router::new().fallback(embedded::serve_assets);

        // Get API key secret early (needed for AuthState)
        let api_key_secret = app.secrets.get_api_key_secret().await?;

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
            };

        // Build OTLP ingestion routes (rate limited by project, optionally auth required)
        let otlp_routes = otlp_collector::routes(&app.topics, debug_path)
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
                database: app.database.clone(),
                cache: app.cache.clone(),
                api_key_secret: api_key_secret.clone(),
                otel_auth_required: app.config.otel.auth_required,
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
            app.shutdown.subscribe(),
        )
        .layer(axum::middleware::from_fn_with_state(
            AuthState {
                auth_manager: auth_manager.clone(),
                allowed_origins: allowed_origins.clone(),
                database: app.database.clone(),
                cache: app.cache.clone(),
                api_key_secret: api_key_secret.clone(),
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
        )
        .layer(axum::middleware::from_fn_with_state(
            AuthState {
                auth_manager: auth_manager.clone(),
                allowed_origins: allowed_origins.clone(),
                database: app.database.clone(),
                cache: app.cache.clone(),
                api_key_secret: api_key_secret.clone(),
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
        )
        .layer(axum::middleware::from_fn_with_state(
            AuthState {
                auth_manager: auth_manager.clone(),
                allowed_origins: allowed_origins.clone(),
                database: app.database.clone(),
                cache: app.cache.clone(),
                api_key_secret: api_key_secret.clone(),
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
                    cache: app.cache.clone(),
                    api_key_secret: api_key_secret.clone(),
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
                    cache: app.cache.clone(),
                    api_key_secret: api_key_secret.clone(),
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
                    cache: app.cache.clone(),
                    api_key_secret: api_key_secret.clone(),
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
                cache: app.cache.clone(),
                api_key_secret: api_key_secret.clone(),
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

        // Build files routes (rate limited by project)
        let api_files_routes =
            files_routes(app.files.clone()).layer(axum::middleware::from_fn_with_state(
                AuthState {
                    auth_manager,
                    allowed_origins: allowed_origins.clone(),
                    database: app.database.clone(),
                    cache: app.cache.clone(),
                    api_key_secret,
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

        // Build MCP routes if enabled (no auth, rate limited by IP)
        let mcp_routes = if app.config.mcp.enabled {
            let ct = super::mcp::cancellation_token_from_shutdown(&shutdown);
            let mcp = super::mcp::routes(app.analytics.clone(), ct);
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
            .nest("/api/v1/project/{project_id}/files", api_files_routes);

        let router = if let Some(mcp) = mcp_routes {
            router.nest("/api/v1/projects/{project_id}/mcp", mcp)
        } else {
            router
        };

        let router = router
            .fallback(middleware::handle_404)
            .layer(CompressionLayer::new())
            .layer(middleware::cors(&allowed_origins))
            .layer(DefaultBodyLimit::max(DEFAULT_BODY_LIMIT));

        let listener = TcpListener::bind(addr).await?;
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown.wait())
        .await?;

        Ok(app)
    }
}

/// Build files API routes
fn files_routes(file_service: Arc<FileService>) -> Router<()> {
    let state = FilesApiState { file_service };

    Router::new()
        .route("/{hash}", get(get_file).head(head_file))
        .with_state(state)
}
