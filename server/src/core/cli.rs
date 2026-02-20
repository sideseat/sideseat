use clap::{Parser, Subcommand};

use std::path::PathBuf;

use super::config::{
    AnalyticsBackend, CacheBackendType, EvictionPolicy, SecretsBackend, StorageBackend,
    TransactionalBackend,
};
use super::constants::{
    ENV_ANALYTICS_BACKEND, ENV_CACHE_BACKEND, ENV_CACHE_EVICTION_POLICY, ENV_CACHE_MAX_ENTRIES,
    ENV_CACHE_REDIS_URL, ENV_CLICKHOUSE_URL, ENV_CONFIG, ENV_DEBUG, ENV_FILES_ENABLED,
    ENV_FILES_QUOTA_BYTES, ENV_FILES_STORAGE, ENV_HOST, ENV_MCP_ENABLED, ENV_NO_UPDATE_CHECK,
    ENV_OTEL_AUTH_REQUIRED, ENV_OTEL_GRPC_ENABLED, ENV_OTEL_GRPC_PORT,
    ENV_OTEL_RETENTION_MAX_AGE_MINUTES, ENV_OTEL_RETENTION_MAX_SPANS, ENV_PORT, ENV_POSTGRES_URL,
    ENV_PRICING_SYNC_HOURS, ENV_RATE_LIMIT_API_RPM, ENV_RATE_LIMIT_AUTH_RPM,
    ENV_RATE_LIMIT_BYPASS_HEADER, ENV_RATE_LIMIT_ENABLED, ENV_RATE_LIMIT_FILES_RPM,
    ENV_RATE_LIMIT_INGESTION_RPM, ENV_RATE_LIMIT_PER_IP, ENV_SECRETS_BACKEND,
    ENV_TRANSACTIONAL_BACKEND,
};

#[derive(Parser)]
#[command(name = "sideseat")]
#[command(version, about = "AI Development Workbench", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Server host address
    #[arg(long, short = 'H', global = true, env = ENV_HOST)]
    pub host: Option<String>,

    /// Server port
    #[arg(long, short = 'p', global = true, env = ENV_PORT)]
    pub port: Option<u16>,

    /// Disable authentication (for development)
    #[arg(long, global = true)]
    pub no_auth: bool,

    /// Enable debug mode (writes incoming OTLP data to debug folder)
    #[arg(long, global = true, env = ENV_DEBUG)]
    pub debug: bool,

    /// Path to config file
    #[arg(long, short = 'c', global = true, env = ENV_CONFIG)]
    pub config: Option<PathBuf>,

    /// Enable OTEL gRPC endpoint
    #[arg(long, global = true, env = ENV_OTEL_GRPC_ENABLED)]
    pub otel_grpc: Option<bool>,

    /// OTEL gRPC port
    #[arg(long, global = true, env = ENV_OTEL_GRPC_PORT)]
    pub otel_grpc_port: Option<u16>,

    /// OTEL retention max age in minutes (data older than this is deleted)
    #[arg(long, global = true, env = ENV_OTEL_RETENTION_MAX_AGE_MINUTES)]
    pub otel_retention_max_age: Option<u64>,

    /// OTEL retention max spans limit
    #[arg(long, global = true, env = ENV_OTEL_RETENTION_MAX_SPANS)]
    pub otel_retention_max_spans: Option<u64>,

    /// Require API key for OTEL ingestion
    #[arg(long, global = true, env = ENV_OTEL_AUTH_REQUIRED)]
    pub otel_auth_required: Option<bool>,

    /// Pricing sync interval in hours (0 = disabled)
    #[arg(long, global = true, env = ENV_PRICING_SYNC_HOURS)]
    pub pricing_sync_hours: Option<u64>,

    /// Disable update check on startup
    #[arg(long, global = true, env = ENV_NO_UPDATE_CHECK)]
    pub no_update_check: bool,

    /// Enable or disable file storage
    #[arg(long, global = true, env = ENV_FILES_ENABLED)]
    pub files_enabled: Option<bool>,

    /// Enable or disable MCP server
    #[arg(long, global = true, env = ENV_MCP_ENABLED)]
    pub mcp: Option<bool>,

    /// File storage backend (filesystem or s3)
    #[arg(long, global = true, env = ENV_FILES_STORAGE, value_parser = parse_storage_backend)]
    pub files_storage: Option<StorageBackend>,

    /// File storage quota in bytes per project
    #[arg(long, global = true, env = ENV_FILES_QUOTA_BYTES)]
    pub files_quota_bytes: Option<u64>,

    // Cache options
    /// Cache backend (memory or redis)
    #[arg(long, global = true, env = ENV_CACHE_BACKEND, value_parser = parse_cache_backend_type)]
    pub cache_backend: Option<CacheBackendType>,

    /// Maximum number of cache entries
    #[arg(long, global = true, env = ENV_CACHE_MAX_ENTRIES)]
    pub cache_max_entries: Option<u64>,

    /// Cache eviction policy (tinylfu or lru)
    #[arg(long, global = true, env = ENV_CACHE_EVICTION_POLICY, value_parser = parse_eviction_policy)]
    pub cache_eviction_policy: Option<EvictionPolicy>,

    /// Redis-compatible cache URL. Supports Redis, Sentinel, Valkey, Dragonfly.
    /// Formats: redis://host:port/db, redis+sentinel://s1:port,s2:port/master/db
    #[arg(long, global = true, env = ENV_CACHE_REDIS_URL)]
    pub cache_redis_url: Option<String>,

    // Rate limit options
    /// Enable or disable rate limiting
    #[arg(long, global = true, env = ENV_RATE_LIMIT_ENABLED)]
    pub rate_limit_enabled: Option<bool>,

    /// Enable per-IP rate limiting (API, auth endpoints). Disabled by default.
    #[arg(long, global = true, env = ENV_RATE_LIMIT_PER_IP)]
    pub rate_limit_per_ip: Option<bool>,

    /// API rate limit (requests per minute)
    #[arg(long, global = true, env = ENV_RATE_LIMIT_API_RPM)]
    pub rate_limit_api_rpm: Option<u32>,

    /// Ingestion rate limit (requests per minute)
    #[arg(long, global = true, env = ENV_RATE_LIMIT_INGESTION_RPM)]
    pub rate_limit_ingestion_rpm: Option<u32>,

    /// Auth rate limit (requests per minute)
    #[arg(long, global = true, env = ENV_RATE_LIMIT_AUTH_RPM)]
    pub rate_limit_auth_rpm: Option<u32>,

    /// Files rate limit (requests per minute)
    #[arg(long, global = true, env = ENV_RATE_LIMIT_FILES_RPM)]
    pub rate_limit_files_rpm: Option<u32>,

    /// Rate limit bypass header secret
    #[arg(long, global = true, env = ENV_RATE_LIMIT_BYPASS_HEADER)]
    pub rate_limit_bypass_header: Option<String>,

    /// Secrets backend
    #[arg(long, global = true, env = ENV_SECRETS_BACKEND, value_parser = parse_secrets_backend)]
    pub secrets_backend: Option<SecretsBackend>,

    // Database options
    /// Transactional database backend (sqlite or postgres)
    #[arg(long, global = true, env = ENV_TRANSACTIONAL_BACKEND, value_parser = parse_transactional_backend)]
    pub transactional_backend: Option<TransactionalBackend>,

    /// Analytics database backend (duckdb or clickhouse)
    #[arg(long, global = true, env = ENV_ANALYTICS_BACKEND, value_parser = parse_analytics_backend)]
    pub analytics_backend: Option<AnalyticsBackend>,

    /// PostgreSQL connection URL (when using postgres backend)
    #[arg(long, global = true, env = ENV_POSTGRES_URL)]
    pub postgres_url: Option<String>,

    /// ClickHouse connection URL (when using clickhouse backend)
    #[arg(long, global = true, env = ENV_CLICKHOUSE_URL)]
    pub clickhouse_url: Option<String>,
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
}

