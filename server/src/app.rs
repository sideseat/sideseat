//! Core application

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::api::{ApiServer, AuthManager, OtlpGrpcServer};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;

use crate::core::TopicService;
use crate::core::banner;
use crate::core::cli::{self, CliConfig, Commands, SystemCommands};
use crate::core::config::AppConfig;
use crate::core::constants::{APP_NAME_LOWER, ENV_LOG, TOPIC_TRACES};
use crate::core::shutdown::ShutdownService;
use crate::core::storage::AppStorage;
use crate::core::update;
use crate::data::cache::{CacheService, RateLimiter};
use crate::data::files::FileService;
use crate::data::secrets::SecretManager;
use crate::data::{AnalyticsService, TransactionalService};
use crate::domain::pricing::PricingService;
use crate::domain::providers::CredentialService;

pub struct CoreApp {
    pub shutdown: ShutdownService,
    pub config: AppConfig,
    pub storage: AppStorage,
    pub secrets: SecretManager,
    pub database: Arc<TransactionalService>,
    pub analytics: Arc<AnalyticsService>,
    pub pricing: Arc<PricingService>,
    pub auth: Arc<AuthManager>,
    pub topics: Arc<TopicService>,
    pub files: Arc<FileService>,
    pub cache: Arc<CacheService>,
    pub rate_limiter: Arc<RateLimiter>,
    pub credentials: Arc<CredentialService>,
}

impl CoreApp {
    /// Run the application with CLI argument parsing
    pub async fn run() -> Result<()> {
        dotenvy::dotenv().ok();
        Self::init_logging();

        tracing::debug!("Application starting");

        let (cli_config, command) = cli::parse();
        tracing::trace!(command = ?command, "Parsed command");

        match command {
            Some(Commands::System {
                command: system_cmd,
            }) => {
                return Self::handle_system_command(system_cmd);
            }
            Some(Commands::Start) | None => {}
        }

        let app = Self::init(&cli_config).await?;
        Self::start_server(app).await
    }

