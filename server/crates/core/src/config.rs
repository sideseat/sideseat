use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::utils::file::expand_path;

use super::cli::CliConfig;
use super::constants::{
    APP_DOT_FOLDER, CONFIG_FILE_NAME, DEFAULT_CACHE_MAX_ENTRIES, DEFAULT_HOST,
    DEFAULT_OTEL_GRPC_PORT, DEFAULT_OTEL_RETENTION_MAX_SPANS, DEFAULT_OTEL_STAGING_REDRIVE_CAP,
    DEFAULT_PORT, DEFAULT_RATE_LIMIT_API_RPM, DEFAULT_RATE_LIMIT_AUTH_RPM,
    DEFAULT_RATE_LIMIT_FILES_RPM, DEFAULT_RATE_LIMIT_INGESTION_RPM, DEFAULT_REDPANDA_BROKERS,
    DEFAULT_REDPANDA_PARTITIONS, DEFAULT_REDPANDA_REPLICATION_FACTOR,
    DEFAULT_REDPANDA_RETENTION_MS, DEFAULT_REDPANDA_RETENTION_WARNING_MS, ENV_SECRETS_AWS_PREFIX,
    ENV_SECRETS_AWS_REGION, ENV_SECRETS_ENV_PREFIX, ENV_SECRETS_VAULT_ADDR,
    ENV_SECRETS_VAULT_MOUNT, ENV_SECRETS_VAULT_PREFIX, ENV_SECRETS_VAULT_TOKEN,
    FILES_DEFAULT_QUOTA_BYTES, FILES_DEFAULT_S3_PREFIX, POSTGRES_DEFAULT_ACQUIRE_TIMEOUT_SECS,
    POSTGRES_DEFAULT_IDLE_TIMEOUT_SECS, POSTGRES_DEFAULT_MAX_CONNECTIONS,
    POSTGRES_DEFAULT_MAX_LIFETIME_SECS, POSTGRES_DEFAULT_MIN_CONNECTIONS,
    POSTGRES_DEFAULT_STATEMENT_TIMEOUT_SECS, PRICING_SYNC_INTERVAL_SECS,
    SECRETS_DEFAULT_AWS_PREFIX, SECRETS_DEFAULT_ENV_PREFIX, SECRETS_DEFAULT_VAULT_MOUNT,
    SECRETS_DEFAULT_VAULT_PREFIX,
};

mod app;
mod file;

pub use file::*;

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
    pub staging_redrive_cap: u32,
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

/// Credentials configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct CredentialsConfig {
    /// Scan environment variables for provider API keys
    pub scan_env: bool,
}

