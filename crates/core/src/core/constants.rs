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

/// How long to wait for the OS credential store before giving up at startup.
///
/// A keychain read can block on a user prompt, and where there is nobody to answer it - a
/// background process, an SSH session, CI - it blocks forever. Startup then hangs with no output
/// at all, because nothing else logs before this point.
///
/// Sized for what the prompt actually asks: macOS opens a window wanting the login *password*,
/// and it can open behind the terminal. Finding it and typing takes longer than clicking "Allow",
/// so a budget tight enough to look responsive would abort startup while someone was still
/// typing - the one case where waiting is correct. A machine with nobody at the keyboard pays
/// this once and then fails with a message that says how to avoid it (approve with Always Allow,
/// or use the file backend), which beats hanging forever.
pub const SECRETS_LOAD_TIMEOUT_SECS: u64 = 120;

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
/// Maximum failed staged-payload write/read-back cycles before the payload is
/// quarantined as unconfirmed. The bytes remain held for operator recovery.
pub const DEFAULT_OTEL_STAGING_REDRIVE_CAP: u32 = 5;

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
// Rule engine work ceilings
// =============================================================================

/// How many nodes one rule's bounded walk may visit on one span.
///
/// **Server policy, not a declaration.** A rule states how *deep* to descend, which is semantics - the shape
/// of the state object a framework writes. How much work that may cost against an adversarial payload is a
/// property of this server, and a limit an asset could raise would not be a limit.
///
/// The declared depth alone does not bound the work: a payload nests as widely as it likes within it, and every
/// node is evaluated. The 64 MiB OTLP body limit is not a useful bound either, since the cost is in the
/// evaluation rather than the bytes.
pub const RULE_WALK_MAX_NODES: usize = 4_096;

/// How many observations one rule may produce from one span's carrier.
///
/// A rule reading an array emits one observation per element, so a payload holding a hundred thousand elements
/// is a hundred thousand messages from one span - which no producer means and no reader can use.
pub const RULE_MAX_EMISSIONS_PER_CARRIER: usize = 8_192;

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

/// Environment variable for the durable queue backend.
pub const ENV_QUEUE_BACKEND: &str = "SIDESEAT_QUEUE_BACKEND";

/// Environment variable for RedPanda/Kafka bootstrap brokers.
pub const ENV_REDPANDA_BROKERS: &str = "SIDESEAT_REDPANDA_BROKERS";

/// Default RedPanda broker address for server-mode deployments.
pub const DEFAULT_REDPANDA_BROKERS: &str = "127.0.0.1:9092";

/// Default partitions for each queue topic.
pub const DEFAULT_REDPANDA_PARTITIONS: i32 = 6;

/// Default replication factor. Local and test deployments are single-node.
pub const DEFAULT_REDPANDA_REPLICATION_FACTOR: i32 = 1;

/// Default queue retention: seven days.
pub const DEFAULT_REDPANDA_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

/// Warn when the oldest uncommitted record is within an hour of retention.
pub const DEFAULT_REDPANDA_RETENTION_WARNING_MS: u64 = 60 * 60 * 1_000;

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

/// Default unified per-project storage quota across analytics, transactional staging/journal and blobs (1 GB).
pub const FILES_DEFAULT_QUOTA_BYTES: u64 = 1024 * 1024 * 1024;

/// Max concurrent file finalization operations during batch processing
/// Limits parallel I/O to prevent overwhelming the storage backend
pub const FILES_MAX_CONCURRENT_FINALIZATION: usize = 128;

/// Max entries in the in-process file extraction cache (moka TinyLFU)
pub const FILE_EXTRACTION_CACHE_MAX_ENTRIES: u64 = 10_000;

/// Idle TTL for file extraction cache entries (seconds).
/// Entries expire after this duration without access, allowing retry of
/// failed finalizations within the same server session.
pub const FILE_EXTRACTION_CACHE_IDLE_SECS: u64 = 300;

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

/// Environment variable for S3 bucket name
pub const ENV_FILES_S3_BUCKET: &str = "SIDESEAT_FILES_S3_BUCKET";

/// Environment variable for S3 key prefix
pub const ENV_FILES_S3_PREFIX: &str = "SIDESEAT_FILES_S3_PREFIX";

/// Environment variable for S3 region
pub const ENV_FILES_S3_REGION: &str = "SIDESEAT_FILES_S3_REGION";

/// Environment variable for S3 endpoint (for S3-compatible services)
pub const ENV_FILES_S3_ENDPOINT: &str = "SIDESEAT_FILES_S3_ENDPOINT";

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

