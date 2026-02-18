use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::utils::file::expand_path;

use super::cli::CliConfig;
use super::constants::{
    APP_DOT_FOLDER, CONFIG_FILE_NAME, DEFAULT_CACHE_MAX_ENTRIES, DEFAULT_HOST,
    DEFAULT_OTEL_GRPC_PORT, DEFAULT_OTEL_RETENTION_MAX_SPANS, DEFAULT_PORT,
    DEFAULT_RATE_LIMIT_API_RPM, DEFAULT_RATE_LIMIT_AUTH_RPM, DEFAULT_RATE_LIMIT_FILES_RPM,
    DEFAULT_RATE_LIMIT_INGESTION_RPM, ENV_SECRETS_AWS_PREFIX, ENV_SECRETS_AWS_REGION,
    ENV_SECRETS_ENV_PREFIX, ENV_SECRETS_VAULT_ADDR, ENV_SECRETS_VAULT_MOUNT,
    ENV_SECRETS_VAULT_PREFIX, ENV_SECRETS_VAULT_TOKEN, FILES_DEFAULT_QUOTA_BYTES,
    FILES_DEFAULT_S3_PREFIX, POSTGRES_DEFAULT_ACQUIRE_TIMEOUT_SECS,
    POSTGRES_DEFAULT_IDLE_TIMEOUT_SECS, POSTGRES_DEFAULT_MAX_CONNECTIONS,
    POSTGRES_DEFAULT_MAX_LIFETIME_SECS, POSTGRES_DEFAULT_MIN_CONNECTIONS,
    POSTGRES_DEFAULT_STATEMENT_TIMEOUT_SECS, PRICING_SYNC_INTERVAL_SECS,
    SECRETS_DEFAULT_AWS_PREFIX, SECRETS_DEFAULT_ENV_PREFIX, SECRETS_DEFAULT_VAULT_MOUNT,
    SECRETS_DEFAULT_VAULT_PREFIX,
};

// =============================================================================
// Storage Backend Enum
// =============================================================================

/// Storage backend type for file storage
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    #[default]
    Filesystem,
    S3,
}

impl fmt::Display for StorageBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageBackend::Filesystem => write!(f, "filesystem"),
            StorageBackend::S3 => write!(f, "s3"),
        }
    }
}

// =============================================================================
// Transactional Backend Enum (SQLite or PostgreSQL)
// =============================================================================

/// Transactional database backend for metadata storage
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionalBackend {
    #[default]
    Sqlite,
    Postgres,
}

impl fmt::Display for TransactionalBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransactionalBackend::Sqlite => write!(f, "sqlite"),
            TransactionalBackend::Postgres => write!(f, "postgres"),
        }
    }
}

// =============================================================================
// Analytics Backend Enum (DuckDB or ClickHouse)
// =============================================================================

/// Analytics database backend for OTEL data (high-throughput writes)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnalyticsBackend {
    #[default]
    Duckdb,
    Clickhouse,
}

impl fmt::Display for AnalyticsBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnalyticsBackend::Duckdb => write!(f, "duckdb"),
            AnalyticsBackend::Clickhouse => write!(f, "clickhouse"),
        }
    }
}

// =============================================================================
// Cache Backend Enum
// =============================================================================

/// Cache backend type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheBackendType {
    #[default]
    Memory,
    Redis,
}

impl fmt::Display for CacheBackendType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CacheBackendType::Memory => write!(f, "memory"),
            CacheBackendType::Redis => write!(f, "redis"),
        }
    }
}

// =============================================================================
// Eviction Policy Enum
// =============================================================================

/// Cache eviction policy
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EvictionPolicy {
    /// TinyLFU - LRU eviction + LFU admission (near-optimal hit ratio)
    #[default]
    TinyLfu,
    /// Simple LRU (better for recency-biased workloads)
    Lru,
}

impl fmt::Display for EvictionPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvictionPolicy::TinyLfu => write!(f, "tinylfu"),
            EvictionPolicy::Lru => write!(f, "lru"),
        }
    }
}

// =============================================================================
// Secrets Backend Enum
// =============================================================================

/// Secrets storage backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretsBackend {
    DataProtectionKeychain,
    Keychain,
    CredentialManager,
    SecretService,
    Keyutils,
    File,
    Env,
    Aws,
    Vault,
}

impl SecretsBackend {
    /// Auto-detect best available backend for the current platform.
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::DataProtectionKeychain
        }
        #[cfg(target_os = "windows")]
        {
            Self::CredentialManager
        }
        #[cfg(target_os = "linux")]
        {
            Self::SecretService
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Self::File
        }
    }

    /// Whether this backend uses vault-blob storage (keychain/file variants)
    pub fn is_vault_based(&self) -> bool {
        matches!(
            self,
            Self::DataProtectionKeychain
                | Self::Keychain
                | Self::CredentialManager
                | Self::SecretService
                | Self::Keyutils
                | Self::File
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DataProtectionKeychain => "data-protection-keychain",
            Self::Keychain => "keychain",
            Self::CredentialManager => "credential-manager",
            Self::SecretService => "secret-service",
            Self::Keyutils => "keyutils",
            Self::File => "file",
            Self::Env => "env",
            Self::Aws => "aws",
            Self::Vault => "vault",
        }
    }
}

impl fmt::Display for SecretsBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// File Config Structs (JSON deserialization)
// =============================================================================

/// Server configuration section
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ServerFileConfig {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub mcp: Option<McpFileConfig>,
}

/// Authentication configuration section
#[derive(Debug, Default, Clone, Deserialize)]
pub struct AuthFileConfig {
    pub enabled: Option<bool>,
}

/// gRPC configuration (nested under otel)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct GrpcFileConfig {
    pub enabled: Option<bool>,
    pub port: Option<u16>,
}

/// Retention configuration (nested under otel)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct RetentionFileConfig {
    pub max_age_minutes: Option<u64>,
    pub max_spans: Option<u64>,
}

/// OTEL auth configuration (nested under otel)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct OtelAuthFileConfig {
    /// Require API key for OTEL ingestion
    pub required: Option<bool>,
}

/// OpenTelemetry configuration section
#[derive(Debug, Default, Clone, Deserialize)]
pub struct OtelFileConfig {
    pub grpc: Option<GrpcFileConfig>,
    pub retention: Option<RetentionFileConfig>,
    pub auth: Option<OtelAuthFileConfig>,
}

/// Pricing configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct PricingFileConfig {
    pub sync_hours: Option<u64>,
}

/// Update check configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct UpdateFileConfig {
    pub enabled: Option<bool>,
}

/// MCP server configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct McpFileConfig {
    pub enabled: Option<bool>,
}

/// Filesystem storage configuration
#[derive(Debug, Default, Clone, Deserialize)]
pub struct FilesFilesystemFileConfig {
    pub path: Option<String>,
}

/// S3 storage configuration
#[derive(Debug, Default, Clone, Deserialize)]
pub struct FilesS3FileConfig {
    pub bucket: Option<String>,
    pub prefix: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
}

/// File storage configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct FilesFileConfig {
    pub enabled: Option<bool>,
    pub storage: Option<StorageBackend>,
    pub quota_bytes: Option<u64>,
    pub filesystem: Option<FilesFilesystemFileConfig>,
    pub s3: Option<FilesS3FileConfig>,
}