/// Redis cache configuration (final/runtime)
#[derive(Debug, Clone)]
pub struct RedisConfig {
    /// Connection URL for Redis-compatible backends
    pub url: String,
    /// How many replicas must acknowledge a queued trace before the export is answered.
    ///
    /// `appendfsync always` makes an acknowledged entry survive the loss of *that host*. It says nothing
    /// about a failover: a replica promoted before it received the entry serves a keyspace without it, and
    /// the exporter has long since moved on. `WAIT` is what closes that, at the cost of a round trip to
    /// each replica per publish.
    ///
    /// Zero (the default) means a single-instance Redis, where there is nothing to fail over to. Startup
    /// logs a warning when the server *has* replicas and this is still zero, because that is the
    /// configuration where the gap exists and is invisible.
    pub min_replica_acks: u32,
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

/// RedPanda queue configuration (final/runtime).
#[derive(Debug, Clone)]
pub struct RedpandaConfig {
    pub brokers: String,
    pub partitions: i32,
    pub replication_factor: i32,
    pub retention_ms: u64,
    pub retention_warning_ms: u64,
}

/// Queue configuration, deliberately independent from [`CacheConfig`].
#[derive(Debug, Clone)]
pub struct QueueConfig {
    pub backend: QueueBackendType,
    pub redis_url: Option<String>,
    pub redis_min_replica_acks: u32,
    pub redpanda: Option<RedpandaConfig>,
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
    /// Addresses or CIDR blocks whose forwarded-for header may be believed.
    ///
    /// Empty by default, which means only the immediate peer is ever attributed. That is what makes an
    /// IP-keyed limiter safe in both deployments, and neither alternative is: trusting a forwarded header
    /// unconditionally lets a direct attacker rotate it and never exhaust a bucket, while attributing
    /// everything to the peer lets one attacker behind a proxy exhaust the bucket every other client shares -
    /// an unauthenticated denial of service. Whether the header can be believed is a property of the
    /// deployment, so only the deployment can say. See `utils::client_ip`.
    pub trusted_proxies: Vec<String>,
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
    /// How many replicas of a shard must confirm an insert before it is reported stored.
    ///
    /// `insert_distributed_sync = 1` makes the insert reach *a* shard rather than a spool file on the
    /// initiating node. It says nothing about that shard's replicas: the node holding the rows can fail
    /// before replication, and the rows go with it - after the exporter was told 200. `insert_quorum` is
    /// ClickHouse's answer, and `2` is the smallest value that survives losing one replica.
    ///
    /// Zero (the default) means an unreplicated table, where there is nothing to lose it to.
    pub insert_quorum: u32,
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
    /// Queue backend, independently selectable from the cache.
    pub queue: QueueBackendType,
    /// PostgreSQL-specific configuration (only used if transactional = postgres)
    pub postgres: Option<PostgresConfig>,
    /// ClickHouse-specific configuration (only used if analytics = clickhouse)
    pub clickhouse: Option<ClickhouseConfig>,
    /// Redis cache configuration (only used if cache = redis)
    pub redis: Option<RedisConfig>,
    /// RedPanda queue configuration (only used if queue = redpanda).
    pub redpanda: Option<RedpandaConfig>,
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

    /// Build queue configuration independently from the cache selection.
    pub fn queue_config(&self) -> QueueConfig {
        QueueConfig {
            backend: self.queue,
            redis_url: self.redis.as_ref().map(|r| r.url.clone()),
            redis_min_replica_acks: self.redis.as_ref().map_or(0, |r| r.min_replica_acks),
            redpanda: self.redpanda.clone(),
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
    pub credentials: CredentialsConfig,
    pub database: DatabaseConfig,
    pub secrets: SecretsConfig,
    pub debug: bool,
}

/// Get the profile config path (~/.sideseat/sideseat.json)
fn get_profile_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(APP_DOT_FOLDER).join(CONFIG_FILE_NAME))
}

/// Check if host binds to all network interfaces
pub fn is_all_interfaces(host: &str) -> bool {
    matches!(host, "0.0.0.0" | "::" | "[::]")
}

/// Whether a store is visible to every instance of the server, or only to the one that wrote to it.
///
/// # Why one word for three unrelated subsystems
///
/// SideSeat keeps a datum's *bytes* and the *record naming them* in different stores, and three times
/// over: a file's metadata is a row while its content is on disk or in S3; an API key's row holds
/// `HMAC(key, pepper)` while the pepper lives in a secrets backend; a session is a row while the key that
/// signs its token lives there too. Each pair works if - and only if - the store holding the bytes is at
/// least as reachable as the store holding the record.
///
/// That single rule explains three otherwise unrelated failures, each silent:
///
/// * PostgreSQL with filesystem storage: replica A writes the bytes to its own disk and the row to the
///   shared database, so B finds the row, cannot serve the content, and cannot clean it up either. A
///   restart onto a fresh host loses it outright, with the row still promising it.
/// * PostgreSQL with a keychain or file secrets backend: an API key created on A hashes under A's pepper.
///   B looks the key up *by hash*, finds nothing, and answers a plain 401 - indistinguishable from a
///   forged key - so authenticated ingestion fails on whichever replica the balancer happened to pick.
/// * The same for the JWT key, where the symptom is a browser being signed out at random.
///
/// So it is checked once, as a comparison, rather than three times as special cases - and stated as a
/// property of each backend, so a new backend has to declare which kind it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sharing {
    /// Every instance sees the same contents.
    Shared,
    /// Only the instance that wrote it, and only until its disk goes away.
    PerInstance,
}

impl TransactionalBackend {
    pub fn sharing(&self) -> Sharing {
        match self {
            // A file on the instance's own disk.
            Self::Sqlite => Sharing::PerInstance,
            Self::Postgres => Sharing::Shared,
        }
    }
}

