//! Command-line parsing and configuration overrides.

use clap::{Parser, Subcommand};

use std::path::PathBuf;

use super::config::{
    AnalyticsBackend, CacheBackendType, EvictionPolicy, QueueBackendType, SecretsBackend,
    StorageBackend, TransactionalBackend,
};
use super::constants::{
    ENV_ANALYTICS_BACKEND, ENV_CACHE_BACKEND, ENV_CACHE_EVICTION_POLICY, ENV_CACHE_MAX_ENTRIES,
    ENV_CACHE_REDIS_URL, ENV_CLICKHOUSE_URL, ENV_CONFIG, ENV_CREDENTIALS_SCAN_ENV, ENV_DEBUG,
    ENV_FILES_ENABLED, ENV_FILES_QUOTA_BYTES, ENV_FILES_S3_BUCKET, ENV_FILES_S3_ENDPOINT,
    ENV_FILES_S3_PREFIX, ENV_FILES_S3_REGION, ENV_FILES_STORAGE, ENV_HOST, ENV_MCP_ENABLED,
    ENV_NO_UPDATE_CHECK, ENV_OTEL_AUTH_REQUIRED, ENV_OTEL_GRPC_ENABLED, ENV_OTEL_GRPC_PORT,
    ENV_OTEL_RETENTION_MAX_AGE_MINUTES, ENV_OTEL_RETENTION_MAX_SPANS, ENV_PORT, ENV_POSTGRES_URL,
    ENV_PRICING_SYNC_HOURS, ENV_QUEUE_BACKEND, ENV_RATE_LIMIT_API_RPM, ENV_RATE_LIMIT_AUTH_RPM,
    ENV_RATE_LIMIT_BYPASS_HEADER, ENV_RATE_LIMIT_ENABLED, ENV_RATE_LIMIT_FILES_RPM,
    ENV_RATE_LIMIT_INGESTION_RPM, ENV_RATE_LIMIT_PER_IP, ENV_REDPANDA_BROKERS, ENV_SECRETS_BACKEND,
    ENV_TRANSACTIONAL_BACKEND,
};