/// How old a file deletion claim must be before a sweep treats it as abandoned and finishes it.
///
/// A claim fences ingestion off a file being deleted, and it is durable so that it survives a restart -
/// which means a crash mid-deletion leaves one behind. Long enough that a slow deletion in progress is
/// never mistaken for an abandoned one; short enough that a stuck file is not stuck for a day.
pub const FILE_DELETION_CLAIM_STALE_SECS: i64 = 900;

/// How old a project deletion claim must be before startup treats it as abandoned and finishes it.
///
/// Shorter than the file threshold has to be long, and for the opposite reason: a claimed project is
/// invisible to every read path, so a stuck one looks to its owner exactly like data loss. Startup is
/// also the moment a crashed process's claims become findable, and nothing else can release them.
pub const PROJECT_DELETION_CLAIM_STALE_SECS: i64 = 60;

/// How often the background sweep looks for claims a crash or a failed step abandoned.
///
/// Startup sweeps once, which cannot be enough on its own: a claim taken a second before the crash is
/// still *fresh* when the process comes back, so the sweep that runs then correctly leaves it alone and
/// nothing looks again. Frequent enough that a stuck project is hidden for minutes rather than for the
/// process's lifetime; the sweep is two indexed queries when there is nothing to do.
pub const CLAIM_RECOVERY_INTERVAL_SECS: u64 = 120;

/// Consecutive cleanup sweeps that must find no data before a deleted project's row is removed.
///
/// The row is a tombstone: while it exists no new write is accepted for the project and every sweep
/// deletes whatever appeared, so a writer that read the fence before the tombstone has its spans
/// collected rather than stranded. Removal follows *observation* for that reason - elapsed time says
/// nothing about a writer that is stalled rather than gone.
///
/// Five sweeps at `CLAIM_RECOVERY_INTERVAL_SECS` is ten minutes of continuously verified-empty, and a
/// sweep that finds anything starts the count again.
pub const PROJECT_TOMBSTONE_CLEAN_SWEEPS: i64 = 5;

/// Discovery policy for deleted projects, whose records are kept forever.
///
/// Kept forever because any retention is a bound on how late a stalled writer may commit and still be
/// collected. That makes the *rate* the thing to bound instead: a first check soon after the deletion, then
/// each quiet check pushing the next one out geometrically to a daily floor, and at most a fixed number of
/// ids per sweep so one pass cannot outlive its own window. Without the backoff, a hundred thousand
/// historical deletions meant a hundred thousand storage listings every sweep, forever.
pub const DELETED_PROJECT_CHECK_BASE_SECS: i64 = 60;
pub const DELETED_PROJECT_CHECK_MAX_SECS: i64 = 24 * 60 * 60;
pub const DELETED_PROJECT_CHECK_BATCH: i64 = 50;

/// How many abandoned project or organization cleanups one sweep resumes, and how long taking one leases it.
///
/// Both are needed and for different reasons. **Uncapped**, a backlog of stale projects ran ahead of the
/// leased trace and session sweeps in the same pass - each project's cleanup touches four stores - so a stuck
/// association from a crashed writer could go unreclaimed indefinitely while the sweep worked through
/// projects. **Unleased**, every replica resumed every one of them, duplicating all of that storage work
/// rather than sharing it.
///
/// The lease is the tombstone's own timestamp, pushed forward: `deleting_at` is both the fence and the age
/// that makes a claim look abandoned, so refreshing it re-leases the work *without* lifting the fence, which
/// must stay set until the cleanup finishes. A crashed resumer's project simply looks abandoned again once
/// the window passes.
pub const STALE_CLEANUP_RESUME_BATCH: i64 = 20;

/// How long a claimed deleted-project check is leased for before another sweep may take it.
///
/// A batch of fifty storage listings has no guaranteed duration, and without a lease a sweep that outran
/// its interval would have its ids claimed again by the next one - two replicas doing the same S3 work.
/// Comfortably longer than a batch should take, and short enough that a crashed sweep's ids come back soon.
pub const DELETED_PROJECT_CHECK_LEASE_SECS: i64 = 600;