impl StorageBackend {
    pub fn sharing(&self) -> Sharing {
        match self {
            Self::Filesystem => Sharing::PerInstance,
            Self::S3 => Sharing::Shared,
        }
    }
}

impl AnalyticsBackend {
    pub fn sharing(&self) -> Sharing {
        match self {
            // An embedded file on the instance's own disk, and DuckDB holds a process-wide lock on it
            // while running - so two replicas cannot share one file even if they could see it.
            Self::Duckdb => Sharing::PerInstance,
            Self::Clickhouse => Sharing::Shared,
        }
    }
}

impl SecretsBackend {
    pub fn sharing(&self) -> Sharing {
        match self {
            // An OS credential store, or a file beside the database. Even mounted on shared storage the
            // file backend has no compare-and-set, so two instances provisioning at once is last-writer
            // -wins - which is the same loss by a different route.
            Self::Keychain
            | Self::CredentialManager
            | Self::SecretService
            | Self::Keyutils
            | Self::File => Sharing::PerInstance,
            // Read from the environment, so whatever provisions the instances decides; the point is that
            // it *can* be the same value everywhere, which a generated keychain entry cannot.
            Self::Env => Sharing::Shared,
            Self::Aws | Self::Vault => Sharing::Shared,
        }
    }
}

/// A store holding bytes must be at least as reachable as the rows that name them.
///
/// The transactional store is the reference because that is where the naming rows live. When it is
/// per-instance the whole deployment is one instance by construction and everything matches; when it is
/// shared, anything holding bytes it points at has to be shared too.
///
/// Refused at startup rather than warned about, because every symptom is silent and looks like something
/// else: a missing attachment reads as a producer that never sent one, and a rejected API key reads as a
/// bad key. Both send the operator looking in the wrong place.
fn validate_store_sharing(
    transactional: TransactionalBackend,
    analytics: AnalyticsBackend,
    storage: StorageBackend,
    secrets: SecretsBackend,
    auth_enabled: bool,
) -> Result<()> {
    if transactional.sharing() == Sharing::PerInstance {
        return Ok(());
    }

    if analytics.sharing() == Sharing::PerInstance {
        anyhow::bail!(
            "Configuration error: database.transactional is '{transactional}', which every instance \
             shares, but database.analytics is '{analytics}', which is a file each instance holds \
             separately. Telemetry would be partitioned across replicas - a trace ingested on one is \
             absent from the others' reads, and its files are visible everywhere via the shared \
             transactional store. Set database.analytics to 'clickhouse', or database.transactional to \
             'sqlite' for a single-instance deployment."
        );
    }

    if storage.sharing() == Sharing::PerInstance {
        anyhow::bail!(
            "Configuration error: database.transactional is '{transactional}', which every instance \
             shares, but files.storage is '{storage}', which is local to one instance. A file's metadata \
             would be visible everywhere while its content existed on a single machine's disk - so \
             another instance finds the row, cannot serve the content and cannot clean it up, and \
             replacing that instance loses the content with the row still promising it. Set \
             files.storage to 's3', or database.transactional to 'sqlite' for a single-instance \
             deployment."
        );
    }

    // Only when auth is on: with `--no-auth` there are no API keys and no sessions, so nothing depends
    // on a pepper being the same everywhere.
    if auth_enabled && secrets.sharing() == Sharing::PerInstance {
        anyhow::bail!(
            "Configuration error: database.transactional is '{transactional}', which every instance \
             shares, but secrets.backend is '{secrets}', which is local to one instance. An API key row \
             holds HMAC(key, secret) and is looked up by that hash, so a key created on one instance is \
             not merely unknown on another - it is unverifiable there, and the answer is an ordinary 401. \
             Authenticated ingestion would fail depending on which instance a request reached. Set \
             secrets.backend to 'env', 'aws' or 'vault' so every instance reads the same secret."
        );
    }

    Ok(())
}

