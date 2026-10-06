use super::*;

impl AppConfig {
    /// Load configuration from all sources
    ///
    /// Priority (lowest to highest):
    /// 1. Defaults
    /// 2. Profile directory config (~/.sideseat/sideseat.json)
    /// 3. Local directory config OR CLI-specified config path
    /// 4. CLI arguments (which include env var fallbacks via clap)
    pub fn load(cli: &CliConfig) -> Result<Self, ConfigError> {
        tracing::debug!("Loading application configuration");
        tracing::trace!(cli = ?cli, "CLI config");

        let mut file_config = FileConfig::default();
        let mut found_configs: Vec<String> = Vec::new();

        // 1. Load from profile dir (~/.sideseat/sideseat.json) - skip if not exists
        if let Some(profile_path) = get_profile_config_path()
            && profile_path.exists()
        {
            let profile_config = FileConfig::load_from_file(&profile_path)?;
            profile_config.warn_unknown_fields();
            file_config.merge(profile_config);
            found_configs.push(profile_path.display().to_string());
        }

        // 2. Load from CLI-specified path OR local directory
        let overlay_path = if let Some(ref path) = cli.config {
            let expanded = expand_path(&path.to_string_lossy());
            if !expanded.exists() {
                return Err(ConfigError::NotFound { path: expanded });
            }
            Some(expanded)
        } else {
            let local = PathBuf::from(CONFIG_FILE_NAME);
            if local.exists() { Some(local) } else { None }
        };

        if let Some(path) = overlay_path {
            let overlay_config = FileConfig::load_from_file(&path)?;
            overlay_config.warn_unknown_fields();
            file_config.merge(overlay_config);
            found_configs.push(path.display().to_string());
        }

        tracing::debug!(configs = ?found_configs, "Config files loaded");

        // 3. Extract file config values with defaults
        let file_server = file_config.server.unwrap_or_default();
        let file_auth = file_config.auth.unwrap_or_default();
        let file_otel = file_config.otel.unwrap_or_default();
        let file_grpc = file_otel.grpc.unwrap_or_default();
        let file_retention = file_otel.retention.unwrap_or_default();
        let file_otel_auth = file_otel.auth.unwrap_or_default();
        let file_pricing = file_config.pricing.unwrap_or_default();
        let file_files = file_config.files.unwrap_or_default();
        let file_rate_limit = file_config.rate_limit.unwrap_or_default();
        let file_update = file_config.update.unwrap_or_default();
        let file_mcp = file_server.mcp.unwrap_or_default();
        let file_credentials = file_config.credentials.unwrap_or_default();
        let file_database = file_config.database.unwrap_or_default();

        // 4. Layer configs: defaults -> file config -> CLI/env overrides
        let host = cli
            .host
            .clone()
            .or(file_server.host)
            .unwrap_or_else(|| DEFAULT_HOST.to_string());

        let port = cli.port.or(file_server.port).unwrap_or(DEFAULT_PORT);

        // auth.enabled: file config sets default, --no-auth CLI flag disables
        let auth_enabled = if cli.no_auth {
            false
        } else {
            file_auth.enabled.unwrap_or(true)
        };

        // otel.grpc config: CLI/env overrides file config
        let otel_grpc_enabled = cli.otel_grpc.or(file_grpc.enabled).unwrap_or(true);
        let otel_grpc_port = cli
            .otel_grpc_port
            .or(file_grpc.port)
            .unwrap_or(DEFAULT_OTEL_GRPC_PORT);

        // retention config: CLI/env overrides file config
        let retention = RetentionConfig {
            max_age_minutes: cli
                .otel_retention_max_age
                .or(file_retention.max_age_minutes),
            max_spans: cli
                .otel_retention_max_spans
                .or(file_retention.max_spans)
                .or(Some(DEFAULT_OTEL_RETENTION_MAX_SPANS)),
        };

        // otel.auth.required: CLI/env overrides file config, default false
        let otel_auth_required = cli
            .otel_auth_required
            .or(file_otel_auth.required)
            .unwrap_or(false);
        let staging_redrive_cap = file_otel
            .staging_redrive_cap
            .unwrap_or(DEFAULT_OTEL_STAGING_REDRIVE_CAP);

        // debug: CLI/env flag takes precedence, then file config, default false
        let debug = cli.debug || file_config.debug.unwrap_or(false);

        // pricing config: CLI/env overrides file config
        let default_sync_hours = PRICING_SYNC_INTERVAL_SECS / 3600;
        let pricing_sync_hours = cli
            .pricing_sync_hours
            .or(file_pricing.sync_hours)
            .unwrap_or(default_sync_hours);

        // files config: CLI/env overrides file config
        let storage_backend = cli.files_storage.or(file_files.storage).unwrap_or_default();

        let files_enabled = cli.files_enabled.or(file_files.enabled).unwrap_or(true);
        let files_quota_bytes = cli
            .files_quota_bytes
            .or(file_files.quota_bytes)
            .unwrap_or(FILES_DEFAULT_QUOTA_BYTES);

        // Parse S3 config if storage type is s3
        // CLI/env vars override config file values
        let s3_config = if storage_backend == StorageBackend::S3 {
            let file_s3 = file_files.s3.as_ref();
            let bucket = cli
                .files_s3_bucket
                .clone()
                .or_else(|| file_s3.and_then(|s| s.bucket.clone()));
            let prefix = cli
                .files_s3_prefix
                .clone()
                .or_else(|| file_s3.and_then(|s| s.prefix.clone()));
            let region = cli
                .files_s3_region
                .clone()
                .or_else(|| file_s3.and_then(|s| s.region.clone()));
            let endpoint = cli
                .files_s3_endpoint
                .clone()
                .or_else(|| file_s3.and_then(|s| s.endpoint.clone()));

            bucket.filter(|b| !b.is_empty()).map(|bucket| S3Config {
                bucket,
                prefix: prefix.unwrap_or_else(|| FILES_DEFAULT_S3_PREFIX.to_string()),
                region,
                endpoint,
            })
        } else {
            None
        };

        let files = FilesConfig {
            enabled: files_enabled,
            storage: storage_backend,
            quota_bytes: files_quota_bytes,
            filesystem_path: file_files.filesystem.and_then(|fs| fs.path),
            s3: s3_config,
        };

        // update config: CLI flag overrides file config, default enabled
        let update_enabled = if cli.no_update_check {
            false
        } else {
            file_update.enabled.unwrap_or(true)
        };

        // mcp config: CLI/env overrides file config, enabled by default
        let mcp_enabled = cli.mcp.or(file_mcp.enabled).unwrap_or(true);

        // credentials config: CLI/env overrides file config, scan_env enabled by default
        let credentials_scan_env = cli
            .credentials_scan_env
            .or(file_credentials.scan_env)
            .unwrap_or(true);

        // cache config: CLI/env overrides file config
        let cache_backend = cli
            .cache_backend
            .or(file_database.cache)
            .unwrap_or_default();
        // Preserve the old implicit coupling when no queue setting is present, while allowing either
        // side to be overridden independently.
        let queue_backend =
            cli.queue_backend
                .or(file_database.queue)
                .unwrap_or(match cache_backend {
                    CacheBackendType::Memory => QueueBackendType::Memory,
                    CacheBackendType::Redis => QueueBackendType::Redis,
                });

        // Memory cache config
        let file_memory_cache = file_database.memory_cache.unwrap_or_default();
        let cache_max_entries = cli
            .cache_max_entries
            .or(file_memory_cache.max_entries)
            .unwrap_or(DEFAULT_CACHE_MAX_ENTRIES);
        let cache_eviction_policy = cli
            .cache_eviction_policy
            .or(file_memory_cache.eviction_policy)
            .unwrap_or_default();
        let memory_cache_config = MemoryCacheConfig {
            max_entries: cache_max_entries,
            eviction_policy: cache_eviction_policy,
        };

        // Redis config (only populated if using redis backend)
        let redis_config = if cache_backend == CacheBackendType::Redis
            || queue_backend == QueueBackendType::Redis
        {
            let file_redis = file_database.redis.unwrap_or_default();
            let url = cli
                .cache_redis_url
                .clone()
                .or(file_redis.url)
                .unwrap_or_default();
            Some(RedisConfig {
                url,
                min_replica_acks: file_redis.min_replica_acks.unwrap_or(0),
            })
        } else {
            None
        };

        let redpanda_config = if queue_backend == QueueBackendType::Redpanda {
            let file_redpanda = file_database.redpanda.unwrap_or_default();
            Some(RedpandaConfig {
                brokers: cli
                    .redpanda_brokers
                    .clone()
                    .or(file_redpanda.brokers)
                    .unwrap_or_else(|| DEFAULT_REDPANDA_BROKERS.to_string()),
                partitions: file_redpanda
                    .partitions
                    .unwrap_or(DEFAULT_REDPANDA_PARTITIONS),
                replication_factor: file_redpanda
                    .replication_factor
                    .unwrap_or(DEFAULT_REDPANDA_REPLICATION_FACTOR),
                retention_ms: file_redpanda
                    .retention_ms
                    .unwrap_or(DEFAULT_REDPANDA_RETENTION_MS),
                retention_warning_ms: file_redpanda
                    .retention_warning_ms
                    .unwrap_or(DEFAULT_REDPANDA_RETENTION_WARNING_MS),
            })
        } else {
            None
        };

        // rate_limit config: CLI/env overrides file config
        let rate_limit_enabled = cli
            .rate_limit_enabled
            .or(file_rate_limit.enabled)
            .unwrap_or(true); // Enabled by default (per-project rate limiting)
        let rate_limit_per_ip = cli
            .rate_limit_per_ip
            .or(file_rate_limit.per_ip)
            .unwrap_or(false); // Per-IP rate limiting disabled by default
        let rate_limit_api_rpm = cli
            .rate_limit_api_rpm
            .or(file_rate_limit.api_rpm)
            .unwrap_or(DEFAULT_RATE_LIMIT_API_RPM);
        let rate_limit_ingestion_rpm = cli
            .rate_limit_ingestion_rpm
            .or(file_rate_limit.ingestion_rpm)
            .unwrap_or(DEFAULT_RATE_LIMIT_INGESTION_RPM);
        let rate_limit_auth_rpm = cli
            .rate_limit_auth_rpm
            .or(file_rate_limit.auth_rpm)
            .unwrap_or(DEFAULT_RATE_LIMIT_AUTH_RPM);
        let rate_limit_files_rpm = cli
            .rate_limit_files_rpm
            .or(file_rate_limit.files_rpm)
            .unwrap_or(DEFAULT_RATE_LIMIT_FILES_RPM);
        let rate_limit_bypass_header = cli
            .rate_limit_bypass_header
            .clone()
            .or(file_rate_limit.bypass_header);

        let rate_limit = RateLimitConfig {
            enabled: rate_limit_enabled,
            per_ip: rate_limit_per_ip,
            api_rpm: rate_limit_api_rpm,
            ingestion_rpm: rate_limit_ingestion_rpm,
            auth_rpm: rate_limit_auth_rpm,
            files_rpm: rate_limit_files_rpm,
            bypass_header: rate_limit_bypass_header,
            trusted_proxies: file_rate_limit.trusted_proxies.clone().unwrap_or_default(),
        };

        // database config: file config with env var overrides for sensitive values
        let transactional_backend = cli
            .transactional_backend
            .or(file_database.transactional)
            .unwrap_or_default();
        let analytics_backend = cli
            .analytics_backend
            .or(file_database.analytics)
            .unwrap_or_default();

        // PostgreSQL config (only populated if using postgres backend)
        // Optimized for scalable SaaS with connection pooling and query protection
        let postgres_config = if transactional_backend == TransactionalBackend::Postgres {
            let file_pg = file_database.postgres.unwrap_or_default();
            let url = cli
                .postgres_url
                .clone()
                .or_else(|| std::env::var("SIDESEAT_POSTGRES_URL").ok())
                .or(file_pg.url)
                .unwrap_or_default();
            Some(PostgresConfig {
                url,
                max_connections: file_pg
                    .max_connections
                    .unwrap_or(POSTGRES_DEFAULT_MAX_CONNECTIONS),
                min_connections: file_pg
                    .min_connections
                    .unwrap_or(POSTGRES_DEFAULT_MIN_CONNECTIONS),
                acquire_timeout_secs: file_pg
                    .acquire_timeout_secs
                    .unwrap_or(POSTGRES_DEFAULT_ACQUIRE_TIMEOUT_SECS),
                idle_timeout_secs: file_pg
                    .idle_timeout_secs
                    .unwrap_or(POSTGRES_DEFAULT_IDLE_TIMEOUT_SECS),
                max_lifetime_secs: file_pg
                    .max_lifetime_secs
                    .unwrap_or(POSTGRES_DEFAULT_MAX_LIFETIME_SECS),
                statement_timeout_secs: file_pg
                    .statement_timeout_secs
                    .unwrap_or(POSTGRES_DEFAULT_STATEMENT_TIMEOUT_SECS),
            })
        } else {
            None
        };

        // ClickHouse config (only populated if using clickhouse backend)
        let clickhouse_config = if analytics_backend == AnalyticsBackend::Clickhouse {
            let file_ch = file_database.clickhouse.unwrap_or_default();
            let url = cli
                .clickhouse_url
                .clone()
                .or_else(|| std::env::var("SIDESEAT_CLICKHOUSE_URL").ok())
                .or(file_ch.url)
                .unwrap_or_default();
            let database = file_ch.database.unwrap_or_else(|| "sideseat".to_string());
            let user = file_ch.user;
            let password = file_ch.password;
            let timeout_secs = file_ch.timeout_secs.unwrap_or(30);
            let compression = file_ch.compression.unwrap_or(true);
            // Direct inserts are the default.
            //
            // An acknowledgement means the data is durable, so asynchronous inserts must wait for their
            // flush. Waiting also pays the server-side flush delay while the caller remains blocked; local
            // measurements were 118.6 ms p50 per trace export versus 59.2 ms for direct inserts.
            //
            // The pipeline already batches ordinary writes. `async_insert` remains configurable for
            // deployments with writers that bypass that batching, and validation still requires waiting.
            let async_insert = file_ch.async_insert.unwrap_or(false);
            let wait_for_async_insert = file_ch.wait_for_async_insert.unwrap_or(true);
            let cluster = file_ch.cluster;
            // Preserve the requested value so validation can reject distributed mode without a cluster.
            let distributed = file_ch.distributed.unwrap_or(false);
            Some(ClickhouseConfig {
                url,
                database,
                user,
                password,
                timeout_secs,
                compression,
                async_insert,
                wait_for_async_insert,
                cluster,
                distributed,
                insert_quorum: file_ch.insert_quorum.unwrap_or(0),
            })
        } else {
            None
        };

        let database = DatabaseConfig {
            transactional: transactional_backend,
            analytics: analytics_backend,
            cache: cache_backend,
            queue: queue_backend,
            postgres: postgres_config,
            clickhouse: clickhouse_config,
            redis: redis_config,
            redpanda: redpanda_config,
            memory_cache: memory_cache_config,
        };

        // Secrets config: CLI > file > platform auto-detect
        let file_secrets = file_config.secrets.unwrap_or_default();

        let secrets_backend = cli
            .secrets_backend
            .or(file_secrets.backend)
            .unwrap_or_else(SecretsBackend::detect);

        let secrets_env = if secrets_backend == SecretsBackend::Env {
            let file_env = file_secrets.env.unwrap_or_default();
            Some(SecretsEnvConfig {
                prefix: std::env::var(ENV_SECRETS_ENV_PREFIX)
                    .ok()
                    .or(file_env.prefix)
                    .unwrap_or_else(|| SECRETS_DEFAULT_ENV_PREFIX.to_string()),
            })
        } else {
            None
        };

        let secrets_aws = if secrets_backend == SecretsBackend::Aws {
            let file_aws = file_secrets.aws.unwrap_or_default();
            Some(SecretsAwsConfig {
                region: std::env::var(ENV_SECRETS_AWS_REGION)
                    .ok()
                    .or(file_aws.region),
                prefix: std::env::var(ENV_SECRETS_AWS_PREFIX)
                    .ok()
                    .or(file_aws.prefix)
                    .unwrap_or_else(|| SECRETS_DEFAULT_AWS_PREFIX.to_string()),
                recovery_window_days: file_aws.recovery_window_days,
            })
        } else {
            None
        };

        let secrets_vault = if secrets_backend == SecretsBackend::Vault {
            let file_vault = file_secrets.vault.unwrap_or_default();
            Some(SecretsVaultConfig {
                address: std::env::var(ENV_SECRETS_VAULT_ADDR)
                    .ok()
                    .or(file_vault.address)
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_string(),
                mount: std::env::var(ENV_SECRETS_VAULT_MOUNT)
                    .ok()
                    .or(file_vault.mount)
                    .unwrap_or_else(|| SECRETS_DEFAULT_VAULT_MOUNT.to_string()),
                prefix: std::env::var(ENV_SECRETS_VAULT_PREFIX)
                    .ok()
                    .or(file_vault.prefix)
                    .unwrap_or_else(|| SECRETS_DEFAULT_VAULT_PREFIX.to_string()),
                token: std::env::var(ENV_SECRETS_VAULT_TOKEN)
                    .ok()
                    .or_else(|| std::env::var("VAULT_TOKEN").ok())
                    .or(file_vault.token)
                    .unwrap_or_default(),
            })
        } else {
            None
        };

        let secrets = SecretsConfig {
            backend: secrets_backend,
            env: secrets_env,
            aws: secrets_aws,
            vault: secrets_vault,
        };

        let config = Self {
            server: ServerConfig { host, port },
            auth: AuthConfig {
                enabled: auth_enabled,
            },
            otel: OtelConfig {
                grpc_enabled: otel_grpc_enabled,
                grpc_port: otel_grpc_port,
                retention,
                staging_redrive_cap,
                auth_required: otel_auth_required,
            },
            pricing: PricingConfig {
                sync_hours: pricing_sync_hours,
            },
            files,
            rate_limit,
            update: UpdateConfig {
                enabled: update_enabled,
            },
            mcp: McpConfig {
                enabled: mcp_enabled,
            },
            credentials: CredentialsConfig {
                scan_env: credentials_scan_env,
            },
            database,
            secrets,
            debug,
        };

        // Validate configuration
        config.validate()?;

        tracing::debug!(
            host = %config.server.host,
            port = config.server.port,
            auth_enabled = config.auth.enabled,
            debug = config.debug,
            otel_grpc_enabled = config.otel.grpc_enabled,
            otel_grpc_port = config.otel.grpc_port,
            retention_max_age_minutes = ?config.otel.retention.max_age_minutes,
            retention_max_spans = ?config.otel.retention.max_spans,
            staging_redrive_cap = config.otel.staging_redrive_cap,
            otel_auth_required = config.otel.auth_required,
            pricing_sync_hours = config.pricing.sync_hours,
            files_enabled = config.files.enabled,
            files_storage = %config.files.storage,
            files_quota_bytes = config.files.quota_bytes,
            cache_backend = %config.database.cache,
            cache_max_entries = config.database.memory_cache.max_entries,
            rate_limit_enabled = config.rate_limit.enabled,
            update_enabled = config.update.enabled,
            mcp_enabled = config.mcp.enabled,
            credentials_scan_env = config.credentials.scan_env,
            transactional_backend = %config.database.transactional,
            analytics_backend = %config.database.analytics,
            "Configuration loaded"
        );

        Ok(config)
    }