#[derive(Parser)]
#[command(name = "sideseat")]
#[command(version, about = "AI Development Workbench", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Server host address
    #[arg(long, short = 'H', global = true, env = ENV_HOST)]
    host: Option<String>,

    /// Server port
    #[arg(long, short = 'p', global = true, env = ENV_PORT)]
    port: Option<u16>,

    /// Disable authentication (for development)
    #[arg(long, global = true)]
    no_auth: bool,

    /// Enable debug mode (writes incoming OTLP data to debug folder)
    #[arg(long, global = true, env = ENV_DEBUG)]
    debug: bool,

    /// Path to config file
    #[arg(long, short = 'c', global = true, env = ENV_CONFIG)]
    config: Option<PathBuf>,

    /// Enable OTEL gRPC endpoint
    #[arg(long, global = true, env = ENV_OTEL_GRPC_ENABLED)]
    otel_grpc: Option<bool>,

    /// OTEL gRPC port
    #[arg(long, global = true, env = ENV_OTEL_GRPC_PORT)]
    otel_grpc_port: Option<u16>,

    /// OTEL retention max age in minutes (data older than this is deleted)
    #[arg(long, global = true, env = ENV_OTEL_RETENTION_MAX_AGE_MINUTES)]
    otel_retention_max_age: Option<u64>,

    /// OTEL retention max spans limit
    #[arg(long, global = true, env = ENV_OTEL_RETENTION_MAX_SPANS)]
    otel_retention_max_spans: Option<u64>,

    /// Require API key for OTEL ingestion
    #[arg(long, global = true, env = ENV_OTEL_AUTH_REQUIRED)]
    otel_auth_required: Option<bool>,

    /// Pricing sync interval in hours (0 = disabled)
    #[arg(long, global = true, env = ENV_PRICING_SYNC_HOURS)]
    pricing_sync_hours: Option<u64>,

    /// Disable update check on startup
    #[arg(long, global = true, env = ENV_NO_UPDATE_CHECK)]
    no_update_check: bool,

    /// Enable or disable file storage
    #[arg(long, global = true, env = ENV_FILES_ENABLED)]
    files_enabled: Option<bool>,

    /// Enable or disable MCP server
    #[arg(long, global = true, env = ENV_MCP_ENABLED)]
    mcp: Option<bool>,

    /// File storage backend (filesystem or s3)
    #[arg(long, global = true, env = ENV_FILES_STORAGE, value_parser = parse_storage_backend)]
    files_storage: Option<StorageBackend>,

    /// Unified telemetry, staging, journal and blob storage quota in bytes per project
    #[arg(long, global = true, env = ENV_FILES_QUOTA_BYTES)]
    files_quota_bytes: Option<u64>,

    /// S3 bucket name for file storage
    #[arg(long, global = true, env = ENV_FILES_S3_BUCKET)]
    files_s3_bucket: Option<String>,

    /// S3 key prefix for file storage
    #[arg(long, global = true, env = ENV_FILES_S3_PREFIX)]
    files_s3_prefix: Option<String>,

    /// S3 region for file storage
    #[arg(long, global = true, env = ENV_FILES_S3_REGION)]
    files_s3_region: Option<String>,

    /// S3 endpoint URL for S3-compatible services (e.g. MinIO)
    #[arg(long, global = true, env = ENV_FILES_S3_ENDPOINT)]
    files_s3_endpoint: Option<String>,

    // Cache options
    /// Cache backend (memory or redis)
    #[arg(long, global = true, env = ENV_CACHE_BACKEND, value_parser = parse_cache_backend_type)]
    cache_backend: Option<CacheBackendType>,

    /// Maximum number of cache entries
    #[arg(long, global = true, env = ENV_CACHE_MAX_ENTRIES)]
    cache_max_entries: Option<u64>,

    /// Cache eviction policy (tinylfu or lru)
    #[arg(long, global = true, env = ENV_CACHE_EVICTION_POLICY, value_parser = parse_eviction_policy)]
    cache_eviction_policy: Option<EvictionPolicy>,

    /// Redis-compatible cache URL. Supports Redis, Sentinel, Valkey, Dragonfly.
    /// Formats: redis://host:port/db, redis+sentinel://s1:port,s2:port/master/db
    #[arg(long, global = true, env = ENV_CACHE_REDIS_URL)]
    cache_redis_url: Option<String>,

    /// Durable queue backend (memory, redis, or redpanda).
    #[arg(long, global = true, env = ENV_QUEUE_BACKEND, value_parser = parse_queue_backend_type)]
    queue_backend: Option<QueueBackendType>,

    /// RedPanda/Kafka bootstrap broker list.
    #[arg(long, global = true, env = ENV_REDPANDA_BROKERS)]
    redpanda_brokers: Option<String>,

    // Rate limit options
    /// Enable or disable rate limiting
    #[arg(long, global = true, env = ENV_RATE_LIMIT_ENABLED)]
    rate_limit_enabled: Option<bool>,

    /// Enable per-IP rate limiting (API, auth endpoints). Disabled by default.
    #[arg(long, global = true, env = ENV_RATE_LIMIT_PER_IP)]
    rate_limit_per_ip: Option<bool>,

    /// API rate limit (requests per minute)
    #[arg(long, global = true, env = ENV_RATE_LIMIT_API_RPM)]
    rate_limit_api_rpm: Option<u32>,

    /// Ingestion rate limit (requests per minute)
    #[arg(long, global = true, env = ENV_RATE_LIMIT_INGESTION_RPM)]
    rate_limit_ingestion_rpm: Option<u32>,

    /// Auth rate limit (requests per minute)
    #[arg(long, global = true, env = ENV_RATE_LIMIT_AUTH_RPM)]
    rate_limit_auth_rpm: Option<u32>,

    /// Files rate limit (requests per minute)
    #[arg(long, global = true, env = ENV_RATE_LIMIT_FILES_RPM)]
    rate_limit_files_rpm: Option<u32>,

    /// Rate limit bypass header secret
    #[arg(long, global = true, env = ENV_RATE_LIMIT_BYPASS_HEADER)]
    rate_limit_bypass_header: Option<String>,

    /// Secrets backend
    #[arg(long, global = true, env = ENV_SECRETS_BACKEND, value_parser = parse_secrets_backend)]
    secrets_backend: Option<SecretsBackend>,

    // Database options
    /// Transactional database backend (sqlite or postgres)
    #[arg(long, global = true, env = ENV_TRANSACTIONAL_BACKEND, value_parser = parse_transactional_backend)]
    transactional_backend: Option<TransactionalBackend>,

    /// Analytics database backend (duckdb or clickhouse)
    #[arg(long, global = true, env = ENV_ANALYTICS_BACKEND, value_parser = parse_analytics_backend)]
    analytics_backend: Option<AnalyticsBackend>,

    /// PostgreSQL connection URL (when using postgres backend)
    #[arg(long, global = true, env = ENV_POSTGRES_URL)]
    postgres_url: Option<String>,

    /// ClickHouse connection URL (when using clickhouse backend)
    #[arg(long, global = true, env = ENV_CLICKHOUSE_URL)]
    clickhouse_url: Option<String>,

    /// Scan environment variables for provider API keys (default: true)
    #[arg(long, global = true, env = ENV_CREDENTIALS_SCAN_ENV)]
    credentials_scan_env: Option<bool>,
}