/// Redis cache configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct RedisFileConfig {
    /// Connection URL for Redis-compatible backends
    pub url: Option<String>,
}

/// Memory cache configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct MemoryCacheFileConfig {
    /// Maximum number of cache entries
    pub max_entries: Option<u64>,
    /// Cache eviction policy
    pub eviction_policy: Option<EvictionPolicy>,
}

/// Rate limit configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct RateLimitFileConfig {
    pub enabled: Option<bool>,
    /// Enable per-IP rate limiting (for API, auth endpoints). Disabled by default.
    pub per_ip: Option<bool>,
    pub api_rpm: Option<u32>,
    pub ingestion_rpm: Option<u32>,
    pub auth_rpm: Option<u32>,
    pub files_rpm: Option<u32>,
    pub bypass_header: Option<String>,
}

/// PostgreSQL configuration section (from JSON config file)
///
/// Optimized for scalable SaaS deployments with connection pooling,
/// idle timeout, and query protection settings.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct PostgresFileConfig {
    /// PostgreSQL connection URL (or use SIDESEAT_POSTGRES_URL env var)
    pub url: Option<String>,
    /// Maximum number of connections in the pool (default: 20)
    pub max_connections: Option<u32>,
    /// Minimum number of connections to keep warm (default: 2)
    pub min_connections: Option<u32>,
    /// Connection acquire timeout in seconds (default: 30)
    pub acquire_timeout_secs: Option<u64>,
    /// Idle connection timeout in seconds (default: 600)
    pub idle_timeout_secs: Option<u64>,
    /// Max connection lifetime in seconds (default: 1800)
    pub max_lifetime_secs: Option<u64>,
    /// Statement timeout in seconds, 0 to disable (default: 60)
    pub statement_timeout_secs: Option<u64>,
}

/// ClickHouse configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ClickhouseFileConfig {
    /// ClickHouse connection URL (or use SIDESEAT_CLICKHOUSE_URL env var)
    pub url: Option<String>,
    /// Database name (default: "sideseat")
    pub database: Option<String>,
    /// Username for authentication
    pub user: Option<String>,
    /// Password for authentication
    pub password: Option<String>,
    /// Query timeout in seconds
    pub timeout_secs: Option<u64>,
    /// Enable LZ4 compression (default: true)
    pub compression: Option<bool>,
    /// Enable async inserts for high-throughput (default: true)
    pub async_insert: Option<bool>,
    /// Wait for async insert completion (default: false for max throughput)
    pub wait_for_async_insert: Option<bool>,
    /// Cluster name for distributed tables (enables sharding)
    pub cluster: Option<String>,
    /// Enable distributed/sharded tables (requires cluster to be set)
    pub distributed: Option<bool>,
}

/// Database configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct DatabaseFileConfig {
    /// Transactional backend: sqlite (default) or postgres
    pub transactional: Option<TransactionalBackend>,
    /// Analytics backend: duckdb (default) or clickhouse
    pub analytics: Option<AnalyticsBackend>,
    /// Cache backend: memory (default) or redis
    pub cache: Option<CacheBackendType>,
    /// PostgreSQL-specific configuration
    pub postgres: Option<PostgresFileConfig>,
    /// ClickHouse-specific configuration
    pub clickhouse: Option<ClickhouseFileConfig>,
    /// Redis cache configuration
    pub redis: Option<RedisFileConfig>,
    /// Memory cache configuration
    pub memory_cache: Option<MemoryCacheFileConfig>,
}

/// Secrets env backend configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct SecretsEnvFileConfig {
    pub prefix: Option<String>,
}

/// Secrets AWS backend configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct SecretsAwsFileConfig {
    pub region: Option<String>,
    pub prefix: Option<String>,
    pub recovery_window_days: Option<u32>,
}

/// Secrets Vault backend configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct SecretsVaultFileConfig {
    pub address: Option<String>,
    pub mount: Option<String>,
    pub prefix: Option<String>,
    pub token: Option<String>,
}

/// Secrets configuration section (from JSON config file)
#[derive(Debug, Default, Clone, Deserialize)]
pub struct SecretsFileConfig {
    pub backend: Option<SecretsBackend>,
    pub env: Option<SecretsEnvFileConfig>,
    pub aws: Option<SecretsAwsFileConfig>,
    pub vault: Option<SecretsVaultFileConfig>,
}

/// File-based configuration (JSON)
#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    pub server: Option<ServerFileConfig>,
    pub auth: Option<AuthFileConfig>,
    pub otel: Option<OtelFileConfig>,
    pub pricing: Option<PricingFileConfig>,
    pub files: Option<FilesFileConfig>,
    pub rate_limit: Option<RateLimitFileConfig>,
    pub update: Option<UpdateFileConfig>,
    pub database: Option<DatabaseFileConfig>,
    pub secrets: Option<SecretsFileConfig>,
    pub debug: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

impl FileConfig {
    /// Load configuration from a JSON file
    fn load_from_file(path: &Path) -> Result<Self> {
        tracing::debug!(path = %path.display(), "Loading config file");
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: Self = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
        tracing::trace!(config = ?config, "Parsed config file");
        Ok(config)
    }

    /// Warn about unknown fields in the config
    fn warn_unknown_fields(&self) {
        if let serde_json::Value::Object(map) = &self.extra
            && !map.is_empty()
        {
            let keys_str: String = map
                .keys()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            tracing::warn!(
                fields = %keys_str,
                "Unknown fields in config file (possible typos)"
            );
        }
    }

