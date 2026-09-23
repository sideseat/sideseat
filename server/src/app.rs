//! Application composition root and lifecycle orchestration.

pub mod files;
pub mod providers;
pub mod storage;
mod system_commands;
mod update;

use std::sync::Arc;

use anyhow::Result;

use sideseat_api::{ApiDependencies, ApiServer, AuthManager, OtlpGrpcServer};

use self::files::{create_governed_file_service, create_governed_file_service_deferred_cleanup};
use self::storage::{AnalyticsService, TransactionalService};
use crate::runtime::clock::SystemClock;
use crate::runtime::shutdown::ShutdownService;
use sideseat_adapter_cache::CacheService;
use sideseat_adapter_pricing::LiteLlmPricingSource;
use sideseat_adapter_secrets::SecretManager;
use sideseat_core::banner;
use sideseat_core::cli::{self, CliConfig, Commands, SystemCommands};
use sideseat_core::config::AppConfig;
use sideseat_core::constants::{APP_NAME_LOWER, ENV_LOG, TOPIC_TRACES};
use sideseat_core::storage::AppStorage;
use sideseat_domain::files::FileService;
use sideseat_domain::pricing::PricingService;
use sideseat_domain::providers::CredentialService;
use sideseat_domain::rate_limit::RateLimiter;
use sideseat_domain::storage_governance::StorageGovernanceService;
use sideseat_ingestion::staging::{StagedPayloadRef, StagingService};
use sideseat_messaging::TopicService;
use sideseat_ports::cache::CacheStore;
use sideseat_ports::clock::Clock;
use sideseat_ports::pricing::PricingCatalogueSource;
use sideseat_ports::registrations::RegistrationStore;
use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitMode {
    Server,
    RestoreRepair,
}

pub struct CoreApp {
    pub shutdown: ShutdownService,
    pub config: AppConfig,
    pub storage: AppStorage,
    pub secrets: SecretManager,
    pub database: Arc<TransactionalService>,
    pub database_port: Arc<dyn TransactionalRepository + Send + Sync>,
    pub analytics: Arc<AnalyticsService>,
    pub analytics_port: Arc<dyn AnalyticsRepository + Send + Sync>,
    pub pricing: Arc<PricingService>,
    pub auth: Arc<AuthManager>,
    pub topics: Arc<TopicService>,
    pub files: Arc<FileService>,
    pub staging: Arc<StagingService>,
    pub storage_governance: Arc<StorageGovernanceService>,
    pub cache: Arc<CacheService>,
    pub cache_port: Arc<dyn CacheStore>,
    pub rate_limiter: Arc<RateLimiter>,
    pub credentials: Arc<CredentialService>,
    pub clock: Arc<dyn Clock>,
}

impl CoreApp {
    /// Parse the command line and run the selected application mode.
    pub async fn run() -> Result<()> {
        dotenvy::dotenv().ok();
        Self::init_logging();

        tracing::debug!("Application starting");

        let (cli_config, command) = cli::parse();
        tracing::trace!(command = ?command, "Parsed command");

        match command {
            Some(Commands::System {
                command: SystemCommands::Prune { yes },
            }) => return system_commands::prune_data(yes),
            Some(Commands::System {
                command: SystemCommands::RestoreRepair { report },
            }) => {
                system_commands::mark_restore_pending().await?;
                let app = Self::init(&cli_config, InitMode::RestoreRepair).await?;
                return system_commands::run_restore_repair(app, report.as_deref()).await;
            }
            Some(Commands::Start) | None => {}
        }

        let app = Self::init(&cli_config, InitMode::Server).await?;
        Self::start_server(app).await
    }

    async fn init(cli: &CliConfig, mode: InitMode) -> Result<Self> {
        let config = AppConfig::load(cli)?;
        if mode == InitMode::Server {
            system_commands::refuse_pending_restore()?;
        }
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);

        // Compile embedded rule assets before accepting traffic.
        let _ = sideseat_domain::rules::ruleset();
        let storage = AppStorage::init(&config).await?;
        let secrets = SecretManager::init(&storage, &config.secrets, Arc::clone(&clock)).await?;
        secrets.ensure_secrets().await?;

        let cache = Arc::new(
            CacheService::new(&config.database.cache_config())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to initialize cache service: {}", e))?,
        );

        tracing::debug!(backend = cache.backend_name(), "Cache initialized");

        let rate_limiter = Arc::new(RateLimiter::new(cache.clone(), Arc::clone(&clock)));

        let (database, analytics) = tokio::try_join!(
            async {
                TransactionalService::init(
                    config.database.transactional,
                    &storage,
                    config.database.postgres.as_ref(),
                    Some(cache.clone()),
                    Arc::clone(&clock),
                )
                .await
                .map_err(anyhow::Error::from)
            },
            async {
                AnalyticsService::init(
                    config.database.analytics,
                    &storage,
                    config.database.clickhouse.as_ref(),
                    Arc::clone(&clock),
                )
                .await
                .map_err(anyhow::Error::from)
            },
        )?;