/// Parse storage backend from CLI/env string
fn parse_storage_backend(s: &str) -> Result<StorageBackend, String> {
    match s.to_lowercase().as_str() {
        "filesystem" => Ok(StorageBackend::Filesystem),
        "s3" => Ok(StorageBackend::S3),
        _ => Err(format!(
            "Invalid storage backend '{}'. Valid options: filesystem, s3",
            s
        )),
    }
}

/// Parse cache backend type from CLI/env string
fn parse_cache_backend_type(s: &str) -> Result<CacheBackendType, String> {
    match s.to_lowercase().as_str() {
        "memory" => Ok(CacheBackendType::Memory),
        "redis" => Ok(CacheBackendType::Redis),
        _ => Err(format!(
            "Invalid cache backend '{}'. Valid options: memory, redis",
            s
        )),
    }
}

fn parse_queue_backend_type(s: &str) -> Result<QueueBackendType, String> {
    match s.to_lowercase().as_str() {
        "memory" => Ok(QueueBackendType::Memory),
        "redis" => Ok(QueueBackendType::Redis),
        "redpanda" | "kafka" => Ok(QueueBackendType::Redpanda),
        _ => Err(format!(
            "Invalid queue backend '{s}'. Valid options: memory, redis, redpanda"
        )),
    }
}

/// Parse eviction policy from CLI/env string
fn parse_eviction_policy(s: &str) -> Result<EvictionPolicy, String> {
    match s.to_lowercase().as_str() {
        "tinylfu" => Ok(EvictionPolicy::TinyLfu),
        "lru" => Ok(EvictionPolicy::Lru),
        _ => Err(format!(
            "Invalid eviction policy '{}'. Valid options: tinylfu, lru",
            s
        )),
    }
}

/// Parse transactional backend from CLI/env string
fn parse_transactional_backend(s: &str) -> Result<TransactionalBackend, String> {
    match s.to_lowercase().as_str() {
        "sqlite" => Ok(TransactionalBackend::Sqlite),
        "postgres" | "postgresql" => Ok(TransactionalBackend::Postgres),
        _ => Err(format!(
            "Invalid transactional backend '{}'. Valid options: sqlite, postgres",
            s
        )),
    }
}

/// Parse analytics backend from CLI/env string
fn parse_analytics_backend(s: &str) -> Result<AnalyticsBackend, String> {
    match s.to_lowercase().as_str() {
        "duckdb" => Ok(AnalyticsBackend::Duckdb),
        "clickhouse" => Ok(AnalyticsBackend::Clickhouse),
        _ => Err(format!(
            "Invalid analytics backend '{}'. Valid options: duckdb, clickhouse",
            s
        )),
    }
}

/// Parse secrets backend from CLI/env string
fn parse_secrets_backend(s: &str) -> Result<SecretsBackend, String> {
    match s.to_lowercase().as_str() {
        "keychain" => Ok(SecretsBackend::Keychain),
        "credential-manager" => Ok(SecretsBackend::CredentialManager),
        "secret-service" => Ok(SecretsBackend::SecretService),
        "keyutils" => Ok(SecretsBackend::Keyutils),
        "file" => Ok(SecretsBackend::File),
        "env" => Ok(SecretsBackend::Env),
        "aws" => Ok(SecretsBackend::Aws),
        "vault" | "hashicorp" => Ok(SecretsBackend::Vault),
        _ => Err(format!(
            "Invalid secrets backend '{}'. Valid: keychain, \
             credential-manager, secret-service, keyutils, file, env, aws, vault",
            s
        )),
    }
}

