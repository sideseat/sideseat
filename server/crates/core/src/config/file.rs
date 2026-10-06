use super::*;

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

/// Durable queue backend type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueBackendType {
    #[default]
    Memory,
    Redis,
    Redpanda,
}

impl fmt::Display for QueueBackendType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Memory => write!(f, "memory"),
            Self::Redis => write!(f, "redis"),
            Self::Redpanda => write!(f, "redpanda"),
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
///
/// One `strum` declaration carries the kebab-case spelling in every direction it is needed: the config
/// file's JSON (through `serde`), the `--secrets-backend` flag and `SIDESEAT_SECRETS_BACKEND` variable
/// (through `FromStr`, case-insensitively, as the CLI parser always was), and the name the startup
/// refusals print (through `Display`). `hashicorp` is accepted as a second spelling of `vault`, and
/// `vault` is the one printed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
pub enum SecretsBackend {
    Keychain,
    CredentialManager,
    SecretService,
    Keyutils,
    File,
    Env,
    Aws,
    /// `hashicorp` is the legacy spelling the CLI has always accepted.
    #[strum(to_string = "vault", serialize = "hashicorp")]
    Vault,
}

impl SecretsBackend {
    /// Auto-detect best available backend for the current platform.
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::Keychain
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
            Self::Keychain
                | Self::CredentialManager
                | Self::SecretService
                | Self::Keyutils
                | Self::File
        )
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
    pub staging_redrive_cap: Option<u32>,
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
    /// How many replicas must acknowledge a queued trace before the export is answered.
    pub min_replica_acks: Option<u32>,
}

/// RedPanda queue configuration section (from JSON config file).
#[derive(Debug, Default, Clone, Deserialize)]
pub struct RedpandaFileConfig {
    pub brokers: Option<String>,
    pub partitions: Option<i32>,
    pub replication_factor: Option<i32>,
    pub retention_ms: Option<u64>,
    pub retention_warning_ms: Option<u64>,
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
    /// See `RateLimitConfig::trusted_proxies`.
    pub trusted_proxies: Option<Vec<String>>,
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
    /// How many replicas of a shard must confirm an insert - see [`ClickhouseConfig::insert_quorum`].
    pub insert_quorum: Option<u32>,
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
    /// Queue backend: memory, redis, or redpanda.
    pub queue: Option<QueueBackendType>,
    /// PostgreSQL-specific configuration
    pub postgres: Option<PostgresFileConfig>,
    /// ClickHouse-specific configuration
    pub clickhouse: Option<ClickhouseFileConfig>,
    /// Redis cache configuration
    pub redis: Option<RedisFileConfig>,
    /// RedPanda queue configuration.
    pub redpanda: Option<RedpandaFileConfig>,
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

/// Credentials file configuration
#[derive(Debug, Default, Deserialize)]
pub struct CredentialsFileConfig {
    pub scan_env: Option<bool>,
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
    pub credentials: Option<CredentialsFileConfig>,
    pub debug: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

impl FileConfig {
    /// Load configuration from a JSON file
    pub(super) fn load_from_file(path: &Path) -> Result<Self, ConfigError> {
        tracing::debug!(path = %path.display(), "Loading config file");
        let content = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self = serde_json::from_str(&content).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        tracing::trace!(config = ?config, "Parsed config file");
        Ok(config)
    }

