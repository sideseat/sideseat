// =============================================================================
// Application Identity
// =============================================================================

/// Application name in title case (for display and platform directories)
pub const APP_NAME: &str = "SideSeat";

/// Application name in lowercase (for paths and identifiers)
pub const APP_NAME_LOWER: &str = "sideseat";

/// Unix-style dotfile folder name
pub const APP_DOT_FOLDER: &str = ".sideseat";

// =============================================================================
// Configuration Files
// =============================================================================

/// Config file name
pub const CONFIG_FILE_NAME: &str = "sideseat.json";

/// Environment variable for config file path
pub const ENV_CONFIG: &str = "SIDESEAT_CONFIG";

// =============================================================================
// Environment Variables - Debug
// =============================================================================

/// Environment variable for debug mode
pub const ENV_DEBUG: &str = "SIDESEAT_DEBUG";

// =============================================================================
// Environment Variables - Server
// =============================================================================

/// Environment variable for server host
pub const ENV_HOST: &str = "SIDESEAT_HOST";

/// Environment variable for server port
pub const ENV_PORT: &str = "SIDESEAT_PORT";

/// Environment variable for log level/filter
pub const ENV_LOG: &str = "SIDESEAT_LOG";

// =============================================================================
// Server Defaults
// =============================================================================

/// Default server host
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Default server port
pub const DEFAULT_PORT: u16 = 5388;

// =============================================================================
// Environment Variables - Storage
// =============================================================================

/// Environment variable to override data directory
pub const ENV_DATA_DIR: &str = "SIDESEAT_DATA_DIR";

// =============================================================================
// Environment Variables - Secrets
// =============================================================================

/// Environment variable to force specific secrets backend
///
/// Platform-specific values:
/// - macOS: `keychain` (default)
/// - Windows: `credential-manager`
/// - Linux: `secret-service`, `keyutils`
/// - All platforms: `file`, `env`, `aws`, `vault`
pub const ENV_SECRETS_BACKEND: &str = "SIDESEAT_SECRETS_BACKEND";

/// Service name for keychain/credential manager entries
pub const SECRET_SERVICE_NAME: &str = "sideseat";

/// Secret key name for JWT signing key
pub const SECRET_KEY_JWT_SIGNING: &str = "jwt_signing_key";

// =============================================================================
// Secrets Backends
// =============================================================================

pub const ENV_SECRETS_ENV_PREFIX: &str = "SIDESEAT_SECRETS_ENV_PREFIX";
pub const ENV_SECRETS_AWS_REGION: &str = "SIDESEAT_SECRETS_AWS_REGION";
pub const ENV_SECRETS_AWS_PREFIX: &str = "SIDESEAT_SECRETS_AWS_PREFIX";
pub const ENV_SECRETS_VAULT_ADDR: &str = "SIDESEAT_SECRETS_VAULT_ADDR";
pub const ENV_SECRETS_VAULT_TOKEN: &str = "SIDESEAT_SECRETS_VAULT_TOKEN";
pub const ENV_SECRETS_VAULT_MOUNT: &str = "SIDESEAT_SECRETS_VAULT_MOUNT";
pub const ENV_SECRETS_VAULT_PREFIX: &str = "SIDESEAT_SECRETS_VAULT_PREFIX";

pub const SECRETS_DEFAULT_AWS_PREFIX: &str = "sideseat";
pub const SECRETS_DEFAULT_VAULT_MOUNT: &str = "secret";
pub const SECRETS_DEFAULT_VAULT_PREFIX: &str = "sideseat";
pub const SECRETS_DEFAULT_ENV_PREFIX: &str = "SIDESEAT_SECRET_";

// =============================================================================
// Authentication
// =============================================================================

/// Cookie name for session token
pub const SESSION_COOKIE_NAME: &str = "sideseat_session";

/// Default session TTL in days
pub const DEFAULT_SESSION_TTL_DAYS: u32 = 30;

// =============================================================================
// SQLite Database
// =============================================================================