    /// Validate the configuration for consistency and correctness
    fn validate(&self) -> Result<(), ConfigError> {
        // Host must not be empty
        if self.server.host.is_empty() {
            return Err(ConfigError::Invalid(
                "server.host must not be empty".to_string(),
            ));
        }

        // Port must be non-zero (port 0 would cause bind failure)
        if self.server.port == 0 {
            return Err(ConfigError::Invalid(
                "server.port must be greater than 0".to_string(),
            ));
        }
        if self.otel.grpc_enabled && self.otel.grpc_port == 0 {
            return Err(ConfigError::Invalid(
                "otel.grpc.port must be greater than 0".to_string(),
            ));
        }
        if self.otel.staging_redrive_cap == 0 {
            return Err(ConfigError::Invalid(
                "otel.staging_redrive_cap must be greater than 0".to_string(),
            ));
        }

        // Port collision check (only if both are enabled)
        if self.otel.grpc_enabled && self.server.port == self.otel.grpc_port {
            return Err(ConfigError::Invalid(format!(
                "server.port ({}) and otel.grpc.port ({}) cannot be the same",
                self.server.port, self.otel.grpc_port
            )));
        }

        // S3 bucket required when using S3 storage
        if self.files.storage == StorageBackend::S3 && self.files.s3.is_none() {
            return Err(ConfigError::Invalid(
                "files.s3.bucket is required (and non-empty) when files.storage is 's3'"
                    .to_string(),
            ));
        }

        // Redis URL required when using Redis cache backend
        if self.database.cache == CacheBackendType::Redis
            && self
                .database
                .redis
                .as_ref()
                .is_none_or(|r| r.url.is_empty())
        {
            return Err(ConfigError::Invalid(
                "database.redis.url is required when database.cache is 'redis'".to_string(),
            ));
        }
        if self.database.queue == QueueBackendType::Redis
            && self
                .database
                .redis
                .as_ref()
                .is_none_or(|r| r.url.is_empty())
        {
            return Err(ConfigError::Invalid(
                "database.redis.url is required when database.queue is 'redis'".to_string(),
            ));
        }
        if self.database.queue == QueueBackendType::Redpanda {
            let Some(redpanda) = self.database.redpanda.as_ref() else {
                return Err(ConfigError::Invalid(
                    "RedPanda configuration missing when database.queue is 'redpanda'".to_string(),
                ));
            };
            if redpanda.brokers.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "database.redpanda.brokers is required when database.queue is 'redpanda'"
                        .to_string(),
                ));
            }
            if redpanda.partitions <= 0 || redpanda.replication_factor <= 0 {
                return Err(ConfigError::Invalid(
                    "RedPanda partitions and replication_factor must be greater than 0".to_string(),
                ));
            }
            if redpanda.retention_warning_ms >= redpanda.retention_ms {
                return Err(ConfigError::Invalid(
                    "RedPanda retention_warning_ms must be less than retention_ms".to_string(),
                ));
            }
            if redpanda.replication_factor == 1 {
                tracing::warn!(
                    "Redpanda queue topics have one replica; recoverable production requires tested Tiered Storage or a replication factor greater than one"
                );
            }
        }

        // Warn about rate limiting enabled with 0 RPM
        if self.rate_limit.enabled && self.rate_limit.api_rpm == 0 {
            tracing::warn!("rate_limit.api_rpm is 0, all API requests will be blocked");
        }

        // Warn about potentially dangerous retention settings
        if let Some(max_age) = self.otel.retention.max_age_minutes {
            if max_age == 0 {
                tracing::warn!(
                    "otel.retention.max_age_minutes is 0, which will delete all trace data immediately"
                );
            } else if max_age < 5 {
                tracing::warn!(
                    max_age_minutes = max_age,
                    "otel.retention.max_age_minutes is very low, data may be deleted quickly"
                );
            }
        }

        // `files.quota_bytes` is the unified project budget even when blob persistence is disabled.
        if self.files.quota_bytes < 32 * 1024 * 1024 {
            tracing::warn!(
                quota_bytes = self.files.quota_bytes,
                "files.quota_bytes is below the maintenance reserve; ordinary telemetry writes may be refused"
            );
        }

        // Security warning: auth disabled while binding to all interfaces
        if !self.auth.enabled && is_all_interfaces(&self.server.host) {
            tracing::warn!(
                host = %self.server.host,
                "Authentication is disabled while binding to all network interfaces. \
                 This exposes an unauthenticated server to your network."
            );
        }

        // PostgreSQL URL required when using Postgres backend
        if self.database.transactional == TransactionalBackend::Postgres {
            if let Some(ref pg) = self.database.postgres {
                if pg.url.is_empty() {
                    return Err(ConfigError::Invalid("database.postgres.url is required when database.transactional is 'postgres'. \
                         Set via SIDESEAT_POSTGRES_URL env var or database.postgres.url in config file.".to_string()));
                }
            } else {
                return Err(ConfigError::Invalid(
                    "PostgreSQL configuration missing when database.transactional is 'postgres'"
                        .to_string(),
                ));
            }
        }

        // A store holding bytes must be at least as reachable as the rows naming them. The pepper is
        // needed whenever *any* auth path is enabled, not just the console one: `otel.auth.required`
        // ingests through the same API-key hash, so a browser session and an ingestion key both fail on
        // whichever replica the balancer picked.
        validate_store_sharing(
            self.database.transactional,
            self.database.analytics,
            self.files.storage,
            self.secrets.backend,
            self.auth.enabled || self.otel.auth_required,
        )?;

        // ClickHouse URL required when using ClickHouse backend
        if self.database.analytics == AnalyticsBackend::Clickhouse {
            if let Some(ref ch) = self.database.clickhouse {
                validate_clickhouse(ch)?;
            } else {
                return Err(ConfigError::Invalid(
                    "ClickHouse configuration missing when database.analytics is 'clickhouse'"
                        .to_string(),
                ));
            }
        }

        // AWS recovery_window_days must be 7-30 if set
        if let Some(ref aws) = self.secrets.aws
            && let Some(d) = aws.recovery_window_days
            && !(7..=30).contains(&d)
        {
            return Err(ConfigError::Invalid(format!(
                "secrets.aws.recovery_window_days must be between 7 and 30 (got {})",
                d
            )));
        }

        // Vault address and token required when using Vault secrets backend
        if self.secrets.backend == SecretsBackend::Vault {
            if let Some(ref v) = self.secrets.vault {
                if v.address.is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "secrets.vault.address is required when secrets.backend is 'vault'. \
                         Set via {} env var or secrets.vault.address in config file.",
                        ENV_SECRETS_VAULT_ADDR
                    )));
                }
                if !v.address.starts_with("http://") && !v.address.starts_with("https://") {
                    return Err(ConfigError::Invalid(format!(
                        "secrets.vault.address must start with http:// or https://. Got: {}",
                        v.address
                    )));
                }
                if v.token.is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "Vault token required when secrets.backend is 'vault'. \
                         Set via VAULT_TOKEN, {} env var, or secrets.vault.token in config file.",
                        ENV_SECRETS_VAULT_TOKEN
                    )));
                }
            } else {
                return Err(ConfigError::Invalid(
                    "Vault configuration missing when secrets.backend is 'vault'".to_string(),
                ));
            }
        }

        Ok(())
    }
}