/// The same schedule for deleted *trace* records, which are far more numerous than project ones - hence a
/// larger batch and a shorter base interval, so a fresh deletion is re-checked promptly while an old one
/// backs off to the daily floor. What is being collected is narrow: spans written by a batch that passed
/// the pre-write tombstone check and then crashed before its own compensating re-check.
pub const DELETED_TRACE_CHECK_BASE_SECS: i64 = 30;
pub const DELETED_TRACE_CHECK_MAX_SECS: i64 = 24 * 60 * 60;
pub const DELETED_TRACE_CHECK_BATCH: i64 = 200;
pub const DELETED_TRACE_CHECK_LEASE_SECS: i64 = 300;

/// How much memory the reconstruction cache may hold, and how long an unused entry stays.
///
/// **Bytes, not entries**, and the entry count this replaced was unbounded in the dimension that matters.
/// The argument for a count was that a reconstruction's *output* is the blocks a reader sees rather than
/// the megabytes of re-sent history that produced them - true on average and false in the worst case: an
/// incremental session's output grows with its turns, so a 10 000-turn read is tens of thousands of blocks
/// carrying full tool payloads, and 512 of those is however many gigabytes the largest sessions happen to
/// be. A cache that cannot state its own ceiling is not compatible with a footprint ceiling.
///
/// A per-entry floor is folded into the weight ([`RECONSTRUCTION_CACHE_ENTRY_OVERHEAD_BYTES`]), so this
/// also bounds the entry count - the two questions have one answer instead of two knobs that can disagree.
///
/// The idle window exists because a session nobody opens again should not hold memory, not because an entry
/// can go stale: the key is a hash of the rows, so a changed row is a different key.
pub const RECONSTRUCTION_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const RECONSTRUCTION_CACHE_IDLE_SECS: u64 = 900;

/// Charged per cached block on top of its serialised size.
///
/// The weight is measured by serialising the answer, which counts the content and not the `BlockEntry`
/// structs, their `Vec` slots, their `span_path` allocations or the fields marked `#[serde(skip)]`. Those
/// are real bytes and a weigher that ignores them understates the cache by a factor that grows with how
/// many small blocks an answer holds - so each block is charged a flat amount as well.
pub const RECONSTRUCTION_CACHE_ENTRY_OVERHEAD_BYTES: u64 = 512;

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

// =============================================================================
// Credentials
// =============================================================================

/// Environment variable to scan env vars for provider API keys (default: true)
pub const ENV_CREDENTIALS_SCAN_ENV: &str = "SIDESEAT_CREDENTIALS_SCAN_ENV";

/// Secret key prefix for per-org credential secrets
pub const CRED_SECRET_PREFIX: &str = "cred_";

/// Timeout in seconds for credential test-connection operations
pub const CRED_TEST_TIMEOUT_SECS: u64 = 10;

/// Cache TTL for credential secrets (5 minutes)
pub const CACHE_TTL_CRED_SECRET: u64 = 300;

/// Cache TTL for credential list (60 seconds)
pub const CACHE_TTL_CRED_LIST: u64 = 60;

/// Maximum entries in the process-local cache (never Redis).
///
/// This cache holds sensitive data like credential secrets that must never
/// leave the process. Sized to hold secrets for many orgs comfortably.
pub const LOCAL_CACHE_MAX_ENTRIES: u64 = 10_000;

// =============================================================================
// SDK WebSocket Protocol (registration + introspection)
// =============================================================================

/// Maximum WebSocket message size for the SDK channel (4 MiB).
pub const WS_MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// Server-initiated heartbeat ping interval.
pub const WS_HEARTBEAT_INTERVAL_SECS: u64 = 20;

/// Grace period after a ping before treating the connection as dead.
pub const WS_PONG_GRACE_SECS: u64 = 10;

/// Per-connection rolling rate limit applied to all client-initiated frames.
/// `agent.event` frames are exempt (they're SDK→server fan-out replies for
/// in-flight invocations, not user-initiated traffic).
pub const WS_FRAME_RATE_LIMIT_COUNT: u32 = 1_000;

/// Default time the AG-UI HTTP route waits for the SDK to respond with the
/// first `agent.event` after sending `agent.invoke` before timing out the
/// SSE with a synthesised `RUN_ERROR`.
pub const INVOKE_TIMEOUT_MS: u64 = 60_000;

/// How long a partial chunk-group sits in the reassembly buffer before
/// being discarded as orphaned. The server-side reassembler is the only
/// reader of this constant; the SDK chunks events under its own size
/// thresholds (`_AGUI_CHUNK_THRESHOLD_BYTES` in
/// `sdk/python/.../runtime/client.py`).
pub const AGUI_CHUNK_REASSEMBLY_TTL_SECS: u64 = 60;