/// SQLite database filename
pub const SQLITE_DB_FILENAME: &str = "sideseat.db";

/// SQLite connection pool max connections
pub const SQLITE_MAX_CONNECTIONS: u32 = 5;

/// SQLite busy timeout in seconds
pub const SQLITE_BUSY_TIMEOUT_SECS: u64 = 30;

/// SQLite cache size (negative = KB, so -64000 = 64MB)
pub const SQLITE_CACHE_SIZE: &str = "-64000";

/// SQLite WAL auto-checkpoint threshold (pages, ~4MB at 1000)
pub const SQLITE_WAL_AUTOCHECKPOINT: &str = "1000";

/// WAL checkpoint interval in seconds (5 minutes)
pub const SQLITE_CHECKPOINT_INTERVAL_SECS: u64 = 300;

// =============================================================================
// DuckDB Database
// =============================================================================

/// DuckDB database filename
pub const DUCKDB_DB_FILENAME: &str = "sideseat.duckdb";

/// DuckDB checkpoint interval in seconds (5 minutes)
pub const DUCKDB_CHECKPOINT_INTERVAL_SECS: u64 = 300;

/// DuckDB retention check interval in seconds (300 seconds)
pub const DUCKDB_RETENTION_INTERVAL_SECS: u64 = 300;

/// DuckDB API query timeout in seconds
pub const DUCKDB_QUERY_TIMEOUT_SECS: u64 = 30;

// =============================================================================
// OTEL Retention
// =============================================================================

/// Environment variable for OTEL retention max age in minutes
pub const ENV_OTEL_RETENTION_MAX_AGE_MINUTES: &str = "SIDESEAT_OTEL_RETENTION_MAX_AGE_MINUTES";

/// Environment variable for OTEL retention max spans
pub const ENV_OTEL_RETENTION_MAX_SPANS: &str = "SIDESEAT_OTEL_RETENTION_MAX_SPANS";

/// Default retention max spans (5 million)
pub const DEFAULT_OTEL_RETENTION_MAX_SPANS: u64 = 5_000_000;

// =============================================================================
// OpenTelemetry
// =============================================================================

/// Environment variable for OTEL gRPC enabled
pub const ENV_OTEL_GRPC_ENABLED: &str = "SIDESEAT_OTEL_GRPC_ENABLED";

/// Environment variable for OTEL gRPC port
pub const ENV_OTEL_GRPC_PORT: &str = "SIDESEAT_OTEL_GRPC_PORT";

/// Default OTEL gRPC port (standard OTLP gRPC port)
pub const DEFAULT_OTEL_GRPC_PORT: u16 = 4317;

// =============================================================================
// Request Body Limits
// =============================================================================

/// Default body limit for general API requests (1 MB)
pub const DEFAULT_BODY_LIMIT: usize = 1024 * 1024;

/// Body limit for OTLP endpoints (64 MB - multimodal AI traces with images/audio/documents)
pub const OTLP_BODY_LIMIT: usize = 64 * 1024 * 1024;

/// Body limit for auth endpoints (64 KB)
pub const AUTH_BODY_LIMIT: usize = 64 * 1024;

// =============================================================================
// Topic Names
// =============================================================================

/// Topic name for OTLP traces
pub const TOPIC_TRACES: &str = "traces";

/// Topic name for OTLP metrics
pub const TOPIC_METRICS: &str = "metrics";

/// Topic name for OTLP logs
pub const TOPIC_LOGS: &str = "logs";

// =============================================================================
// Topic Configuration
// =============================================================================

/// Environment variable for topic buffer size
pub const ENV_TOPIC_BUFFER_SIZE: &str = "SIDESEAT_TOPIC_BUFFER_SIZE";

/// Environment variable for topic channel capacity
pub const ENV_TOPIC_CHANNEL_CAPACITY: &str = "SIDESEAT_TOPIC_CHANNEL_CAPACITY";