/// Configuration derived from CLI arguments
#[derive(Debug, Clone, Default)]
pub struct CliConfig {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub no_auth: bool,
    pub debug: bool,
    pub config: Option<PathBuf>,
    pub otel_grpc: Option<bool>,
    pub otel_grpc_port: Option<u16>,
    pub otel_retention_max_age: Option<u64>,
    pub otel_retention_max_spans: Option<u64>,
    pub otel_auth_required: Option<bool>,
    pub pricing_sync_hours: Option<u64>,
    pub no_update_check: bool,
    pub files_enabled: Option<bool>,
    pub mcp: Option<bool>,
    pub files_storage: Option<StorageBackend>,
    pub files_quota_bytes: Option<u64>,
    pub cache_backend: Option<CacheBackendType>,
    pub cache_max_entries: Option<u64>,
    pub cache_eviction_policy: Option<EvictionPolicy>,
    pub cache_redis_url: Option<String>,
    pub rate_limit_enabled: Option<bool>,
    pub rate_limit_per_ip: Option<bool>,
    pub rate_limit_api_rpm: Option<u32>,
    pub rate_limit_ingestion_rpm: Option<u32>,
    pub rate_limit_auth_rpm: Option<u32>,
    pub rate_limit_files_rpm: Option<u32>,
    pub rate_limit_bypass_header: Option<String>,
    pub secrets_backend: Option<SecretsBackend>,
    pub transactional_backend: Option<TransactionalBackend>,
    pub analytics_backend: Option<AnalyticsBackend>,
    pub postgres_url: Option<String>,
    pub clickhouse_url: Option<String>,
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
        cache_backend: cli.cache_backend,
        cache_max_entries: cli.cache_max_entries,
        cache_eviction_policy: cli.cache_eviction_policy,
        cache_redis_url: cli.cache_redis_url,
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
    };
    (config, cli.command)
}