/// Per-request memory cap on partial reassembly bytes. Prevents a buggy
/// or malicious SDK from holding gigabytes resident across stale chunks.
pub const AGUI_CHUNK_MAX_PER_REQUEST_BYTES: usize = 64 * 1024 * 1024;

/// Window for the rate limit counter.
pub const WS_FRAME_RATE_LIMIT_WINDOW_SECS: u64 = 10;

/// TTL for stored registrations after the last heartbeat.
pub const REGISTRATION_TTL_SECS: u64 = 60;

/// Time within which the SDK must send `hello` after `welcome`.
pub const WS_HELLO_TIMEOUT_SECS: u64 = 5;

// ---------------------------------------------------------------------------
// Footprint ceilings
//
// Enforced by `server/tests/footprint.rs` (the two in-process gates) and
// `scripts/footprint-gates.sh` (the two that need a running server). They live
// here, in one place, because two of the four are read from a shell script and
// a ceiling with two spellings is a ceiling that drifts;
// `the_footprint_script_enforces_the_declared_ceilings` compares the script's
// text against these values.
//
// All four are stated against the *pinned* allocator
// (`runtime/allocation.rs`). An absolute megabyte figure is only comparable
// within one allocator, so a build without it reports the numbers and skips
// the resident gates rather than passing on a figure it cannot interpret.
// ---------------------------------------------------------------------------

/// Resident bytes after startup, quiesced.
pub const FOOTPRINT_IDLE_RSS_MAX_BYTES: u64 = 100 * 1024 * 1024;

/// Resident bytes under steady ingest, taken as the median over the sampling window.
pub const FOOTPRINT_INGEST_RSS_MAX_BYTES: u64 = 400 * 1024 * 1024;

/// Spans per second the steady-ingest ceiling above is stated at.
pub const FOOTPRINT_INGEST_SPANS_PER_SECOND: u64 = 5_000;

/// How much *live allocated* memory a long session read may leave behind once its answer and its memo are
/// dropped.
///
/// Live allocations rather than RSS, deliberately: both glibc and jemalloc retain freed pages, so an RSS
/// ceiling here fails correct code and fails it differently depending on timing. See
/// `runtime::allocation` for the whole argument.
pub const FOOTPRINT_SESSION_READ_GROWTH_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// Turns in the session the ceiling above is stated against.
pub const FOOTPRINT_SESSION_READ_TURNS: usize = 10_000;

/// Live bytes a queued span may occupy, as a multiple of its decoded protobuf size.
///
/// The denominator is **decoded protobuf bytes**, not wire bytes: HTTP accepts gzip and the queue carries an
/// uncompressed encoding, so a wire-relative bound is unachievable for a valid repetitive request.
pub const FOOTPRINT_QUEUED_SPAN_MAX_RATIO: f64 = 3.0;

// ---------------------------------------------------------------------------
// In-process queue admission
//
// The bound is on *bytes*, not on entry count, and it refuses rather than
// trims. `PIPELINE_BATCH_MAX_SIZE` is a drain limit, so budgeting it bounds
// nothing that matters; what has to be bounded is what the queue holds.
// ---------------------------------------------------------------------------

/// Bytes one in-process stream topic may hold in unconsumed entries.
///
/// Sized against the 400 MB steady-ingest ceiling rather than picked: the queue is one contributor to that
/// figure, alongside the decode, the write path and DuckDB's own buffers, so it gets a fraction of it. An
/// exporter that outruns the consumer by more than this is told 503 with `Retry-After` and keeps its data,
/// which is what an OTLP exporter is built to do.
pub const STREAM_MAX_RETAINED_BYTES: u64 = 128 * 1024 * 1024;

/// Charged per entry on top of its payload, so one budget bounds the memory rather than only the payloads.
///
/// A queue of a hundred million one-byte entries costs far more than a hundred megabytes: each occupies a
/// `VecDeque` slot, a heap allocation for its payload, and a pending-map entry per consumer group. A pure
/// payload budget would admit that and the process would die inside a bound it was passing. Deliberately
/// generous, because the failure of underestimating it is an out-of-memory kill and the failure of
/// overestimating it is refusing slightly early.
pub const STREAM_ENTRY_OVERHEAD_BYTES: u64 = 256;