/// Default topic buffer size (100 MB)
pub const DEFAULT_TOPIC_BUFFER_SIZE: usize = 100 * 1024 * 1024;

/// Default topic channel capacity (message count)
pub const DEFAULT_TOPIC_CHANNEL_CAPACITY: usize = 100_000;

/// Retry-After header value for backpressure (in seconds)
pub const BACKPRESSURE_RETRY_AFTER_SECS: u64 = 1;

// =============================================================================
// Shutdown
// =============================================================================

/// Graceful shutdown timeout in seconds (5 minutes)
pub const SHUTDOWN_TIMEOUT_SECS: u64 = 300;

// =============================================================================
// Pricing
// =============================================================================

/// Pricing sync interval in seconds (4 hours)
pub const PRICING_SYNC_INTERVAL_SECS: u64 = 4 * 60 * 60;

/// Environment variable for pricing sync interval (in hours, 0 = disabled)
pub const ENV_PRICING_SYNC_HOURS: &str = "SIDESEAT_PRICING_SYNC_HOURS";

// =============================================================================
// File Storage
// =============================================================================

/// Minimum file size for extraction (1 KB) - smaller files stay inline as base64
pub const FILES_MIN_SIZE_BYTES: usize = 1024;

/// Maximum file size for extraction (50 MB)
pub const FILES_MAX_SIZE_BYTES: usize = 50 * 1024 * 1024;

/// Maximum message size after file extraction (10 MB)
/// Messages larger than this after extraction likely have base64 we missed
pub const FILES_MAX_MESSAGE_SIZE_BYTES: usize = 10 * 1024 * 1024;

/// Hash algorithm used for file content addressing
pub const FILE_HASH_ALGORITHM: &str = "blake3";

/// Default per-project storage quota (1 GB)
pub const FILES_DEFAULT_QUOTA_BYTES: u64 = 1024 * 1024 * 1024;

/// Max concurrent file finalization operations during batch processing
/// Limits parallel I/O to prevent overwhelming the storage backend
pub const FILES_MAX_CONCURRENT_FINALIZATION: usize = 128;

/// Max entries in the in-process file extraction cache (moka TinyLFU)
pub const FILE_EXTRACTION_CACHE_MAX_ENTRIES: u64 = 10_000;

/// Cache TTL for file quota storage bytes (seconds)
pub const CACHE_TTL_FILE_QUOTA: u64 = 60;

/// Threshold for streaming large file decoding (5 MB)
/// Files larger than this are decoded and hashed incrementally
pub const FILES_STREAM_THRESHOLD_BYTES: usize = 5 * 1024 * 1024;

/// Environment variable for file storage enabled
pub const ENV_FILES_ENABLED: &str = "SIDESEAT_FILES_ENABLED";

/// Environment variable for file storage backend (filesystem or s3)
pub const ENV_FILES_STORAGE: &str = "SIDESEAT_FILES_STORAGE";

/// Environment variable for file storage quota (bytes)
pub const ENV_FILES_QUOTA_BYTES: &str = "SIDESEAT_FILES_QUOTA_BYTES";

/// Default S3 key prefix for file storage
pub const FILES_DEFAULT_S3_PREFIX: &str = "sideseat/files";

// =============================================================================
// Organizations & Users
// =============================================================================

/// Default organization ID (created on first run)
pub const DEFAULT_ORG_ID: &str = "default";

/// Default user ID (created on first run)
pub const DEFAULT_USER_ID: &str = "local";

/// Default project ID (created on first run)
pub const DEFAULT_PROJECT_ID: &str = "default";

/// Organization role: viewer (read-only access)
pub const ORG_ROLE_VIEWER: &str = "viewer";

/// Organization role: member (read + write access)
pub const ORG_ROLE_MEMBER: &str = "member";

/// Organization role: admin (manage members + settings)
pub const ORG_ROLE_ADMIN: &str = "admin";

/// Organization role: owner (full control including delete)
pub const ORG_ROLE_OWNER: &str = "owner";