/// Check a ClickHouse configuration for combinations that cannot work.
///
/// Extracted so it can be tested: the surrounding `validate` runs inside `AppConfig::load`, which
/// reads the real config files and environment. This rule in particular was unreachable for a
/// while - `distributed` was being folded to false when no cluster was named, before validation
/// saw it - so a deployment meant to be sharded came up single-node in silence.
fn validate_clickhouse(ch: &ClickhouseConfig) -> Result<()> {
    if ch.url.is_empty() {
        anyhow::bail!(
            "Configuration error: database.clickhouse.url is required when database.analytics is 'clickhouse'. \
             Set via SIDESEAT_CLICKHOUSE_URL env var or database.clickhouse.url in config file."
        );
    }
    if ch.distributed && ch.cluster.as_ref().is_none_or(|c| c.is_empty()) {
        anyhow::bail!(
            "Configuration error: database.clickhouse.cluster is required when database.clickhouse.distributed is true. \
             Specify the ClickHouse cluster name for distributed table creation."
        );
    }
    // Fire-and-forget insertion contradicts what a 200 means here.
    //
    // With `async_insert` on and `wait_for_async_insert` off, `INSERT` returns as soon as ClickHouse has
    // buffered the rows in memory. The write path treats that return as durability: an HTTP export is
    // answered 200 and a Redis stream message is acknowledged, both on the strength of a buffer that a
    // restart discards. The whole point of acknowledging only what is durable is lost, and nothing
    // downstream can tell - the rows simply are not there later.
    //
    // Refused at startup rather than warned about, because a configuration that silently loses accepted
    // data is not a performance trade an operator can make knowingly through one boolean. The measured
    // cost of getting it right is in CLAUDE.md: waiting is *faster* here than the async path anyway.
    // A quorum of one is not a quorum; it is the default with extra latency and a false sense of safety.
    if ch.insert_quorum == 1 {
        anyhow::bail!(
            "Configuration error: database.clickhouse.insert_quorum of 1 means the initiating replica \
             alone, which is what happens with no quorum at all. Use 0 for an unreplicated table, or at \
             least 2 so an insert survives losing one replica."
        );
    }
    if ch.async_insert && !ch.wait_for_async_insert {
        anyhow::bail!(
            "Configuration error: database.clickhouse.wait_for_async_insert must be true when \
             async_insert is true. With both set this way an INSERT returns once ClickHouse has \
             buffered the rows in memory, and SideSeat answers the exporter 200 - and acknowledges the \
             ingestion queue - for data a restart would discard. Set wait_for_async_insert to true, or \
             async_insert to false."
        );
    }
    Ok(())
}

#[cfg(test)]
mod config_surface_tests {
    /// Every field of a file-config section is carried by `merge` and described by the JSON schema.
    ///
    /// Two silent failures, one test. A field added to a `*FileConfig` struct but forgotten in `merge` is
    /// *dead*: the operator sets it, the file parses, and the value never reaches the runtime config - which
    /// is what happened to `insert_quorum` and `min_replica_acks`, so an entire replication-acknowledgement
    /// feature was inert. A field missing from the schema is worse than dead: `additionalProperties: false`
    /// makes the whole config file *invalid*, so setting it correctly is what breaks startup.
    ///
    /// Checked by reading this file's own source and the schema rather than by reflection, because neither
    /// `merge` nor serde exposes the field list at runtime. Structural, and it fails on the next addition
    /// rather than at a user's first attempt to use it.
    #[test]
    fn every_file_config_field_is_merged_and_in_the_schema() {
        const SCHEMA: &str = include_str!("../../../../config/sideseat.schema.json");

        fn append_module_sources(directory: &std::path::Path, source: &mut String) {
            let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(directory)
                .expect("config module directory")
                .map(|entry| entry.expect("config module entry").path())
                .collect();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    append_module_sources(&path, source);
                } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                    source.push('\n');
                    source.push_str(
                        &std::fs::read_to_string(&path).expect("config module source file"),
                    );
                }
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/config.rs");
        let mut source = std::fs::read_to_string(&root).expect("config module root");
        append_module_sources(&root.with_extension(""), &mut source);