#[derive(Subcommand, Clone, Debug)]
pub enum Commands {
    /// Start the server (default command)
    Start,
    /// System maintenance commands
    System {
        #[command(subcommand)]
        command: SystemCommands,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum SystemCommands {
    /// Delete local data directory (databases, files, caches). Requires confirmation.
    Prune {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
    /// Replay durable deletions and reconcile independently restored stores before serving.
    RestoreRepair {
        /// Write the JSON repair report to this file instead of stdout.
        #[arg(long)]
        report: Option<PathBuf>,
    },
}

/// Configuration derived from CLI arguments
#[derive(Debug, Clone, Default)]
pub struct CliConfig {
    pub(crate) host: Option<String>,
    pub(crate) port: Option<u16>,
    pub(crate) no_auth: bool,
    pub(crate) debug: bool,
    pub(crate) config: Option<PathBuf>,
    pub(crate) otel_grpc: Option<bool>,
    pub(crate) otel_grpc_port: Option<u16>,
    pub(crate) otel_retention_max_age: Option<u64>,
    pub(crate) otel_retention_max_spans: Option<u64>,
    pub(crate) otel_auth_required: Option<bool>,
    pub(crate) pricing_sync_hours: Option<u64>,
    pub(crate) no_update_check: bool,
    pub(crate) files_enabled: Option<bool>,
    pub(crate) mcp: Option<bool>,
    pub(crate) files_storage: Option<StorageBackend>,
    pub(crate) files_quota_bytes: Option<u64>,
    pub(crate) files_s3_bucket: Option<String>,
    pub(crate) files_s3_prefix: Option<String>,
    pub(crate) files_s3_region: Option<String>,
    pub(crate) files_s3_endpoint: Option<String>,
    pub(crate) cache_backend: Option<CacheBackendType>,
    pub(crate) cache_max_entries: Option<u64>,
    pub(crate) cache_eviction_policy: Option<EvictionPolicy>,
    pub(crate) cache_redis_url: Option<String>,
    pub(crate) queue_backend: Option<QueueBackendType>,
    pub(crate) redpanda_brokers: Option<String>,
    pub(crate) rate_limit_enabled: Option<bool>,
    pub(crate) rate_limit_per_ip: Option<bool>,
    pub(crate) rate_limit_api_rpm: Option<u32>,
    pub(crate) rate_limit_ingestion_rpm: Option<u32>,
    pub(crate) rate_limit_auth_rpm: Option<u32>,
    pub(crate) rate_limit_files_rpm: Option<u32>,
    pub(crate) rate_limit_bypass_header: Option<String>,
    pub(crate) secrets_backend: Option<SecretsBackend>,
    pub(crate) transactional_backend: Option<TransactionalBackend>,
    pub(crate) analytics_backend: Option<AnalyticsBackend>,
    pub(crate) postgres_url: Option<String>,
    pub(crate) clickhouse_url: Option<String>,
    pub(crate) credentials_scan_env: Option<bool>,
}

/// Parse CLI arguments and return config with command
pub fn parse() -> (CliConfig, Option<Commands>) {
    let cli = Cli::parse();
    let config = CliConfig {
        host: cli.host,
        port: cli.port,
        no_auth: cli.no_auth,
        debug: cli.debug,
        config: cli.config,
        otel_grpc: cli.otel_grpc,
        otel_grpc_port: cli.otel_grpc_port,
        otel_retention_max_age: cli.otel_retention_max_age,
        otel_retention_max_spans: cli.otel_retention_max_spans,
        otel_auth_required: cli.otel_auth_required,
        pricing_sync_hours: cli.pricing_sync_hours,
        no_update_check: cli.no_update_check,
        files_enabled: cli.files_enabled,
        mcp: cli.mcp,
        files_storage: cli.files_storage,
        files_quota_bytes: cli.files_quota_bytes,
        files_s3_bucket: cli.files_s3_bucket,
        files_s3_prefix: cli.files_s3_prefix,
        files_s3_region: cli.files_s3_region,
        files_s3_endpoint: cli.files_s3_endpoint,
        cache_backend: cli.cache_backend,
        cache_max_entries: cli.cache_max_entries,
        cache_eviction_policy: cli.cache_eviction_policy,
        cache_redis_url: cli.cache_redis_url,
        queue_backend: cli.queue_backend,
        redpanda_brokers: cli.redpanda_brokers,
        rate_limit_enabled: cli.rate_limit_enabled,
        rate_limit_per_ip: cli.rate_limit_per_ip,
        rate_limit_api_rpm: cli.rate_limit_api_rpm,
        rate_limit_ingestion_rpm: cli.rate_limit_ingestion_rpm,
        rate_limit_auth_rpm: cli.rate_limit_auth_rpm,
        rate_limit_files_rpm: cli.rate_limit_files_rpm,
        rate_limit_bypass_header: cli.rate_limit_bypass_header,
        secrets_backend: cli.secrets_backend,
        transactional_backend: cli.transactional_backend,
        analytics_backend: cli.analytics_backend,
        postgres_url: cli.postgres_url,
        clickhouse_url: cli.clickhouse_url,
        credentials_scan_env: cli.credentials_scan_env,
    };
    (config, cli.command)
}