/// Authentication method: bootstrap (local dev/testing)
pub const AUTH_METHOD_BOOTSTRAP: &str = "bootstrap";

/// Authentication method: OAuth (Google, GitHub, etc.)
pub const AUTH_METHOD_OAUTH: &str = "oauth";

/// Authentication method: password
pub const AUTH_METHOD_PASSWORD: &str = "password";

/// Authentication method: passkey (WebAuthn)
pub const AUTH_METHOD_PASSKEY: &str = "passkey";

/// Authentication method: API key
pub const AUTH_METHOD_API_KEY: &str = "api_key";

/// Minimum organization slug length
pub const ORG_SLUG_MIN_LEN: usize = 1;

/// Maximum organization slug length
pub const ORG_SLUG_MAX_LEN: usize = 50;

/// Maximum organization name length
pub const ORG_NAME_MAX_LEN: usize = 100;

/// Reserved slugs that cannot be used for organizations
pub const RESERVED_SLUGS: &[&str] = &["default", "api", "admin", "settings", "new"];

/// Maximum number of organizations to return for a user profile
pub const MAX_USER_ORGS: u32 = 1000;

// =============================================================================
// Favorites
// =============================================================================

/// Maximum IDs per batch check request
pub const MAX_CHECK_BATCH: usize = 500;

/// Soft limit on favorites per user per project
pub const MAX_FAVORITES_PER_PROJECT: usize = 5000;

// =============================================================================
// Update Check
// =============================================================================

/// NPM registry URL for checking latest version
pub const NPM_REGISTRY_URL: &str = "https://registry.npmjs.org/sideseat/latest";

/// Update check HTTP timeout in seconds
pub const UPDATE_CHECK_TIMEOUT_SECS: u64 = 3;

/// Number of retry attempts for update check
pub const UPDATE_CHECK_RETRIES: u32 = 2;

/// Delay between retry attempts in milliseconds
pub const UPDATE_CHECK_RETRY_DELAY_MS: u64 = 500;

/// Environment variable to disable update check
pub const ENV_NO_UPDATE_CHECK: &str = "SIDESEAT_NO_UPDATE_CHECK";

// =============================================================================
// Cache
// =============================================================================

/// Environment variable for cache backend
pub const ENV_CACHE_BACKEND: &str = "SIDESEAT_CACHE_BACKEND";

/// Environment variable for cache max entries
pub const ENV_CACHE_MAX_ENTRIES: &str = "SIDESEAT_CACHE_MAX_ENTRIES";

/// Environment variable for cache eviction policy
pub const ENV_CACHE_EVICTION_POLICY: &str = "SIDESEAT_CACHE_EVICTION_POLICY";

/// Environment variable for Redis-compatible cache URL
/// Supports: redis://, rediss://, redis+sentinel://, rediss+sentinel://
pub const ENV_CACHE_REDIS_URL: &str = "SIDESEAT_CACHE_REDIS_URL";

/// Default cache max entries
pub const DEFAULT_CACHE_MAX_ENTRIES: u64 = 100_000;

/// Default Redis URL (works with Redis, Valkey, Dragonfly)
/// For Sentinel: redis+sentinel://sentinel1:26379,sentinel2:26379/master_name/db
pub const DEFAULT_CACHE_REDIS_URL: &str = "redis://127.0.0.1:6379/0";

/// Cache key version (bump on schema changes to invalidate all cached data)
pub const CACHE_KEY_VERSION: &str = "v1";

/// Cache TTL for user profile (5 min)
pub const CACHE_TTL_USER: u64 = 300;

/// Cache TTL for organization metadata (5 min)
pub const CACHE_TTL_ORG: u64 = 300;

/// Cache TTL for orgs list for user (2 min)
pub const CACHE_TTL_ORG_LIST: u64 = 120;

/// Cache TTL for project metadata (5 min)
pub const CACHE_TTL_PROJECT: u64 = 300;