        /// The field names a `pub struct <name> {` block declares.
        fn fields_of(source: &str, struct_name: &str) -> Vec<String> {
            let start = source
                .find(&format!("pub struct {struct_name} {{"))
                .unwrap_or_else(|| panic!("{struct_name} not found"));
            let body_start = source[start..].find('{').expect("brace") + start + 1;
            let body_end = source[body_start..].find("\n}").expect("close") + body_start;
            source[body_start..body_end]
                .lines()
                .map(str::trim)
                .filter(|line| {
                    !line.is_empty() && !line.starts_with("//") && !line.starts_with("#[")
                })
                .filter_map(|line| line.strip_prefix("pub "))
                .filter_map(|line| line.split(':').next())
                .map(str::to_string)
                .collect()
        }

        // Each section: its file-config struct, the `merge` marker that proves it is carried, and the JSON
        // path the schema describes it under.
        for (struct_name, merge_prefix, schema_parent) in [
            ("ClickhouseFileConfig", "database.clickhouse", "clickhouse"),
            ("RedisFileConfig", "database.redis", "redis"),
            ("PostgresFileConfig", "database.postgres", "postgres"),
        ] {
            for field in fields_of(&source, struct_name) {
                // `merge` logs each field it carries with a `Merging <section>.<field>` trace, which makes
                // the carrying observable without reflection - and a field carried without that line is one
                // an operator cannot see being applied either.
                let marker = format!("\"Merging {merge_prefix}.{field}\"");
                assert!(
                    source.contains(&marker),
                    "{struct_name}.{field} is not carried by `merge`: an operator could set it and it would \
                     never reach the runtime config. Add the branch, with its {marker} trace."
                );
                // And the schema has to describe it, or `additionalProperties: false` rejects the file.
                let quoted = format!("\"{field}\"");
                let section = SCHEMA
                    .find(&format!("\"{schema_parent}\""))
                    .map(|i| &SCHEMA[i..])
                    .unwrap_or(SCHEMA);
                assert!(
                    section.contains(&quoted),
                    "{struct_name}.{field} is missing from sideseat.schema.json under `{schema_parent}`, so \
                     a config file that sets it is rejected outright"
                );
            }
        }

        // Walk every `*FileConfig` from the root and verify each field at its exact schema path. Path-aware
        // lookup avoids false matches with JSON Schema keywords or identically named fields elsewhere.
        let schema: serde_json::Value = serde_json::from_str(SCHEMA).expect("the schema is JSON");
        // struct name -> [(field, type)]
        let mut declared: std::collections::BTreeMap<String, Vec<(String, String)>> =
            std::collections::BTreeMap::new();
        for (at, _) in source.match_indices("pub struct ") {
            let name: String = source[at + "pub struct ".len()..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.ends_with("FileConfig") {
                continue;
            }
            let body_start = source[at..].find('{').expect("brace") + at + 1;
            let body_end = source[body_start..].find("\n}").expect("close") + body_start;
            let mut fields = Vec::new();
            let mut flatten_next = false;
            for line in source[body_start..body_end].lines() {
                let line = line.trim();
                if line.contains("serde(flatten)") {
                    flatten_next = true;
                    continue;
                }
                let Some(rest) = line.strip_prefix("pub ") else {
                    continue;
                };
                let Some((field, ty)) = rest.split_once(':') else {
                    continue;
                };
                // A flattened field is where unknown keys go, not a key of its own.
                if std::mem::take(&mut flatten_next) {
                    continue;
                }
                fields.push((
                    field.trim().to_string(),
                    ty.trim().trim_end_matches(',').to_string(),
                ));
            }
            declared.insert(name, fields);
        }

        let mut missing: Vec<String> = Vec::new();
        let mut unmerged: Vec<String> = Vec::new();
        let mut walk: Vec<(String, Vec<String>)> = vec![("FileConfig".to_string(), Vec::new())];
        let mut checked = 0usize;
        let structs = declared.len();
        while let Some((name, path)) = walk.pop() {
            let Some(fields) = declared.get(&name) else {
                continue;
            };
            for (field, ty) in fields {
                let mut here = path.clone();
                here.push(field.clone());
                // A nested section: recurse, and check the section itself exists on the way. The type's own
                // identifier, not a substring search - `Option<SecretsFileConfig>` contains `FileConfig`, so
                // a `contains` match resolved every section to the *root* struct and walked in circles.
                let inner = ty
                    .trim_start_matches("Option<")
                    .split(['<', '>', ',', ' '])
                    .find(|part| part.ends_with("FileConfig"))
                    .unwrap_or_default();
                let nested = declared.get_key_value(inner).map(|(name, _)| name);
                checked += 1;
                let mut node = &schema;
                let mut resolved = true;
                for segment in &here {
                    match node.get("properties").and_then(|p| p.get(segment)) {
                        Some(next) => node = next,
                        None => {
                            resolved = false;
                            break;
                        }
                    }
                }
                if !resolved {
                    missing.push(format!("{name}.{field} -> {}", here.join(".")));
                    continue;
                }
                if let Some(nested) = nested {
                    walk.push((nested.clone(), here));
                    continue;
                }
                // Every leaf must be carried by `merge` as well as described by the schema. The merge trace
                // makes that observable to operators and gives this structural test one source of truth.
                let marker = format!("\"Merging {}\"", here.join("."));
                if !source.contains(&marker) {
                    unmerged.push(format!("{name}.{field} -> {}", here.join(".")));
                }
            }
        }
        assert!(
            unmerged.is_empty(),
            "{} config field(s) `merge` does not announce with a `Merging <path>` trace. From outside, a field \
             that is carried silently and one that is not carried at all look identical. Add the trace, or \
             the branch and the trace:\n  {}",
            unmerged.len(),
            unmerged.join("\n  ")
        );
        assert!(
            missing.is_empty(),
            "{} config field(s) that sideseat.schema.json does not describe at the path they are read from, \
             so a config file setting one is rejected by `additionalProperties: false`:\n  {}",
            missing.len(),
            missing.join("\n  ")
        );
        assert!(
            structs >= 20 && checked >= 60,
            "found {structs} config structs and {checked} fields - the scan is wrong, not the schema"
        );
    }
}