    async fn init(cli: &CliConfig) -> Result<Self> {
        let config = AppConfig::load(cli)?;
        let storage = AppStorage::init(&config).await?;
        let secrets = SecretManager::init(&storage, &config.secrets).await?;
        secrets.ensure_secrets().await?;

        // Initialize cache service
        let cache = Arc::new(
            CacheService::new(&config.database.cache_config())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to initialize cache service: {}", e))?,
        );

        tracing::debug!(backend = cache.backend_name(), "Cache initialized");

        // Initialize rate limiter
        let rate_limiter = Arc::new(RateLimiter::new(cache.clone()));

        let (database, analytics) = tokio::try_join!(
            async {
                TransactionalService::init(
                    config.database.transactional,
                    &storage,
                    config.database.postgres.as_ref(),
                )
                .await
                .map_err(anyhow::Error::from)
            },
            async {
                AnalyticsService::init(
                    config.database.analytics,
                    &storage,
                    config.database.clickhouse.as_ref(),
                )
                .await
                .map_err(anyhow::Error::from)
            },
        )?;

        let database = Arc::new(database);
        let analytics = Arc::new(analytics);
        let pricing = PricingService::init(&storage, config.pricing.sync_hours)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize pricing service: {}", e))?;
        let auth = Arc::new(AuthManager::init(&secrets, config.auth.enabled).await?);
        let topics = Arc::new(
            crate::data::topics::TopicService::from_cache_config(&config.database.cache_config())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to initialize topic service: {}", e))?,
        );

        tracing::debug!(backend = topics.backend_name(), "Topics initialized");
        let files = Arc::new(
            FileService::new(
                config.files.clone(),
                &storage,
                database.clone(),
                cache.clone(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize file service: {}", e))?,
        );
        // No startup sweep here on purpose.
        //
        // Advancing pending deletions used to run inline before the server was listening, which makes every
        // new instance wait for work that is not urgent - and on a horizontally scaled deployment, one
        // instance's slow sweep delays its own readiness while other instances are already sweeping. The
        // periodic task in `start_background_tasks` picks it up, and its first tick comes soon enough that
        // nothing is deferred meaningfully.

        let shutdown = ShutdownService::new(topics.clone(), database.clone(), analytics.clone());

        let credentials = CredentialService::new(
            database.clone(),
            secrets.clone(),
            cache.clone(),
            config.credentials.scan_env,
        );

        Ok(Self {
            config,
            storage,
            secrets,
            database,
            analytics,
            pricing,
            auth,
            topics,
            shutdown,
            files,
            cache,
            rate_limiter,
            credentials,
        })
    }

    fn handle_system_command(cmd: SystemCommands) -> Result<()> {
        match cmd {
            SystemCommands::Prune { yes } => Self::prune_data(yes),
        }
    }

    fn prune_data(skip_confirm: bool) -> Result<()> {
        let data_dir = AppStorage::resolve_data_dir();

        if !data_dir.exists() {
            println!(
                "Nothing to prune. Data directory does not exist: {}",
                data_dir.display()
            );
            return Ok(());
        }

        let data_dir = data_dir.canonicalize().unwrap_or(data_dir);

        println!("This will permanently delete the local data directory:");
        println!("  {}", data_dir.display());
        println!();
        println!(
            "Make sure the server is not running. \
             Deleting data while the server is running will cause data corruption."
        );

        if !skip_confirm {
            print!("\nContinue? [y/N] ");
            std::io::Write::flush(&mut std::io::stdout())?;

            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;

            if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                println!("Aborted.");
                return Ok(());
            }
        }

        std::fs::remove_dir_all(&data_dir)
            .with_context(|| format!("Failed to delete data directory: {}", data_dir.display()))?;
        println!("Pruned: {}", data_dir.display());
        Ok(())
    }

    fn init_logging() {
        let default_filter = format!("info,{}=info", APP_NAME_LOWER);

        let filter = std::env::var(ENV_LOG)
            .or_else(|_| std::env::var("RUST_LOG"))
            .unwrap_or(default_filter);

        tracing_subscriber::fmt()
            .with_target(false)
            .with_thread_ids(false)
            .with_level(true)
            .with_ansi(true)
            .compact()
            .with_env_filter(filter)
            .init();
    }

    async fn start_server(app: Self) -> Result<()> {
        // Install signal handlers FIRST (before any blocking calls)
        app.shutdown.install_signal_handlers();

        // Spawn update check (runs in background, prints notification when ready)
        if app.config.update.enabled {
            tokio::spawn(async {
                if let Some(new_version) = update::check_for_update().await {
                    banner::print_update_available(update::current_version(), &new_version);
                }
            });
        } else {
            tracing::debug!("Update check disabled by config");
        }

        app.start_background_tasks().await?;

        // Start OTLP gRPC server if enabled
        if app.config.otel.grpc_enabled {
            // Parsed before the server is built, so a bad entry refuses startup here as it does for HTTP -
            // see `utils::client_ip` for why skipping one is harmful rather than merely lax.
            let grpc_trusted_proxies = Arc::new(
                crate::utils::client_ip::TrustedProxies::parse(
                    &app.config.rate_limit.trusted_proxies,
                )
                .map_err(|e| anyhow::anyhow!(e))?,
            );
            let grpc_server = OtlpGrpcServer::new(
                &app.config.otel,
                &app.config.server.host,
                &app.topics,
                &app.storage,
                crate::api::routes::otlp_collector::IngestStores {
                    analytics: Arc::clone(&app.analytics),
                    database: Arc::clone(&app.database),
                    // Only where the queue cannot promise durability - then this transport writes in
                    // the request too, as the HTTP one does.
                    trace_pipeline: (!app.topics.is_durable()).then(|| {
                        Arc::new(crate::domain::TracePipeline::new(
                            app.analytics.clone(),
                            app.pricing.clone(),
                            app.topics.clone(),
                            app.files.clone(),
                        ))
                    }),
                },
                app.config.debug,
                // The same gate the HTTP transport applies. Built here rather than inside the gRPC server so
                // it shares the one API-key secret: a second read could pick up a *replacement* secret if the
                // backend had regenerated one, and a key hashed under the other pepper verifies nowhere.
                if app.config.otel.auth_required {
                    Some(crate::api::routes::otlp_collector::GrpcIngestAuth {
                        cache: Arc::clone(&app.cache),
                        database: Arc::clone(&app.database),
                        api_key_secret: Arc::new(app.secrets.get_api_key_secret().await?),
                        // Both switches, as the HTTP path reads them: `per_ip` alone ignored the master
                        // `enabled`, so a deployment that had turned rate limiting off still had it enforced
                        // on this transport only.
                        rate_limiter: (app.config.rate_limit.enabled
                            && app.config.rate_limit.per_ip)
                            .then(|| Arc::clone(&app.rate_limiter)),
                        trusted_proxies: Arc::clone(&grpc_trusted_proxies),
                    })
                } else {
                    None
                },
            )?;
            let shutdown_rx = app.shutdown.subscribe();
            let handle = tokio::spawn(async move {
                if let Err(e) = grpc_server.start(shutdown_rx).await {
                    tracing::error!(error = %e, "OTLP gRPC server error");
                }
            });

            app.shutdown.register(handle).await;
        }

        banner::print_banner(
            &app.config.server.host,
            app.config.server.port,
            app.auth.is_enabled(),
            app.auth.bootstrap_token(),
            app.config.otel.grpc_enabled,
            app.config.otel.grpc_port,
            &app.storage.data_dir().display().to_string(),
            app.config.mcp.enabled,
        );

        let server = ApiServer::new(app);
        let app = server.start().await?;
        app.shutdown.shutdown().await;

        Ok(())
    }

    pub async fn start_background_tasks(&self) -> Result<()> {
        self.shutdown
            .register(
                self.secrets
                    .start_health_check_task(self.shutdown.subscribe()),
            )
            .await;

        self.shutdown
            .register(
                self.database
                    .start_checkpoint_task(self.shutdown.subscribe()),
            )
            .await;

        self.shutdown
            .register(
                self.analytics
                    .start_checkpoint_task(self.shutdown.subscribe()),
            )
            .await;

        if let Some(h) = self.analytics.start_retention_task(
            self.config.otel.retention.clone(),
            self.shutdown.subscribe(),
            Some(Arc::clone(&self.files)),
            Arc::clone(&self.database),
        ) {
            self.shutdown.register(h).await;
        }

        if let Some(h) = self
            .pricing
            .start_sync_task(self.config.pricing.sync_hours, self.shutdown.subscribe())
        {
            self.shutdown.register(h).await;
        }

        // Claims a crash abandoned. Startup swept once already, and that is not enough on its own: a
        // claim taken just before the crash reads as a deletion in progress when the process returns.
        self.shutdown
            .register(crate::data::cleanup::start_claim_recovery_task(
                Arc::clone(&self.database),
                Arc::clone(&self.analytics),
                Arc::clone(&self.files),
                self.shutdown.subscribe(),
            ))
            .await;

        // Create stream topic for traces (at-least-once delivery with consumer groups)
        let traces_topic = self
            .topics
            .stream_topic::<ExportTraceServiceRequest>(TOPIC_TRACES);

        let pipeline = crate::domain::TracePipeline::new(
            self.analytics.clone(),
            self.pricing.clone(),
            self.topics.clone(),
            self.files.clone(),
        );

        self.shutdown
            .register(pipeline.start(traces_topic, self.shutdown.subscribe()))
            .await;

        // No metrics pipeline: metrics are written inside their request, so a 200 means they are stored.
        // See `domain::metrics::ingest` for why traces keep a queue and metrics do not.

        tracing::debug!("Background tasks started");
        Ok(())
    }
}