/// Cache TTL for projects list (2 min)
pub const CACHE_TTL_PROJECT_LIST: u64 = 120;

/// Cache TTL for membership/permissions (1 min - critical)
pub const CACHE_TTL_MEMBERSHIP: u64 = 60;

/// Cache TTL for auth methods (10 min)
pub const CACHE_TTL_AUTH_METHOD: u64 = 600;

/// Cache TTL for aggregated stats (15 min)
pub const CACHE_TTL_STATS: u64 = 900;

/// Cache TTL for negative (not-found) results (30 sec - short)
pub const CACHE_TTL_NEGATIVE: u64 = 30;

// =============================================================================
// API Keys
// =============================================================================

/// API key prefix (identifies SideSeat project keys)
pub const API_KEY_PREFIX: &str = "pk-ss-";

/// Length of random characters in API key (after prefix)
pub const API_KEY_RANDOM_LENGTH: usize = 50;

/// Number of characters to display as prefix in UI (e.g., "pk-ss-a1b2c3")
pub const API_KEY_PREFIX_DISPLAY_LEN: usize = 12;

/// Maximum API keys allowed per organization
pub const API_KEY_MAX_PER_ORG: usize = 100;

/// Cache TTL for valid API key lookups (5 minutes)
pub const CACHE_TTL_API_KEY_VALID: u64 = 300;

/// Cache TTL for invalid/not-found API key lookups (30 seconds)
pub const CACHE_TTL_API_KEY_INVALID: u64 = 30;

/// Debounce interval for updating last_used_at (5 minutes)
pub const API_KEY_TOUCH_DEBOUNCE_SECS: u64 = 300;

/// Environment variable for API key HMAC secret (base64-encoded)
pub const ENV_API_KEY_SECRET: &str = "SIDESEAT_API_KEY_SECRET";

/// Environment variable for requiring OTEL auth
pub const ENV_OTEL_AUTH_REQUIRED: &str = "SIDESEAT_OTEL_AUTH_REQUIRED";

/// Length of API key HMAC secret in bytes (256 bits)
pub const API_KEY_SECRET_LENGTH: usize = 32;

/// Secret key name for API key HMAC secret
pub const SECRET_KEY_API_KEY: &str = "api_key_secret";

// =============================================================================
// Rate Limiting
// =============================================================================

/// Environment variable for rate limit enabled
pub const ENV_RATE_LIMIT_ENABLED: &str = "SIDESEAT_RATE_LIMIT_ENABLED";

/// Environment variable for per-IP rate limiting (disabled by default)
pub const ENV_RATE_LIMIT_PER_IP: &str = "SIDESEAT_RATE_LIMIT_PER_IP";

/// Environment variable for API rate limit (requests per minute)
pub const ENV_RATE_LIMIT_API_RPM: &str = "SIDESEAT_RATE_LIMIT_API_RPM";

/// Environment variable for ingestion rate limit (requests per minute)
pub const ENV_RATE_LIMIT_INGESTION_RPM: &str = "SIDESEAT_RATE_LIMIT_INGESTION_RPM";

/// Environment variable for auth rate limit (requests per minute)
pub const ENV_RATE_LIMIT_AUTH_RPM: &str = "SIDESEAT_RATE_LIMIT_AUTH_RPM";

/// Environment variable for files rate limit (requests per minute)
pub const ENV_RATE_LIMIT_FILES_RPM: &str = "SIDESEAT_RATE_LIMIT_FILES_RPM";

/// Environment variable for rate limit bypass header secret
pub const ENV_RATE_LIMIT_BYPASS_HEADER: &str = "SIDESEAT_RATE_LIMIT_BYPASS_HEADER";

/// Default API rate limit (requests per minute)
pub const DEFAULT_RATE_LIMIT_API_RPM: u32 = 1000;

/// Default ingestion rate limit (requests per minute)
pub const DEFAULT_RATE_LIMIT_INGESTION_RPM: u32 = 10_000;