#[cfg(test)]
mod clickhouse_config_tests {
    use super::*;

    fn config(distributed: bool, cluster: Option<&str>) -> ClickhouseConfig {
        ClickhouseConfig {
            url: "http://localhost:8123".to_string(),
            database: "sideseat".to_string(),
            user: None,
            password: None,
            timeout_secs: 30,
            compression: true,
            async_insert: false,
            wait_for_async_insert: true,
            cluster: cluster.map(str::to_owned),
            distributed,
            insert_quorum: 0,
        }
    }

    #[test]
    fn distributed_without_a_cluster_is_rejected() {
        let err = validate_clickhouse(&config(true, None))
            .expect_err("distributed with no cluster must not be accepted");
        assert!(
            err.to_string().contains("cluster is required"),
            "unexpected error: {err}"
        );

        let err = validate_clickhouse(&config(true, Some("")))
            .expect_err("an empty cluster name is not a cluster name");
        assert!(err.to_string().contains("cluster is required"));

        validate_clickhouse(&config(true, Some("sideseat_cluster")))
            .expect("distributed with a cluster is the supported combination");
        validate_clickhouse(&config(false, None)).expect("single node needs no cluster");
    }

    /// A 200 must not be answerable from a memory buffer.
    ///
    /// `async_insert` with `wait_for_async_insert` off returns from an INSERT once ClickHouse has the
    /// rows in RAM. The ingestion path reads that as durability - it answers the exporter and
    /// acknowledges the queue message - so a restart loses data that was reported as stored.
    #[test]
    fn fire_and_forget_insertion_is_rejected() {
        let mut ch = config(false, None);
        ch.async_insert = true;
        ch.wait_for_async_insert = false;
        let err = validate_clickhouse(&ch).expect_err(
            "acknowledging data held only in a server-side buffer must not be a setting",
        );
        assert!(
            err.to_string()
                .contains("wait_for_async_insert must be true"),
            "unexpected error: {err}"
        );

        ch.wait_for_async_insert = true;
        validate_clickhouse(&ch).expect("async batching that waits for the flush is durable");

        // The default: no async batching at all, so the wait flag decides nothing.
        ch.async_insert = false;
        ch.wait_for_async_insert = false;
        validate_clickhouse(&ch)
            .expect("a synchronous insert is durable whatever the async wait flag says");
    }