/// Charged per *pending record*, of which each consumer group holds one per delivered-and-unacknowledged entry.
///
/// Separate from the per-entry overhead because the two multiply. Counting only entries made the bound blind to
/// group state: ten thousand entries against a thousand abandoned groups is ten million pending records, and
/// `retained_bytes` reported the queue comfortably inside its budget while it held gigabytes. Smaller than an
/// entry's overhead because a pending record is an id, a consumer name and an instant rather than a payload.
pub const STREAM_PENDING_RECORD_OVERHEAD_BYTES: u64 = 128;

/// Consumer groups one in-process stream may have.
///
/// The other half of the pending-record bound, and the half a publish-time check cannot provide: group state
/// grows at *delivery*, which cannot refuse without stalling a consumer, so the only sound bound on it is a
/// bound on the number of groups. Charging pending records at publish stops a backlog from being admitted while
/// group state is already large; it does nothing about a single retained entry delivered to unboundedly many
/// groups.
///
/// Generous, because a legitimate deployment has one group per signal and a handful of consumers inside it: a
/// stream with dozens is a mistake in the calling code rather than a workload, and this is where that mistake
/// becomes a refused subscription with a message instead of a slow memory leak.
pub const STREAM_MAX_CONSUMER_GROUPS: usize = 32;

/// Consumer names one group remembers, for `StreamStats::consumers`.
///
/// The third place group state can grow without bound, and the one the group cap does not reach: a client that
/// reconnects with a fresh name adds an entry per reconnect, so one group with a million reconnects is a million
/// remembered names while the group count stays at one and every entry and pending record is reclaimed.
///
/// Bounded by eviction rather than by refusal, because this map is a *statistic* and not a registry - nothing
/// reads it to decide anything, and refusing a subscription because a stat is full would trade a real capability
/// for a number. The least recently active name goes, which is the one a "how many consumers are on this group"
/// answer cares about least.
pub const STREAM_MAX_REMEMBERED_CONSUMERS: usize = 64;

// ---------------------------------------------------------------------------
// The embedded engine's share of the footprint ceiling
//
// DuckDB's default `memory_limit` is 80% of physical RAM - on a 64 GB host that
// is 51 GB, which makes a 400 MB process ceiling a statement about everything
// except the component most likely to breach it. An embedded engine that
// ignores the budget makes the budget false.
// ---------------------------------------------------------------------------

/// Bytes DuckDB may use, as a share of [`FOOTPRINT_INGEST_RSS_MAX_BYTES`].
///
/// Half, not all of it: the rest of the process - the decode, the pipeline, the queue and the reconstruction
/// cache - has to fit inside the same ceiling, and those are the parts this repository's own benchmarks
/// measure.
///
/// **Most operators spill; not all of them do.** Hash aggregates, sorts and window functions are out-of-core,
/// and `temp_directory` is set beside this so the destination is a directory SideSeat owns. But DuckDB
/// documents complex aggregate states - `list()`, `first()` - as unable to offload, and the trace list builds
/// its tag column with `LIST_DISTINCT(FLATTEN(LIST(...)))`, so a trace with thousands of large tag arrays can
/// raise an out-of-memory error here where the default limit would have completed.
///
/// So the claim is not "a tight limit only costs latency": it can cost the query. The trade is taken because an
/// error names the limit while the default silently makes the process ceiling meaningless, and because which
/// way it should go is a measurement rather than an argument - `make bench-http` plus a large-corpus read. If
/// it proves too tight the fix is a configuration key, not a bigger constant, since the value depends on the
/// corpus.
pub const DUCKDB_MEMORY_LIMIT_BYTES: u64 = FOOTPRINT_INGEST_RSS_MAX_BYTES / 2;

/// Decoded protobuf bytes the CPU phase may have in flight at once.
///
/// The fan-out used to be sized by **thread count**: one worker per core, each expanding its own request. So
/// peak memory during the CPU phase was the number of cores times the largest request, which means a
/// 32-core host holding thirty-two 15.8 MB image-heavy exports expanded simultaneously - and the expansion is
/// several times its input, because base64 attachments are decoded and every message is parsed. A bound
/// expressed in threads is not a bound on memory, and the host decides the multiplier.
///
/// So requests are grouped into waves whose summed size stays under this, and the waves run one after another.
/// Parallelism is unchanged where payloads are small, which is the common case; it degrades to fewer
/// concurrent requests exactly where each one is large, which is where it had to.
///
/// A single request larger than this is still processed - it forms a wave of one - because refusing it here
/// would refuse a valid export that the byte-budgeted admission at the edge already accepted.
pub const PIPELINE_CPU_PHASE_MAX_INFLIGHT_BYTES: u64 = 64 * 1024 * 1024;