    /// Merge another FileConfig into this one (other takes precedence)
    fn merge(&mut self, other: FileConfig) {
        // Server
        if let Some(server) = other.server {
            let current = self.server.get_or_insert_with(ServerFileConfig::default);
            if server.host.is_some() {
                tracing::trace!(host = ?server.host, "Merging server.host");
                current.host = server.host;
            }
            if server.port.is_some() {
                tracing::trace!(port = ?server.port, "Merging server.port");
                current.port = server.port;
            }
            if let Some(mcp) = server.mcp {
                let current_mcp = current.mcp.get_or_insert_with(McpFileConfig::default);
                if mcp.enabled.is_some() {
                    tracing::trace!(enabled = ?mcp.enabled, "Merging server.mcp.enabled");
                    current_mcp.enabled = mcp.enabled;
                }
            }
        }

        // Auth
        if let Some(auth) = other.auth {
            let current = self.auth.get_or_insert_with(AuthFileConfig::default);
            if auth.enabled.is_some() {
                tracing::trace!(enabled = ?auth.enabled, "Merging auth.enabled");
                current.enabled = auth.enabled;
            }
        }

        // Otel (with nested grpc and retention)
        if let Some(otel) = other.otel {
            let current = self.otel.get_or_insert_with(OtelFileConfig::default);

            if let Some(grpc) = otel.grpc {
                let current_grpc = current.grpc.get_or_insert_with(GrpcFileConfig::default);
                if grpc.enabled.is_some() {
                    tracing::trace!(enabled = ?grpc.enabled, "Merging otel.grpc.enabled");
                    current_grpc.enabled = grpc.enabled;
                }
                if grpc.port.is_some() {
                    tracing::trace!(port = ?grpc.port, "Merging otel.grpc.port");
                    current_grpc.port = grpc.port;
                }
            }

            if let Some(retention) = otel.retention {
                let current_retention = current
                    .retention
                    .get_or_insert_with(RetentionFileConfig::default);
                if retention.max_age_minutes.is_some() {
                    tracing::trace!(max_age_minutes = ?retention.max_age_minutes, "Merging otel.retention.max_age_minutes");
                    current_retention.max_age_minutes = retention.max_age_minutes;
                }
                if retention.max_spans.is_some() {
                    tracing::trace!(max_spans = ?retention.max_spans, "Merging otel.retention.max_spans");
                    current_retention.max_spans = retention.max_spans;
                }
            }
        }

        // Pricing
        if let Some(pricing) = other.pricing {
            let current = self.pricing.get_or_insert_with(PricingFileConfig::default);
            if pricing.sync_hours.is_some() {
                tracing::trace!(sync_hours = ?pricing.sync_hours, "Merging pricing.sync_hours");
                current.sync_hours = pricing.sync_hours;
            }
        }

        // Files
        if let Some(files) = other.files {
            let current = self.files.get_or_insert_with(FilesFileConfig::default);
            if files.enabled.is_some() {
                tracing::trace!(enabled = ?files.enabled, "Merging files.enabled");
                current.enabled = files.enabled;
            }
            if files.storage.is_some() {
                tracing::trace!(storage = ?files.storage, "Merging files.storage");
                current.storage = files.storage;
            }
            if files.quota_bytes.is_some() {
                tracing::trace!(quota_bytes = ?files.quota_bytes, "Merging files.quota_bytes");
                current.quota_bytes = files.quota_bytes;
            }
            if let Some(fs) = files.filesystem {
                let current_fs = current
                    .filesystem
                    .get_or_insert_with(FilesFilesystemFileConfig::default);
                if fs.path.is_some() {
                    tracing::trace!(path = ?fs.path, "Merging files.filesystem.path");
                    current_fs.path = fs.path;
                }
            }
            if let Some(s3) = files.s3 {
                let current_s3 = current.s3.get_or_insert_with(FilesS3FileConfig::default);
                if s3.bucket.is_some() {
                    tracing::trace!(bucket = ?s3.bucket, "Merging files.s3.bucket");
                    current_s3.bucket = s3.bucket;
                }
                if s3.prefix.is_some() {
                    tracing::trace!(prefix = ?s3.prefix, "Merging files.s3.prefix");
                    current_s3.prefix = s3.prefix;
                }
                if s3.region.is_some() {
                    tracing::trace!(region = ?s3.region, "Merging files.s3.region");
                    current_s3.region = s3.region;
                }
                if s3.endpoint.is_some() {
                    tracing::trace!(endpoint = ?s3.endpoint, "Merging files.s3.endpoint");
                    current_s3.endpoint = s3.endpoint;
                }
            }
        }

        // Rate Limit
        if let Some(rate_limit) = other.rate_limit {
            let current = self
                .rate_limit
                .get_or_insert_with(RateLimitFileConfig::default);
            if rate_limit.enabled.is_some() {
                tracing::trace!(enabled = ?rate_limit.enabled, "Merging rate_limit.enabled");
                current.enabled = rate_limit.enabled;
            }
            if rate_limit.per_ip.is_some() {
                tracing::trace!(per_ip = ?rate_limit.per_ip, "Merging rate_limit.per_ip");
                current.per_ip = rate_limit.per_ip;
            }
            if rate_limit.api_rpm.is_some() {
                tracing::trace!(api_rpm = ?rate_limit.api_rpm, "Merging rate_limit.api_rpm");
                current.api_rpm = rate_limit.api_rpm;
            }
            if rate_limit.ingestion_rpm.is_some() {
                tracing::trace!(ingestion_rpm = ?rate_limit.ingestion_rpm, "Merging rate_limit.ingestion_rpm");
                current.ingestion_rpm = rate_limit.ingestion_rpm;
            }
            if rate_limit.auth_rpm.is_some() {
                tracing::trace!(auth_rpm = ?rate_limit.auth_rpm, "Merging rate_limit.auth_rpm");
                current.auth_rpm = rate_limit.auth_rpm;
            }
            if rate_limit.files_rpm.is_some() {
                tracing::trace!(files_rpm = ?rate_limit.files_rpm, "Merging rate_limit.files_rpm");
                current.files_rpm = rate_limit.files_rpm;
            }
            if rate_limit.bypass_header.is_some() {
                tracing::trace!(bypass_header = "***", "Merging rate_limit.bypass_header");
                current.bypass_header = rate_limit.bypass_header;
            }
        }

        // Update
        if let Some(update) = other.update {
            let current = self.update.get_or_insert_with(UpdateFileConfig::default);
            if update.enabled.is_some() {
                tracing::trace!(enabled = ?update.enabled, "Merging update.enabled");
                current.enabled = update.enabled;
            }
        }

        // Database
        if let Some(database) = other.database {
            let current = self
                .database
                .get_or_insert_with(DatabaseFileConfig::default);
            if database.transactional.is_some() {
                tracing::trace!(transactional = ?database.transactional, "Merging database.transactional");
                current.transactional = database.transactional;
            }
            if database.analytics.is_some() {
                tracing::trace!(analytics = ?database.analytics, "Merging database.analytics");
                current.analytics = database.analytics;
            }
            if let Some(postgres) = database.postgres {
                let current_pg = current
                    .postgres
                    .get_or_insert_with(PostgresFileConfig::default);
                if postgres.url.is_some() {
                    tracing::trace!(url = "***", "Merging database.postgres.url");
                    current_pg.url = postgres.url;
                }
                if postgres.max_connections.is_some() {
                    tracing::trace!(max_connections = ?postgres.max_connections, "Merging database.postgres.max_connections");
                    current_pg.max_connections = postgres.max_connections;
                }
                if postgres.acquire_timeout_secs.is_some() {
                    tracing::trace!(acquire_timeout_secs = ?postgres.acquire_timeout_secs, "Merging database.postgres.acquire_timeout_secs");
                    current_pg.acquire_timeout_secs = postgres.acquire_timeout_secs;
                }
            }
            if let Some(clickhouse) = database.clickhouse {
                let current_ch = current
                    .clickhouse
                    .get_or_insert_with(ClickhouseFileConfig::default);
                if clickhouse.url.is_some() {
                    tracing::trace!(url = "***", "Merging database.clickhouse.url");
                    current_ch.url = clickhouse.url;
                }
                if clickhouse.database.is_some() {
                    tracing::trace!(database = ?clickhouse.database, "Merging database.clickhouse.database");
                    current_ch.database = clickhouse.database;
                }
                if clickhouse.user.is_some() {
                    tracing::trace!(user = "***", "Merging database.clickhouse.user");
                    current_ch.user = clickhouse.user;
                }
                if clickhouse.password.is_some() {
                    tracing::trace!(password = "***", "Merging database.clickhouse.password");
                    current_ch.password = clickhouse.password;
                }
                if clickhouse.timeout_secs.is_some() {
                    tracing::trace!(timeout_secs = ?clickhouse.timeout_secs, "Merging database.clickhouse.timeout_secs");
                    current_ch.timeout_secs = clickhouse.timeout_secs;
                }
                if clickhouse.compression.is_some() {
                    tracing::trace!(compression = ?clickhouse.compression, "Merging database.clickhouse.compression");
                    current_ch.compression = clickhouse.compression;
                }
                if clickhouse.async_insert.is_some() {
                    tracing::trace!(async_insert = ?clickhouse.async_insert, "Merging database.clickhouse.async_insert");
                    current_ch.async_insert = clickhouse.async_insert;
                }
                if clickhouse.wait_for_async_insert.is_some() {
                    tracing::trace!(wait_for_async_insert = ?clickhouse.wait_for_async_insert, "Merging database.clickhouse.wait_for_async_insert");
                    current_ch.wait_for_async_insert = clickhouse.wait_for_async_insert;
                }
                if clickhouse.cluster.is_some() {
                    tracing::trace!(cluster = ?clickhouse.cluster, "Merging database.clickhouse.cluster");
                    current_ch.cluster = clickhouse.cluster;
                }
                if clickhouse.distributed.is_some() {
                    tracing::trace!(distributed = ?clickhouse.distributed, "Merging database.clickhouse.distributed");
                    current_ch.distributed = clickhouse.distributed;
                }
            }
            if database.cache.is_some() {
                tracing::trace!(cache = ?database.cache, "Merging database.cache");
                current.cache = database.cache;
            }
            if let Some(redis) = database.redis {
                let current_redis = current.redis.get_or_insert_with(RedisFileConfig::default);
                if redis.url.is_some() {
                    tracing::trace!(url = "***", "Merging database.redis.url");
                    current_redis.url = redis.url;
                }
            }
            if let Some(memory_cache) = database.memory_cache {
                let current_mc = current
                    .memory_cache
                    .get_or_insert_with(MemoryCacheFileConfig::default);
                if memory_cache.max_entries.is_some() {
                    tracing::trace!(max_entries = ?memory_cache.max_entries, "Merging database.memory_cache.max_entries");
                    current_mc.max_entries = memory_cache.max_entries;
                }
                if memory_cache.eviction_policy.is_some() {
                    tracing::trace!(eviction_policy = ?memory_cache.eviction_policy, "Merging database.memory_cache.eviction_policy");
                    current_mc.eviction_policy = memory_cache.eviction_policy;
                }
            }
        }

        // Secrets
        if let Some(secrets) = other.secrets {
            let current = self.secrets.get_or_insert_with(SecretsFileConfig::default);
            if secrets.backend.is_some() {
                tracing::trace!(backend = ?secrets.backend, "Merging secrets.backend");
                current.backend = secrets.backend;
            }
            if let Some(env_cfg) = secrets.env {
                let ce = current
                    .env
                    .get_or_insert_with(SecretsEnvFileConfig::default);
                if env_cfg.prefix.is_some() {
                    ce.prefix = env_cfg.prefix;
                }
            }
            if let Some(aws_cfg) = secrets.aws {
                let ca = current
                    .aws
                    .get_or_insert_with(SecretsAwsFileConfig::default);
                if aws_cfg.region.is_some() {
                    ca.region = aws_cfg.region;
                }
                if aws_cfg.prefix.is_some() {
                    ca.prefix = aws_cfg.prefix;
                }
                if aws_cfg.recovery_window_days.is_some() {
                    ca.recovery_window_days = aws_cfg.recovery_window_days;
                }
            }
            if let Some(vault_cfg) = secrets.vault {
                let cv = current
                    .vault
                    .get_or_insert_with(SecretsVaultFileConfig::default);
                if vault_cfg.address.is_some() {
                    tracing::trace!(address = "***", "Merging secrets.vault.address");
                    cv.address = vault_cfg.address;
                }
                if vault_cfg.mount.is_some() {
                    cv.mount = vault_cfg.mount;
                }
                if vault_cfg.prefix.is_some() {
                    cv.prefix = vault_cfg.prefix;
                }
                if vault_cfg.token.is_some() {
                    tracing::trace!(token = "***", "Merging secrets.vault.token");
                    cv.token = vault_cfg.token;
                }
            }
        }

        // Debug
        if other.debug.is_some() {
            tracing::trace!(debug = ?other.debug, "Merging debug");
            self.debug = other.debug;
        }
    }
}