/// Default auth rate limit (requests per minute)
pub const DEFAULT_RATE_LIMIT_AUTH_RPM: u32 = 30;

/// Default files rate limit (requests per minute)
pub const DEFAULT_RATE_LIMIT_FILES_RPM: u32 = 100;

/// Default auth failures rate limit (failures per minute per IP)
/// Limits brute force attacks by blocking IPs with excessive failed auth attempts
pub const DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM: u32 = 60;

/// Rate limit window in seconds (fixed 1-minute window)
pub const DEFAULT_RATE_LIMIT_WINDOW_SECS: u64 = 60;

// =============================================================================
// Database Backends
// =============================================================================

/// Environment variable for transactional database backend (sqlite or postgres)
pub const ENV_TRANSACTIONAL_BACKEND: &str = "SIDESEAT_TRANSACTIONAL_BACKEND";

/// Environment variable for analytics database backend (duckdb or clickhouse)
pub const ENV_ANALYTICS_BACKEND: &str = "SIDESEAT_ANALYTICS_BACKEND";

/// Environment variable for PostgreSQL connection URL
pub const ENV_POSTGRES_URL: &str = "SIDESEAT_POSTGRES_URL";

/// Environment variable for ClickHouse connection URL
pub const ENV_CLICKHOUSE_URL: &str = "SIDESEAT_CLICKHOUSE_URL";

// =============================================================================
// PostgreSQL Database
// =============================================================================

/// PostgreSQL default max connections (sized for SaaS workloads)
pub const POSTGRES_DEFAULT_MAX_CONNECTIONS: u32 = 20;

/// PostgreSQL default min connections (keep warm for low latency)
pub const POSTGRES_DEFAULT_MIN_CONNECTIONS: u32 = 2;

/// PostgreSQL default connection acquire timeout in seconds
pub const POSTGRES_DEFAULT_ACQUIRE_TIMEOUT_SECS: u64 = 30;

/// PostgreSQL idle connection timeout in seconds (release unused connections)
pub const POSTGRES_DEFAULT_IDLE_TIMEOUT_SECS: u64 = 600;

/// PostgreSQL max connection lifetime in seconds (cycle connections to prevent stale state)
pub const POSTGRES_DEFAULT_MAX_LIFETIME_SECS: u64 = 1800;

/// PostgreSQL statement timeout in seconds (prevent runaway queries, 0 = disabled)
pub const POSTGRES_DEFAULT_STATEMENT_TIMEOUT_SECS: u64 = 60;

// =============================================================================
// ClickHouse Database
// =============================================================================

/// ClickHouse default database name
pub const CLICKHOUSE_DEFAULT_DATABASE: &str = "sideseat";

/// ClickHouse default query timeout in seconds
pub const CLICKHOUSE_DEFAULT_TIMEOUT_SECS: u64 = 30;

// =============================================================================
// Query Limits
// =============================================================================

/// Maximum spans returned for trace/session span queries (memory safety)
pub const QUERY_MAX_SPANS_PER_TRACE: u32 = 10_000;

/// Maximum results for filter suggestions (models, providers, etc.)
pub const QUERY_MAX_FILTER_SUGGESTIONS: u32 = 100;

/// Maximum results for top-N stats queries (top models, providers)
pub const QUERY_MAX_TOP_STATS: u32 = 10;

/// Batch size for file cleanup operations
pub const FILE_CLEANUP_BATCH_SIZE: u32 = 1000;

// =============================================================================
// Error Message Limits
// =============================================================================

/// Maximum length for error status message header (type + message)
pub const ERROR_MESSAGE_MAX_LEN: usize = 2048;

/// Maximum length for exception stacktrace
pub const ERROR_STACKTRACE_MAX_LEN: usize = 16_384;

// =============================================================================
// MCP Server
// =============================================================================

/// Environment variable for MCP server enabled
pub const ENV_MCP_ENABLED: &str = "SIDESEAT_MCP_ENABLED";