    /// Warn about unknown fields in the config
    pub(super) fn warn_unknown_fields(&self) {
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
    pub(super) fn merge(&mut self, other: FileConfig) {
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

            if let Some(auth) = otel.auth {
                let current_auth = current.auth.get_or_insert_with(OtelAuthFileConfig::default);
                if auth.required.is_some() {
                    tracing::trace!(required = ?auth.required, "Merging otel.auth.required");
                    current_auth.required = auth.required;
                }
            }

            if otel.staging_redrive_cap.is_some() {
                tracing::trace!(
                    staging_redrive_cap = ?otel.staging_redrive_cap,
                    "Merging otel.staging_redrive_cap"
                );
                current.staging_redrive_cap = otel.staging_redrive_cap;
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
            if rate_limit.trusted_proxies.is_some() {
                tracing::trace!(
                    trusted_proxies = ?rate_limit.trusted_proxies,
                    "Merging rate_limit.trusted_proxies"
                );
                current.trusted_proxies = rate_limit.trusted_proxies;
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
                if postgres.min_connections.is_some() {
                    tracing::trace!(min_connections = ?postgres.min_connections, "Merging database.postgres.min_connections");
                    current_pg.min_connections = postgres.min_connections;
                }
                if postgres.idle_timeout_secs.is_some() {
                    tracing::trace!(idle_timeout_secs = ?postgres.idle_timeout_secs, "Merging database.postgres.idle_timeout_secs");
                    current_pg.idle_timeout_secs = postgres.idle_timeout_secs;
                }
                if postgres.max_lifetime_secs.is_some() {
                    tracing::trace!(max_lifetime_secs = ?postgres.max_lifetime_secs, "Merging database.postgres.max_lifetime_secs");
                    current_pg.max_lifetime_secs = postgres.max_lifetime_secs;
                }
                if postgres.statement_timeout_secs.is_some() {
                    tracing::trace!(statement_timeout_secs = ?postgres.statement_timeout_secs, "Merging database.postgres.statement_timeout_secs");
                    current_pg.statement_timeout_secs = postgres.statement_timeout_secs;
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
                if clickhouse.insert_quorum.is_some() {
                    tracing::trace!(insert_quorum = ?clickhouse.insert_quorum, "Merging database.clickhouse.insert_quorum");
                    current_ch.insert_quorum = clickhouse.insert_quorum;
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
            if database.queue.is_some() {
                tracing::trace!(queue = ?database.queue, "Merging database.queue");
                current.queue = database.queue;
            }
            if let Some(redis) = database.redis {
                let current_redis = current.redis.get_or_insert_with(RedisFileConfig::default);
                if redis.url.is_some() {
                    tracing::trace!(url = "***", "Merging database.redis.url");
                    current_redis.url = redis.url;
                }
                if redis.min_replica_acks.is_some() {
                    tracing::trace!(min_replica_acks = ?redis.min_replica_acks, "Merging database.redis.min_replica_acks");
                    current_redis.min_replica_acks = redis.min_replica_acks;
                }
            }
            if let Some(redpanda) = database.redpanda {
                let current_redpanda = current
                    .redpanda
                    .get_or_insert_with(RedpandaFileConfig::default);
                if redpanda.brokers.is_some() {
                    tracing::trace!(brokers = "***", "Merging database.redpanda.brokers");
                    current_redpanda.brokers = redpanda.brokers;
                }
                if redpanda.partitions.is_some() {
                    tracing::trace!(partitions = ?redpanda.partitions, "Merging database.redpanda.partitions");
                    current_redpanda.partitions = redpanda.partitions;
                }
                if redpanda.replication_factor.is_some() {
                    tracing::trace!(
                        replication_factor = ?redpanda.replication_factor,
                        "Merging database.redpanda.replication_factor"
                    );
                    current_redpanda.replication_factor = redpanda.replication_factor;
                }
                if redpanda.retention_ms.is_some() {
                    tracing::trace!(retention_ms = ?redpanda.retention_ms, "Merging database.redpanda.retention_ms");
                    current_redpanda.retention_ms = redpanda.retention_ms;
                }
                if redpanda.retention_warning_ms.is_some() {
                    tracing::trace!(
                        retention_warning_ms = ?redpanda.retention_warning_ms,
                        "Merging database.redpanda.retention_warning_ms"
                    );
                    current_redpanda.retention_warning_ms = redpanda.retention_warning_ms;
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
                    tracing::trace!(prefix = ?env_cfg.prefix, "Merging secrets.env.prefix");
                    ce.prefix = env_cfg.prefix;
                }
            }
            if let Some(aws_cfg) = secrets.aws {
                let ca = current
                    .aws
                    .get_or_insert_with(SecretsAwsFileConfig::default);
                if aws_cfg.region.is_some() {
                    tracing::trace!(region = ?aws_cfg.region, "Merging secrets.aws.region");
                    ca.region = aws_cfg.region;
                }
                if aws_cfg.prefix.is_some() {
                    tracing::trace!(prefix = ?aws_cfg.prefix, "Merging secrets.aws.prefix");
                    ca.prefix = aws_cfg.prefix;
                }
                if aws_cfg.recovery_window_days.is_some() {
                    tracing::trace!(days = ?aws_cfg.recovery_window_days, "Merging secrets.aws.recovery_window_days");
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
                    tracing::trace!(mount = ?vault_cfg.mount, "Merging secrets.vault.mount");
                    cv.mount = vault_cfg.mount;
                }
                if vault_cfg.prefix.is_some() {
                    tracing::trace!(prefix = ?vault_cfg.prefix, "Merging secrets.vault.prefix");
                    cv.prefix = vault_cfg.prefix;
                }
                if vault_cfg.token.is_some() {
                    tracing::trace!(token = "***", "Merging secrets.vault.token");
                    cv.token = vault_cfg.token;
                }
            }
        }

        // Credentials
        if let Some(credentials) = other.credentials {
            let current = self
                .credentials
                .get_or_insert_with(CredentialsFileConfig::default);
            if credentials.scan_env.is_some() {
                tracing::trace!(scan_env = ?credentials.scan_env, "Merging credentials.scan_env");
                current.scan_env = credentials.scan_env;
            }
        }

        // Debug
        if other.debug.is_some() {
            tracing::trace!(debug = ?other.debug, "Merging debug");
            self.debug = other.debug;
        }
    }
}