        let database = Arc::new(database);
        let analytics = Arc::new(analytics);
        let database_port = Arc::from(database.repository());
        let governance_port = Arc::from(database.governance_repository());
        let analytics_port = Arc::from(analytics.repository());
        let cache_port: Arc<dyn CacheStore> = cache.clone();
        let pricing_source: Option<Arc<dyn PricingCatalogueSource>> = (config.pricing.sync_hours
            > 0)
        .then(LiteLlmPricingSource::new)
        .transpose()
        .map_err(|e| anyhow::anyhow!("Failed to initialize pricing sync: {}", e))?
        .map(|source| Arc::new(source) as Arc<dyn PricingCatalogueSource>);
        let pricing = PricingService::init(&storage, Arc::clone(&clock), pricing_source)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize pricing service: {}", e))?;
        let auth = Arc::new(AuthManager::new(
            secrets.get_jwt_signing_key().await?,
            config.auth.enabled,
            Arc::clone(&clock),
        ));
        let topic_backend =
            sideseat_adapter_topics::backend_from_queue_config(&config.database.queue_config())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to initialize topic service: {}", e))?;
        let topics = Arc::new(TopicService::new(topic_backend));

        tracing::debug!(backend = topics.backend_name(), "Topics initialized");
        let storage_governance = Arc::new(StorageGovernanceService::new(
            Arc::clone(&database_port),
            governance_port,
            Arc::clone(&analytics_port),
            Arc::clone(&clock),
            config.files.quota_bytes,
        ));
        storage_governance
            .validate_startup()
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        let files = Arc::new(
            if mode == InitMode::RestoreRepair {
                create_governed_file_service_deferred_cleanup(
                    config.files.clone(),
                    &storage,
                    database.clone(),
                    cache.clone(),
                    Arc::clone(&storage_governance),
                )
                .await
            } else {
                create_governed_file_service(
                    config.files.clone(),
                    &storage,
                    database.clone(),
                    cache.clone(),
                    Arc::clone(&storage_governance),
                )
                .await
            }
            .map_err(|e| anyhow::anyhow!("Failed to initialize file service: {}", e))?,
        );
        let staging = Arc::new(StagingService::new(
            Arc::clone(files.storage()),
            Arc::clone(&database_port),
            Arc::clone(&analytics_port),
            Arc::clone(&clock),
            config.otel.retention.clone(),
            config.otel.staging_redrive_cap,
        ));
        // Pending deletion cleanup runs in the background so readiness does not depend on a sweep.

        let shutdown = ShutdownService::new(topics.clone(), database.clone(), analytics.clone());

        let credentials = CredentialService::new(
            Arc::from(database.repository()),
            Arc::new(secrets.clone()),
            cache.clone(),
            config.credentials.scan_env,
        );

        Ok(Self {
            config,
            storage,
            secrets,
            database,
            database_port,
            analytics,
            analytics_port,
            pricing,
            auth,
            topics,
            shutdown,
            files,
            staging,
            storage_governance,
            cache,
            cache_port,
            rate_limiter,
            credentials,
            clock,
        })
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
        // Install signal handlers before spawning services or background tasks.
        app.shutdown.install_signal_handlers();

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
        let api_key_secret = app.secrets.get_api_key_secret().await?;