// =============================================================================
// Runtime Config Structs (final merged configuration)
// =============================================================================

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

/// Authentication configuration
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub enabled: bool,
}

/// OpenTelemetry configuration (includes retention)
#[derive(Debug, Clone)]
pub struct OtelConfig {
    pub grpc_enabled: bool,
    pub grpc_port: u16,
    pub retention: RetentionConfig,
    /// Require API key for OTEL ingestion
    pub auth_required: bool,
}

/// Retention configuration
#[derive(Debug, Clone, Default)]
pub struct RetentionConfig {
    pub max_age_minutes: Option<u64>,
    pub max_spans: Option<u64>,
}

/// Pricing configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct PricingConfig {
    pub sync_hours: u64,
}

/// S3 configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct S3Config {
    pub bucket: String,
    pub prefix: String,
    pub region: Option<String>,
    pub endpoint: Option<String>,
}

/// File storage configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct FilesConfig {
    pub enabled: bool,
    pub storage: StorageBackend,
    pub quota_bytes: u64,
    pub filesystem_path: Option<String>,
    pub s3: Option<S3Config>,
}

/// Update check configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct UpdateConfig {
    pub enabled: bool,
}

/// MCP server configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct McpConfig {
    pub enabled: bool,
}

/// Redis cache configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct RedisConfig {
    /// Connection URL for Redis-compatible backends
    pub url: String,
}

/// Memory cache configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct MemoryCacheConfig {
    /// Maximum number of cache entries
    pub max_entries: u64,
    /// Cache eviction policy
    pub eviction_policy: EvictionPolicy,
}

/// Cache configuration (used internally by CacheService)
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Cache backend type
    pub backend: CacheBackendType,
    /// Maximum entries (memory backend)
    pub max_entries: u64,
    /// Eviction policy (memory backend)
    pub eviction_policy: EvictionPolicy,
    /// Redis URL (redis backend)
    pub redis_url: Option<String>,
}

/// Rate limit configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Enable rate limiting (per-project by default)
    pub enabled: bool,
    /// Enable per-IP rate limiting (API, auth endpoints). Disabled by default.
    pub per_ip: bool,
    pub api_rpm: u32,
    pub ingestion_rpm: u32,
    pub auth_rpm: u32,
    pub files_rpm: u32,
    pub bypass_header: Option<String>,
}

/// PostgreSQL configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct PostgresConfig {
    /// PostgreSQL connection URL
    pub url: String,
    /// Maximum number of connections in the pool
    pub max_connections: u32,
    /// Minimum number of connections to keep warm
    pub min_connections: u32,
    /// Connection acquire timeout in seconds
    pub acquire_timeout_secs: u64,
    /// Idle connection timeout in seconds
    pub idle_timeout_secs: u64,
    /// Max connection lifetime in seconds
    pub max_lifetime_secs: u64,
    /// Statement timeout in seconds (0 = disabled)
    pub statement_timeout_secs: u64,
}