    /// A shared database with per-instance byte storage is refused, and matched pairs are accepted.
    ///
    /// Three silent failures share this root cause, so one comparison catches all three: PostgreSQL plus
    /// filesystem storage means a row every instance can see naming content only one machine holds;
    /// PostgreSQL plus a keychain means an API key that verifies on one instance and reads as forged on
    /// the next; the same for the JWT key, where a browser is signed out at random.
    #[test]
    fn byte_storage_must_be_at_least_as_shared_as_the_rows_naming_it() {
        use AnalyticsBackend as An;
        use SecretsBackend as Sec;
        use StorageBackend as Store;
        use TransactionalBackend as Tx;

        // The self-consistent single-instance default: everything per-instance, nothing to complain about.
        validate_store_sharing(
            Tx::Sqlite,
            An::Duckdb,
            Store::Filesystem,
            Sec::Keychain,
            true,
        )
        .expect("SQLite with local files and a local keychain is one instance by construction");

        // A shared database is the signal, because that is where the naming rows live.
        let err = validate_store_sharing(
            Tx::Postgres,
            An::Clickhouse,
            Store::Filesystem,
            Sec::Aws,
            true,
        )
        .expect_err("a shared database naming local files must be refused");
        assert!(
            err.to_string().contains("files.storage"),
            "unexpected error: {err}"
        );

        let err =
            validate_store_sharing(Tx::Postgres, An::Clickhouse, Store::S3, Sec::Keychain, true)
                .expect_err("a shared database with a per-instance pepper must be refused");
        assert!(
            err.to_string().contains("secrets.backend"),
            "unexpected error: {err}"
        );
        for local in [
            Sec::File,
            Sec::CredentialManager,
            Sec::SecretService,
            Sec::Keyutils,
        ] {
            validate_store_sharing(Tx::Postgres, An::Clickhouse, Store::S3, local, true)
                .expect_err("every per-instance secrets backend is refused, not just the keychain");
        }

        // With auth off there are no API keys and no sessions, so nothing reads the pepper.
        validate_store_sharing(
            Tx::Postgres,
            An::Clickhouse,
            Store::S3,
            Sec::Keychain,
            false,
        )
        .expect("a per-instance secret matters only when something is authenticated with it");

        // Analytics is byte storage too, and DuckDB is a per-instance file.
        let err = validate_store_sharing(Tx::Postgres, An::Duckdb, Store::S3, Sec::Aws, true)
            .expect_err("PostgreSQL + DuckDB partitions telemetry across replicas");
        assert!(
            err.to_string().contains("database.analytics"),
            "unexpected error: {err}"
        );

        // And the combinations that hold up.
        for shared in [Sec::Env, Sec::Aws, Sec::Vault] {
            validate_store_sharing(Tx::Postgres, An::Clickhouse, Store::S3, shared, true)
                .expect("shared rows, shared bytes, shared secret");
        }
    }

    /// A quorum of one is refused, because it is no quorum with extra latency.
    ///
    /// `insert_quorum = 1` is satisfied by the initiating replica alone - exactly what happens with no
    /// quorum - so accepting it would let an operator believe an insert survives losing a replica when it
    /// does not. Zero says "unreplicated"; two is the smallest number that means anything.
    #[test]
    fn an_insert_quorum_of_one_is_rejected() {
        let mut ch = config(false, None);
        ch.insert_quorum = 1;
        let err = validate_clickhouse(&ch).expect_err("a quorum of one must not be accepted");
        assert!(
            err.to_string().contains("insert_quorum of 1"),
            "unexpected error: {err}"
        );

        ch.insert_quorum = 0;
        validate_clickhouse(&ch).expect("zero means an unreplicated table");
        ch.insert_quorum = 2;
        validate_clickhouse(&ch).expect("two survives losing one replica");
    }

    #[test]
    fn an_empty_url_is_rejected() {
        let mut ch = config(false, None);
        ch.url = String::new();
        let err = validate_clickhouse(&ch).expect_err("no url means nothing to connect to");
        assert!(err.to_string().contains("url is required"));
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