        if app.config.otel.grpc_enabled {
            // Reject invalid proxy ranges before either transport starts.
            let grpc_trusted_proxies = Arc::new(
                sideseat_core::utils::client_ip::TrustedProxies::parse(
                    &app.config.rate_limit.trusted_proxies,
                )
                .map_err(|e| anyhow::anyhow!(e))?,
            );
            let grpc_server = OtlpGrpcServer::new(
                &app.config.otel,
                &app.config.server.host,
                &app.topics,
                &app.storage,
                sideseat_api::routes::otlp_collector::IngestStores {
                    analytics: Arc::clone(&app.analytics_port),
                    database: Arc::clone(&app.database_port),
                    // Non-durable queues require synchronous persistence before acknowledging.
                    trace_pipeline: (!app.topics.is_durable()).then(|| {
                        Arc::new(
                            sideseat_ingestion::traces::TracePipeline::new(
                                Arc::clone(&app.analytics_port),
                                app.pricing.clone(),
                                app.topics.clone(),
                                app.files.clone(),
                                Arc::clone(&app.staging),
                            )
                            .with_storage_governance(Arc::clone(&app.storage_governance)),
                        )
                    }),
                    clock: Arc::clone(&app.clock),
                    staging: Arc::clone(&app.staging),
                    storage_governance: Arc::clone(&app.storage_governance),
                },
                app.config.debug,
                sideseat_api::routes::otlp_collector::GrpcIngestGuards {
                    // HTTP and gRPC must share one API-key hashing secret for this process.
                    auth: if app.config.otel.auth_required {
                        Some(sideseat_api::routes::otlp_collector::GrpcIngestAuth {
                            cache: Arc::clone(&app.cache_port),
                            database: Arc::clone(&app.database_port),
                            api_key_secret: Arc::new(api_key_secret.clone()),
                            rate_limiter: (app.config.rate_limit.enabled
                                && app.config.rate_limit.per_ip)
                                .then(|| Arc::clone(&app.rate_limiter)),
                            trusted_proxies: Arc::clone(&grpc_trusted_proxies),
                            clock: Arc::clone(&app.clock),
                        })
                    } else {
                        None
                    },
                    // Project ingestion limits apply independently of authentication.
                    limit: (app.config.rate_limit.enabled).then(|| {
                        sideseat_api::routes::otlp_collector::GrpcIngestLimit {
                            limiter: Arc::clone(&app.rate_limiter),
                            ingestion_rpm: app.config.rate_limit.ingestion_rpm,
                        }
                    }),
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

        let shutdown = app.shutdown.clone();
        let registrations: Arc<dyn RegistrationStore> =
            Arc::new(sideseat_adapter_registrations_memory::MemoryRegistrationStore::new());
        let server = ApiServer::new(ApiDependencies {
            config: app.config.clone(),
            storage: app.storage.clone(),
            database: app.database_port.clone(),
            analytics: app.analytics_port.clone(),
            pricing: app.pricing.clone(),
            auth: app.auth.clone(),
            topics: app.topics.clone(),
            files: app.files.clone(),
            staging: app.staging.clone(),
            storage_governance: app.storage_governance.clone(),
            cache: app.cache_port.clone(),
            rate_limiter: app.rate_limiter.clone(),
            credentials: app.credentials.clone(),
            credential_tester: Arc::new(providers::SdkCredentialConnectionTester),
            registrations,
            api_key_secret,
            clock: app.clock.clone(),
            shutdown_rx: app.shutdown.subscribe(),
        });
        server.start().await?;
        shutdown.shutdown().await;

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

        // ClickHouse reports cross-month duplicate residuals; DuckDB has no partition check.
        if let Some(h) = self
            .analytics
            .start_consistency_check_task(self.shutdown.subscribe())
        {
            self.shutdown.register(h).await;
        }

        if let Some(h) = self.analytics.start_retention_task(
            self.config.otel.retention.clone(),
            self.config.files.quota_bytes,
            self.shutdown.subscribe(),
            Some(Arc::clone(&self.files)),
            Arc::clone(&self.database),
            Arc::from(self.database.governance_repository()),
        ) {
            self.shutdown.register(h).await;
        }

        if let Some(h) = self
            .pricing
            .start_sync_task(self.config.pricing.sync_hours, self.shutdown.subscribe())
        {
            self.shutdown.register(h).await;
        }

        // Recover deletion claims abandoned by a crashed worker.
        self.shutdown
            .register(sideseat_domain::cleanup::start_claim_recovery_task(
                Arc::clone(&self.database_port),
                Arc::clone(&self.analytics_port),
                Arc::clone(&self.files),
                self.shutdown.subscribe(),
            ))
            .await;

        let traces_topic = self
            .topics
            .stream_topic::<StagedPayloadRef>(TOPIC_TRACES, StagedPayloadRef::partition_key);

        let pipeline = Arc::new(
            sideseat_ingestion::traces::TracePipeline::new(
                Arc::from(self.analytics.repository()),
                self.pricing.clone(),
                self.topics.clone(),
                self.files.clone(),
                Arc::clone(&self.staging),
            )
            .with_storage_governance(Arc::clone(&self.storage_governance)),
        );

        self.shutdown
            .register(Arc::clone(&pipeline).start(traces_topic, self.shutdown.subscribe()))
            .await;
        self.shutdown
            .register(sideseat_ingestion::staging::start_staging_sweep(
                Arc::clone(&self.staging),
                pipeline,
                self.shutdown.subscribe(),
            ))
            .await;
        self.shutdown
            .register(Arc::clone(&self.storage_governance).start(self.shutdown.subscribe()))
            .await;
        self.shutdown
            .register(
                Arc::new(
                    sideseat_domain::content_bodies::ContentBodyService::from_file_service(
                        &self.files,
                    ),
                )
                .start_backfill_task(
                    Arc::clone(&self.analytics_port),
                    Arc::clone(&self.clock),
                    self.shutdown.subscribe(),
                ),
            )
            .await;

        // No metrics pipeline: metrics are written inside their request, so a 200 means they are stored.
        // See `domain::metrics::ingest` for why traces keep a queue and metrics do not.

        tracing::debug!("Background tasks started");
        Ok(())
    }
}