/// ClickHouse configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct ClickhouseConfig {
    /// ClickHouse connection URL
    pub url: String,
    /// Database name
    pub database: String,
    /// Username for authentication
    pub user: Option<String>,
    /// Password for authentication
    pub password: Option<String>,
    /// Query timeout in seconds
    pub timeout_secs: u64,
    /// Enable LZ4 compression for requests/responses
    pub compression: bool,
    /// Enable async inserts for high-throughput ingestion
    pub async_insert: bool,
    /// Wait for async insert to complete (false = fire-and-forget for max throughput)
    pub wait_for_async_insert: bool,
    /// Cluster name for distributed tables (None = single-node mode)
    pub cluster: Option<String>,
    /// Enable distributed/sharded tables (requires cluster to be set)
    pub distributed: bool,
}

/// Database configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// Transactional backend: sqlite (default) or postgres
    pub transactional: TransactionalBackend,
    /// Analytics backend: duckdb (default) or clickhouse
    pub analytics: AnalyticsBackend,
    /// Cache backend: memory (default) or redis
    pub cache: CacheBackendType,
    /// PostgreSQL-specific configuration (only used if transactional = postgres)
    pub postgres: Option<PostgresConfig>,
    /// ClickHouse-specific configuration (only used if analytics = clickhouse)
    pub clickhouse: Option<ClickhouseConfig>,
    /// Redis cache configuration (only used if cache = redis)
    pub redis: Option<RedisConfig>,
    /// Memory cache configuration
    pub memory_cache: MemoryCacheConfig,
}

// =============================================================================
// Secrets Runtime Config
// =============================================================================

/// Secrets env backend configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct SecretsEnvConfig {
    pub prefix: String,
}

/// Secrets AWS backend configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct SecretsAwsConfig {
    pub region: Option<String>,
    pub prefix: String,
    pub recovery_window_days: Option<u32>,
}

/// Secrets Vault backend configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct SecretsVaultConfig {
    pub address: String,
    pub mount: String,
    pub prefix: String,
    pub token: String,
}

/// Secrets configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct SecretsConfig {
    pub backend: SecretsBackend,
    pub env: Option<SecretsEnvConfig>,
    pub aws: Option<SecretsAwsConfig>,
    pub vault: Option<SecretsVaultConfig>,
}

impl DatabaseConfig {
    /// Build a CacheConfig for use by CacheService
    pub fn cache_config(&self) -> CacheConfig {
        CacheConfig {
            backend: self.cache,
            max_entries: self.memory_cache.max_entries,
            eviction_policy: self.memory_cache.eviction_policy,
            redis_url: self.redis.as_ref().map(|r| r.url.clone()),
        }
    }
}

/// Final merged application configuration
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub otel: OtelConfig,
    pub pricing: PricingConfig,
    pub files: FilesConfig,
    pub rate_limit: RateLimitConfig,
    pub update: UpdateConfig,
    pub mcp: McpConfig,
    pub database: DatabaseConfig,
    pub secrets: SecretsConfig,
    pub debug: bool,
}

impl AppConfig {
    /// Load configuration from all sources
    ///
    /// Priority (lowest to highest):
    /// 1. Defaults
    /// 2. Profile directory config (~/.sideseat/sideseat.json)
    /// 3. Local directory config OR CLI-specified config path
    /// 4. CLI arguments (which include env var fallbacks via clap)
    pub fn load(cli: &CliConfig) -> Result<Self> {
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
                anyhow::bail!("Config file not found: {}", expanded.display());
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
        let s3_config = if storage_backend == StorageBackend::S3 {
            file_files.s3.as_ref().and_then(|s3| {
                s3.bucket
                    .as_ref()
                    .filter(|b| !b.is_empty())
                    .map(|bucket| S3Config {
                        bucket: bucket.clone(),
                        prefix: s3
                            .prefix
                            .clone()
                            .unwrap_or_else(|| FILES_DEFAULT_S3_PREFIX.to_string()),
                        region: s3.region.clone(),
                        endpoint: s3.endpoint.clone(),
                    })
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

        // cache config: CLI/env overrides file config
        let cache_backend = cli
            .cache_backend
            .or(file_database.cache)
            .unwrap_or_default();

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
        let redis_config = if cache_backend == CacheBackendType::Redis {
            let file_redis = file_database.redis.unwrap_or_default();
            let url = cli
                .cache_redis_url
                .clone()
                .or(file_redis.url)
                .unwrap_or_default();
            Some(RedisConfig { url })
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
            let async_insert = file_ch.async_insert.unwrap_or(true);
            let wait_for_async_insert = file_ch.wait_for_async_insert.unwrap_or(false);
            let cluster = file_ch.cluster;
            // Distributed mode requires cluster to be set
            let distributed = file_ch.distributed.unwrap_or(false) && cluster.is_some();
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
            })
        } else {
            None
        };

        let database = DatabaseConfig {
            transactional: transactional_backend,
            analytics: analytics_backend,
            cache: cache_backend,
            postgres: postgres_config,
            clickhouse: clickhouse_config,
            redis: redis_config,
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
            transactional_backend = %config.database.transactional,
            analytics_backend = %config.database.analytics,
            "Configuration loaded"
        );

        Ok(config)
    }

    /// Validate the configuration for consistency and correctness
    fn validate(&self) -> Result<()> {
        // Host must not be empty
        if self.server.host.is_empty() {
            anyhow::bail!("Configuration error: server.host must not be empty");
        }

        // Port must be non-zero (port 0 would cause bind failure)
        if self.server.port == 0 {
            anyhow::bail!("Configuration error: server.port must be greater than 0");
        }
        if self.otel.grpc_enabled && self.otel.grpc_port == 0 {
            anyhow::bail!("Configuration error: otel.grpc.port must be greater than 0");
        }

        // Port collision check (only if both are enabled)
        if self.otel.grpc_enabled && self.server.port == self.otel.grpc_port {
            anyhow::bail!(
                "Configuration error: server.port ({}) and otel.grpc.port ({}) cannot be the same",
                self.server.port,
                self.otel.grpc_port
            );
        }

        // S3 bucket required when using S3 storage
        if self.files.storage == StorageBackend::S3 && self.files.s3.is_none() {
            anyhow::bail!(
                "Configuration error: files.s3.bucket is required (and non-empty) when files.storage is 's3'"
            );
        }

        // Redis URL required when using Redis cache backend
        if self.database.cache == CacheBackendType::Redis
            && self
                .database
                .redis
                .as_ref()
                .is_none_or(|r| r.url.is_empty())
        {
            anyhow::bail!(
                "Configuration error: database.redis.url is required when database.cache is 'redis'"
            );
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

        // Warn about low quota
        if self.files.enabled && self.files.quota_bytes < 1024 * 1024 {
            tracing::warn!(
                quota_bytes = self.files.quota_bytes,
                "files.quota_bytes is less than 1MB, file storage may fill up quickly"
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
                    anyhow::bail!(
                        "Configuration error: database.postgres.url is required when database.transactional is 'postgres'. \
                         Set via SIDESEAT_POSTGRES_URL env var or database.postgres.url in config file."
                    );
                }
            } else {
                anyhow::bail!(
                    "Configuration error: PostgreSQL configuration missing when database.transactional is 'postgres'"
                );
            }
        }

        // ClickHouse URL required when using ClickHouse backend
        if self.database.analytics == AnalyticsBackend::Clickhouse {
            if let Some(ref ch) = self.database.clickhouse {
                if ch.url.is_empty() {
                    anyhow::bail!(
                        "Configuration error: database.clickhouse.url is required when database.analytics is 'clickhouse'. \
                         Set via SIDESEAT_CLICKHOUSE_URL env var or database.clickhouse.url in config file."
                    );
                }
                // Cluster name required when distributed mode is enabled
                if ch.distributed && ch.cluster.as_ref().is_none_or(|c| c.is_empty()) {
                    anyhow::bail!(
                        "Configuration error: database.clickhouse.cluster is required when database.clickhouse.distributed is true. \
                         Specify the ClickHouse cluster name for distributed table creation."
                    );
                }
            } else {
                anyhow::bail!(
                    "Configuration error: ClickHouse configuration missing when database.analytics is 'clickhouse'"
                );
            }
        }

        // AWS recovery_window_days must be 7-30 if set
        if let Some(ref aws) = self.secrets.aws
            && let Some(d) = aws.recovery_window_days
            && !(7..=30).contains(&d)
        {
            anyhow::bail!(
                "Configuration error: secrets.aws.recovery_window_days must be between 7 and 30 (got {})",
                d
            );
        }

        // Vault address and token required when using Vault secrets backend
        if self.secrets.backend == SecretsBackend::Vault {
            if let Some(ref v) = self.secrets.vault {
                if v.address.is_empty() {
                    anyhow::bail!(
                        "Configuration error: secrets.vault.address is required when secrets.backend is 'vault'. \
                         Set via {} env var or secrets.vault.address in config file.",
                        ENV_SECRETS_VAULT_ADDR
                    );
                }
                if !v.address.starts_with("http://") && !v.address.starts_with("https://") {
                    anyhow::bail!(
                        "Configuration error: secrets.vault.address must start with http:// or https://. Got: {}",
                        v.address
                    );
                }
                if v.token.is_empty() {
                    anyhow::bail!(
                        "Configuration error: Vault token required when secrets.backend is 'vault'. \
                         Set via VAULT_TOKEN, {} env var, or secrets.vault.token in config file.",
                        ENV_SECRETS_VAULT_TOKEN
                    );
                }
            } else {
                anyhow::bail!(
                    "Configuration error: Vault configuration missing when secrets.backend is 'vault'"
                );
            }
        }

        Ok(())
    }
}

/// Get the profile config path (~/.sideseat/sideseat.json)
fn get_profile_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(APP_DOT_FOLDER).join(CONFIG_FILE_NAME))
}

/// Check if host binds to all network interfaces
fn is_all_interfaces(host: &str) -> bool {
    matches!(host, "0.0.0.0" | "::" | "[::]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_backend_serde() {
        let json = r#""filesystem""#;
        let backend: StorageBackend = serde_json::from_str(json).unwrap();
        assert_eq!(backend, StorageBackend::Filesystem);

        let json = r#""s3""#;
        let backend: StorageBackend = serde_json::from_str(json).unwrap();
        assert_eq!(backend, StorageBackend::S3);
    }

    #[test]
    fn test_storage_backend_display() {
        assert_eq!(StorageBackend::Filesystem.to_string(), "filesystem");
        assert_eq!(StorageBackend::S3.to_string(), "s3");
    }

    #[test]
    fn test_file_config_parse_full() {
        let json = r#"{
            "server": { "host": "0.0.0.0", "port": 8080 },
            "auth": { "enabled": false }
        }"#;
        let config: FileConfig = serde_json::from_str(json).unwrap();

        assert_eq!(
            config.server.as_ref().unwrap().host,
            Some("0.0.0.0".to_string())
        );
        assert_eq!(config.server.as_ref().unwrap().port, Some(8080));
        assert_eq!(config.auth.as_ref().unwrap().enabled, Some(false));
    }

    #[test]
    fn test_file_config_parse_nested_otel() {
        let json = r#"{
            "otel": {
                "grpc": { "enabled": false, "port": 4318 },
                "retention": { "max_age_minutes": 120, "max_spans": 1000000 }
            }
        }"#;
        let config: FileConfig = serde_json::from_str(json).unwrap();

        let otel = config.otel.as_ref().unwrap();
        let grpc = otel.grpc.as_ref().unwrap();
        let retention = otel.retention.as_ref().unwrap();

        assert_eq!(grpc.enabled, Some(false));
        assert_eq!(grpc.port, Some(4318));
        assert_eq!(retention.max_age_minutes, Some(120));
        assert_eq!(retention.max_spans, Some(1_000_000));
    }

    #[test]
    fn test_file_config_parse_partial() {
        let json = r#"{ "server": { "port": 9000 } }"#;
        let config: FileConfig = serde_json::from_str(json).unwrap();

        assert!(config.server.as_ref().unwrap().host.is_none());
        assert_eq!(config.server.as_ref().unwrap().port, Some(9000));
        assert!(config.auth.is_none());
    }

    #[test]
    fn test_file_config_parse_empty() {
        let json = "{}";
        let config: FileConfig = serde_json::from_str(json).unwrap();

        assert!(config.server.is_none());
        assert!(config.auth.is_none());
    }

    #[test]
    fn test_file_config_parse_extra_fields() {
        let json = r#"{ "server": { "host": "localhost" }, "unknown_field": 123 }"#;
        let config: FileConfig = serde_json::from_str(json).unwrap();

        assert_eq!(
            config.server.as_ref().unwrap().host,
            Some("localhost".to_string())
        );
        assert_eq!(config.extra.get("unknown_field").unwrap(), 123);
    }

    #[test]
    fn test_file_config_parse_storage_backend() {
        let json = r#"{ "files": { "storage": "s3", "enabled": true } }"#;
        let config: FileConfig = serde_json::from_str(json).unwrap();

        assert_eq!(
            config.files.as_ref().unwrap().storage,
            Some(StorageBackend::S3)
        );
        assert_eq!(config.files.as_ref().unwrap().enabled, Some(true));
    }

    #[test]
    fn test_file_config_merge() {
        let mut base = FileConfig {
            server: Some(ServerFileConfig {
                host: Some("base.host".to_string()),
                port: Some(1000),
                mcp: None,
            }),
            auth: Some(AuthFileConfig {
                enabled: Some(true),
            }),
            otel: Some(OtelFileConfig {
                grpc: Some(GrpcFileConfig {
                    enabled: Some(true),
                    port: Some(4317),
                }),
                retention: Some(RetentionFileConfig {
                    max_age_minutes: Some(60),
                    max_spans: None,
                }),
                auth: None,
            }),
            pricing: Some(PricingFileConfig {
                sync_hours: Some(4),
            }),
            files: None,
            rate_limit: None,
            update: None,
            database: None,
            secrets: None,
            debug: Some(false),
            extra: serde_json::Value::Null,
        };

        let overlay = FileConfig {
            server: Some(ServerFileConfig {
                host: None,
                port: Some(2000),
                mcp: None,
            }),
            auth: Some(AuthFileConfig {
                enabled: Some(false),
            }),
            otel: Some(OtelFileConfig {
                grpc: Some(GrpcFileConfig {
                    enabled: Some(false),
                    port: None,
                }),
                retention: Some(RetentionFileConfig {
                    max_age_minutes: None,
                    max_spans: Some(1_000_000),
                }),
                auth: None,
            }),
            pricing: Some(PricingFileConfig {
                sync_hours: Some(8),
            }),
            files: None,
            rate_limit: None,
            update: None,
            database: None,
            secrets: None,
            debug: Some(true),
            extra: serde_json::Value::Null,
        };

        base.merge(overlay);

        assert_eq!(
            base.server.as_ref().unwrap().host,
            Some("base.host".to_string())
        );
        assert_eq!(base.server.as_ref().unwrap().port, Some(2000));
        assert_eq!(base.auth.as_ref().unwrap().enabled, Some(false));

        let otel = base.otel.as_ref().unwrap();
        assert_eq!(otel.grpc.as_ref().unwrap().enabled, Some(false));
        assert_eq!(otel.grpc.as_ref().unwrap().port, Some(4317));
        assert_eq!(otel.retention.as_ref().unwrap().max_age_minutes, Some(60));
        assert_eq!(otel.retention.as_ref().unwrap().max_spans, Some(1_000_000));

        assert_eq!(base.pricing.as_ref().unwrap().sync_hours, Some(8));
        assert_eq!(base.debug, Some(true));
    }

    #[test]
    fn test_app_config_defaults() {
        let cli = CliConfig::default();
        let config = AppConfig::load(&cli).unwrap();

        assert_eq!(config.server.host, DEFAULT_HOST);
        assert_eq!(config.server.port, DEFAULT_PORT);
        assert!(config.auth.enabled);
        assert!(!config.debug);
        assert_eq!(config.files.storage, StorageBackend::Filesystem);
    }

    #[test]
    fn test_app_config_cli_override() {
        let cli = CliConfig {
            host: Some("cli.host".to_string()),
            port: Some(3000),
            no_auth: true,
            debug: true,
            config: None,
            otel_grpc: Some(false),
            otel_grpc_port: Some(4318),
            otel_retention_max_age: Some(120),
            otel_retention_max_spans: Some(1_000_000),
            otel_auth_required: None,
            pricing_sync_hours: Some(12),
            no_update_check: true,
            files_enabled: Some(false),
            files_storage: None,
            files_quota_bytes: Some(500_000_000),
            cache_backend: None,
            cache_max_entries: None,
            cache_eviction_policy: None,
            cache_redis_url: None,
            rate_limit_enabled: None,
            rate_limit_per_ip: None,
            rate_limit_api_rpm: None,
            rate_limit_ingestion_rpm: None,
            rate_limit_auth_rpm: None,
            rate_limit_files_rpm: None,
            rate_limit_bypass_header: None,
            secrets_backend: None,
            mcp: None,
            transactional_backend: None,
            analytics_backend: None,
            postgres_url: None,
            clickhouse_url: None,
        };
        let config = AppConfig::load(&cli).unwrap();

        assert_eq!(config.server.host, "cli.host");
        assert_eq!(config.server.port, 3000);
        assert!(!config.auth.enabled);
        assert!(config.debug);
        assert!(!config.otel.grpc_enabled);
        assert_eq!(config.otel.grpc_port, 4318);
        assert_eq!(config.otel.retention.max_age_minutes, Some(120));
        assert_eq!(config.otel.retention.max_spans, Some(1_000_000));
        assert_eq!(config.pricing.sync_hours, 12);
        assert!(!config.files.enabled);
        assert_eq!(config.files.quota_bytes, 500_000_000);
    }

    #[test]
    fn test_file_config_parse_pricing() {
        let json = r#"{ "pricing": { "sync_hours": 12 } }"#;
        let config: FileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.pricing.as_ref().unwrap().sync_hours, Some(12));
    }

    #[test]
    fn test_app_config_pricing_defaults() {
        let cli = CliConfig::default();
        let config = AppConfig::load(&cli).unwrap();
        assert_eq!(config.pricing.sync_hours, PRICING_SYNC_INTERVAL_SECS / 3600);
    }

    #[test]
    fn test_app_config_pricing_disabled() {
        let cli = CliConfig {
            pricing_sync_hours: Some(0),
            ..Default::default()
        };
        let config = AppConfig::load(&cli).unwrap();
        assert_eq!(config.pricing.sync_hours, 0);
    }

    #[test]
    fn test_file_config_parse_update() {
        let json = r#"{ "update": { "enabled": false } }"#;
        let config: FileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.update.as_ref().unwrap().enabled, Some(false));
    }

    #[test]
    fn test_app_config_update_defaults() {
        let cli = CliConfig::default();
        let config = AppConfig::load(&cli).unwrap();
        assert!(config.update.enabled);
    }

    #[test]
    fn test_app_config_update_cli_override() {
        let cli = CliConfig {
            no_update_check: true,
            ..Default::default()
        };
        let config = AppConfig::load(&cli).unwrap();
        assert!(!config.update.enabled);
    }

    #[test]
    fn test_app_config_validation_port_collision() {
        let cli = CliConfig {
            port: Some(4317),
            otel_grpc_port: Some(4317),
            ..Default::default()
        };
        let result = AppConfig::load(&cli);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot be the same")
        );
    }

    #[test]
    fn test_app_config_validation_s3_bucket_required() {
        let cli = CliConfig {
            files_storage: Some(StorageBackend::S3),
            ..Default::default()
        };
        let result = AppConfig::load(&cli);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("files.s3.bucket is required")
        );
    }

    #[test]
    fn test_app_config_validation_port_collision_disabled_grpc() {
        // Should NOT error if gRPC is disabled
        let cli = CliConfig {
            port: Some(4317),
            otel_grpc: Some(false),
            otel_grpc_port: Some(4317),
            ..Default::default()
        };
        let result = AppConfig::load(&cli);
        assert!(result.is_ok());
    }

    #[test]
    fn test_app_config_validation_server_port_zero() {
        let cli = CliConfig {
            port: Some(0),
            ..Default::default()
        };
        let result = AppConfig::load(&cli);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("server.port must be greater than 0")
        );
    }

    #[test]
    fn test_app_config_validation_grpc_port_zero() {
        let cli = CliConfig {
            otel_grpc: Some(true),
            otel_grpc_port: Some(0),
            ..Default::default()
        };
        let result = AppConfig::load(&cli);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("otel.grpc.port must be greater than 0")
        );
    }

    #[test]
    fn test_app_config_validation_grpc_port_zero_disabled() {
        // Port 0 should be OK if gRPC is disabled
        let cli = CliConfig {
            otel_grpc: Some(false),
            otel_grpc_port: Some(0),
            ..Default::default()
        };
        let result = AppConfig::load(&cli);
        assert!(result.is_ok());
    }

    #[test]
    fn test_app_config_validation_empty_host() {
        let cli = CliConfig {
            host: Some(String::new()),
            ..Default::default()
        };
        let result = AppConfig::load(&cli);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("server.host must not be empty")
        );
    }

    #[test]
    fn test_app_config_s3_empty_bucket_rejected() {
        // Test that empty bucket string in config file is rejected
        use std::io::Write;

        let json = r#"{
            "files": {
                "storage": "s3",
                "s3": { "bucket": "" }
            }
        }"#;

        // Create temp config file
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.write_all(json.as_bytes()).unwrap();

        // Load config pointing to temp file
        let cli = CliConfig {
            config: Some(temp_file.path().to_path_buf()),
            ..Default::default()
        };

        let result = AppConfig::load(&cli);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("files.s3.bucket is required")
        );
    }

    #[test]
    fn test_is_all_interfaces() {
        // Should match all-interfaces bindings
        assert!(is_all_interfaces("0.0.0.0"));
        assert!(is_all_interfaces("::"));
        assert!(is_all_interfaces("[::]"));

        // Should not match localhost or specific IPs
        assert!(!is_all_interfaces("127.0.0.1"));
        assert!(!is_all_interfaces("localhost"));
        assert!(!is_all_interfaces("::1"));
        assert!(!is_all_interfaces("192.168.1.1"));
    }

    #[test]
    fn test_app_config_s3_valid_bucket() {
        // Test that valid S3 config with non-empty bucket loads successfully
        use std::io::Write;

        let json = r#"{
            "files": {
                "storage": "s3",
                "s3": {
                    "bucket": "my-bucket",
                    "prefix": "custom/prefix",
                    "region": "us-west-2"
                }
            }
        }"#;

        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.write_all(json.as_bytes()).unwrap();

        let cli = CliConfig {
            config: Some(temp_file.path().to_path_buf()),
            ..Default::default()
        };

        let config = AppConfig::load(&cli).unwrap();
        assert_eq!(config.files.storage, StorageBackend::S3);
        assert!(config.files.s3.is_some());

        let s3 = config.files.s3.unwrap();
        assert_eq!(s3.bucket, "my-bucket");
        assert_eq!(s3.prefix, "custom/prefix");
        assert_eq!(s3.region, Some("us-west-2".to_string()));
        assert!(s3.endpoint.is_none());
    }

    #[test]
    fn test_file_config_parse_mcp_under_server() {
        let json = r#"{ "server": { "host": "0.0.0.0", "mcp": { "enabled": false } } }"#;
        let config: FileConfig = serde_json::from_str(json).unwrap();
        let server = config.server.unwrap();
        assert_eq!(server.host, Some("0.0.0.0".to_string()));
        assert_eq!(server.mcp.unwrap().enabled, Some(false));
    }

    #[test]
    fn test_file_config_parse_mcp_absent() {
        let json = r#"{ "server": { "port": 9000 } }"#;
        let config: FileConfig = serde_json::from_str(json).unwrap();
        assert!(config.server.unwrap().mcp.is_none());
    }

    #[test]
    fn test_file_config_merge_mcp() {
        let mut base = FileConfig {
            server: Some(ServerFileConfig {
                host: Some("localhost".to_string()),
                port: None,
                mcp: Some(McpFileConfig {
                    enabled: Some(true),
                }),
            }),
            ..Default::default()
        };
        let overlay = FileConfig {
            server: Some(ServerFileConfig {
                host: None,
                port: None,
                mcp: Some(McpFileConfig {
                    enabled: Some(false),
                }),
            }),
            ..Default::default()
        };
        base.merge(overlay);
        let mcp = base.server.unwrap().mcp.unwrap();
        assert_eq!(mcp.enabled, Some(false));
    }

    #[test]
    fn test_file_config_merge_mcp_partial() {
        let mut base = FileConfig {
            server: Some(ServerFileConfig {
                host: None,
                port: None,
                mcp: Some(McpFileConfig {
                    enabled: Some(true),
                }),
            }),
            ..Default::default()
        };
        // Overlay has server but no mcp — base mcp preserved
        let overlay = FileConfig {
            server: Some(ServerFileConfig {
                host: Some("new-host".to_string()),
                port: None,
                mcp: None,
            }),
            ..Default::default()
        };
        base.merge(overlay);
        let server = base.server.unwrap();
        assert_eq!(server.host, Some("new-host".to_string()));
        assert_eq!(server.mcp.unwrap().enabled, Some(true));
    }

    #[test]
    fn test_app_config_mcp_enabled_by_default() {
        let cli = CliConfig::default();
        let config = AppConfig::load(&cli).unwrap();
        assert!(config.mcp.enabled);
    }

    #[test]
    fn test_app_config_mcp_cli_override() {
        let cli = CliConfig {
            mcp: Some(false),
            ..Default::default()
        };
        let config = AppConfig::load(&cli).unwrap();
        assert!(!config.mcp.enabled);
    }

    #[test]
    fn test_app_config_mcp_file_override() {
        use std::io::Write;
        let json = r#"{ "server": { "mcp": { "enabled": false } } }"#;
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.write_all(json.as_bytes()).unwrap();
        let cli = CliConfig {
            config: Some(temp_file.path().to_path_buf()),
            ..Default::default()
        };
        let config = AppConfig::load(&cli).unwrap();
        assert!(!config.mcp.enabled);
    }

    #[test]
    fn test_secrets_aws_recovery_window_days_from_json() {
        let json = r#"{ "secrets": { "backend": "aws", "aws": { "recovery_window_days": 14 } } }"#;
        let config: FileConfig = serde_json::from_str(json).unwrap();
        let aws = config.secrets.unwrap().aws.unwrap();
        assert_eq!(aws.recovery_window_days, Some(14));
    }

    #[test]
    fn test_secrets_aws_recovery_window_days_validation_too_low() {
        use std::io::Write;
        let json = r#"{ "secrets": { "backend": "aws", "aws": { "recovery_window_days": 5 } } }"#;
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.write_all(json.as_bytes()).unwrap();
        let cli = CliConfig {
            config: Some(temp_file.path().to_path_buf()),
            secrets_backend: Some(SecretsBackend::Aws),
            ..Default::default()
        };
        let result = AppConfig::load(&cli);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("between 7 and 30"));
    }

    #[test]
    fn test_secrets_aws_recovery_window_days_validation_too_high() {
        use std::io::Write;
        let json = r#"{ "secrets": { "backend": "aws", "aws": { "recovery_window_days": 50 } } }"#;
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.write_all(json.as_bytes()).unwrap();
        let cli = CliConfig {
            config: Some(temp_file.path().to_path_buf()),
            secrets_backend: Some(SecretsBackend::Aws),
            ..Default::default()
        };
        let result = AppConfig::load(&cli);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("between 7 and 30"));
    }

    #[test]
    fn test_secrets_aws_recovery_window_days_valid() {
        use std::io::Write;
        let json = r#"{ "secrets": { "backend": "aws", "aws": { "recovery_window_days": 7 } } }"#;
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.write_all(json.as_bytes()).unwrap();
        let cli = CliConfig {
            config: Some(temp_file.path().to_path_buf()),
            secrets_backend: Some(SecretsBackend::Aws),
            ..Default::default()
        };
        let config = AppConfig::load(&cli).unwrap();
        let aws = config.secrets.aws.unwrap();
        assert_eq!(aws.recovery_window_days, Some(7));
    }

    #[test]
    fn test_secrets_aws_recovery_window_days_omitted() {
        use std::io::Write;
        let json = r#"{ "secrets": { "backend": "aws", "aws": { "region": "us-east-1" } } }"#;
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        temp_file.write_all(json.as_bytes()).unwrap();
        let cli = CliConfig {
            config: Some(temp_file.path().to_path_buf()),
            secrets_backend: Some(SecretsBackend::Aws),
            ..Default::default()
        };
        let config = AppConfig::load(&cli).unwrap();
        let aws = config.secrets.aws.unwrap();
        assert!(aws.recovery_window_days.is_none());
    }
}
