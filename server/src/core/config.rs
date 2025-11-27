//! Configuration management with multi-source loading and smart merging
//!
//! Loads configuration from multiple sources with the following priority (highest to lowest):
//!
//! 1. Command line arguments
//! 2. Environment variables (`SIDESEAT_*` prefix)
//! 3. Workdir config (`./sideseat.json`)
//! 4. User config (`~/.sideseat/config.json`)
//!
//! ## Usage
//!
//! ```rust,ignore
//! use sideseat::core::{ConfigManager, CliConfig, StorageManager};
//!
//! let storage = StorageManager::init().await?;
//! let cli_config = CliConfig { host: None, port: None };
//! let config_manager = ConfigManager::init(&storage, &cli_config)?;
//! let config = config_manager.config();
//!
//! println!("Server will listen on {}:{}", config.server.host, config.server.port);
//! ```

use super::StorageManager;
use super::constants::{
    CONFIG_FILE_USER, CONFIG_FILE_WORKDIR, DEFAULT_HOST, DEFAULT_LOG_FORMAT, DEFAULT_LOG_LEVEL,
    DEFAULT_PORT, ENV_AUTH_ENABLED, ENV_HOST, ENV_PORT,
};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// CLI configuration values passed from command line arguments
#[derive(Debug, Clone, Default)]
pub struct CliConfig {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub no_auth: bool,
}

/// Information about a configuration source
#[derive(Debug, Clone)]
pub struct ConfigSource {
    /// Name of the source (e.g., "user_config", "workdir", "env", "cli")
    pub name: String,
    /// Path to the config file (if applicable)
    pub path: Option<PathBuf>,
    /// Whether this source was successfully loaded
    pub loaded: bool,
}

impl ConfigSource {
    fn new(name: &str, path: Option<PathBuf>, loaded: bool) -> Self {
        Self { name: name.to_string(), path, loaded }
    }

    fn loaded(name: &str, path: PathBuf) -> Self {
        Self::new(name, Some(path), true)
    }

    fn skipped(name: &str, path: PathBuf) -> Self {
        Self::new(name, Some(path), false)
    }
}

/// Top-level configuration structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub otel: OtelConfig,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_host() -> String {
    DEFAULT_HOST.to_string()
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { host: default_host(), port: default_port() }
    }
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_string()
}

fn default_log_format() -> String {
    DEFAULT_LOG_FORMAT.to_string()
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self { level: default_log_level(), format: default_log_format() }
    }
}

/// Storage path overrides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StorageConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<String>,
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Whether authentication is enabled (default: true)
    #[serde(default = "default_auth_enabled")]
    pub enabled: bool,
}

fn default_auth_enabled() -> bool {
    true
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self { enabled: default_auth_enabled() }
    }
}

/// OpenTelemetry collector configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelConfig {
    /// Whether OTel collector is enabled
    #[serde(default = "default_otel_enabled")]
    pub enabled: bool,

    // gRPC settings
    /// Whether gRPC endpoint is enabled
    #[serde(default = "default_grpc_enabled")]
    pub grpc_enabled: bool,
    /// gRPC port (default: 4317)
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,

    // Ingestion channel
    /// Bounded channel capacity for ingestion
    #[serde(default = "default_channel_capacity")]
    pub channel_capacity: usize,

    // WriteBuffer settings (bounded memory)
    /// Maximum spans in buffer before flush
    #[serde(default = "default_buffer_max_spans")]
    pub buffer_max_spans: usize,
    /// Maximum bytes in buffer before flush
    #[serde(default = "default_buffer_max_bytes")]
    pub buffer_max_bytes: usize,
    /// Flush interval in milliseconds
    #[serde(default = "default_flush_interval_ms")]
    pub flush_interval_ms: u64,
    /// Flush when batch reaches this size
    #[serde(default = "default_flush_batch_size")]
    pub flush_batch_size: usize,

    // Parquet settings
    /// Maximum Parquet file size in MB
    #[serde(default = "default_max_file_size_mb")]
    pub max_file_size_mb: u32,
    /// Rows per row group in Parquet files
    #[serde(default = "default_row_group_size")]
    pub row_group_size: usize,

    // Retention settings
    /// Retention days (None = disabled, uses size-based only)
    #[serde(default)]
    pub retention_days: Option<u32>,
    /// Maximum storage size in GB (FIFO deletion when exceeded)
    #[serde(default = "default_retention_max_gb")]
    pub retention_max_gb: u32,
    /// Retention check interval in seconds
    #[serde(default = "default_retention_check_interval_secs")]
    pub retention_check_interval_secs: u64,

    // Disk monitoring
    /// Disk usage percent to trigger warning
    #[serde(default = "default_disk_warning_percent")]
    pub disk_warning_percent: u8,
    /// Disk usage percent to stop ingestion
    #[serde(default = "default_disk_critical_percent")]
    pub disk_critical_percent: u8,

    // Input validation
    /// Maximum span name length
    #[serde(default = "default_max_span_name_len")]
    pub max_span_name_len: usize,
    /// Maximum attributes per span
    #[serde(default = "default_max_attribute_count")]
    pub max_attribute_count: usize,
    /// Maximum attribute value length in bytes
    #[serde(default = "default_max_attribute_value_len")]
    pub max_attribute_value_len: usize,
    /// Maximum events per span
    #[serde(default = "default_max_events_per_span")]
    pub max_events_per_span: usize,

    // SSE settings
    /// Maximum concurrent SSE connections
    #[serde(default = "default_sse_max_connections")]
    pub sse_max_connections: usize,
    /// SSE connection timeout in seconds
    #[serde(default = "default_sse_timeout_secs")]
    pub sse_timeout_secs: u64,
    /// SSE keepalive interval in seconds
    #[serde(default = "default_sse_keepalive_secs")]
    pub sse_keepalive_secs: u64,
}

// OTel defaults optimized for developer workloads
fn default_otel_enabled() -> bool {
    true
}
fn default_grpc_enabled() -> bool {
    true
}
fn default_grpc_port() -> u16 {
    4317
}
fn default_channel_capacity() -> usize {
    10000
}
fn default_buffer_max_spans() -> usize {
    5000
}
fn default_buffer_max_bytes() -> usize {
    10 * 1024 * 1024 // 10MB
}
fn default_flush_interval_ms() -> u64 {
    100 // 100ms - fast flush for low volume, batch size handles high volume
}
fn default_flush_batch_size() -> usize {
    500
}
fn default_max_file_size_mb() -> u32 {
    64
}
fn default_row_group_size() -> usize {
    10_000
}
fn default_retention_max_gb() -> u32 {
    20 // 20GB FIFO
}
fn default_retention_check_interval_secs() -> u64 {
    300 // 5 minutes
}
fn default_disk_warning_percent() -> u8 {
    80
}
fn default_disk_critical_percent() -> u8 {
    95
}
fn default_max_span_name_len() -> usize {
    1000
}
fn default_max_attribute_count() -> usize {
    100
}
fn default_max_attribute_value_len() -> usize {
    10 * 1024 // 10KB
}
fn default_max_events_per_span() -> usize {
    100
}
fn default_sse_max_connections() -> usize {
    100
}
fn default_sse_timeout_secs() -> u64 {
    3600 // 1 hour
}
fn default_sse_keepalive_secs() -> u64 {
    30
}

impl Default for OtelConfig {
    fn default() -> Self {
        Self {
            enabled: default_otel_enabled(),
            grpc_enabled: default_grpc_enabled(),
            grpc_port: default_grpc_port(),
            channel_capacity: default_channel_capacity(),
            buffer_max_spans: default_buffer_max_spans(),
            buffer_max_bytes: default_buffer_max_bytes(),
            flush_interval_ms: default_flush_interval_ms(),
            flush_batch_size: default_flush_batch_size(),
            max_file_size_mb: default_max_file_size_mb(),
            row_group_size: default_row_group_size(),
            retention_days: None,
            retention_max_gb: default_retention_max_gb(),
            retention_check_interval_secs: default_retention_check_interval_secs(),
            disk_warning_percent: default_disk_warning_percent(),
            disk_critical_percent: default_disk_critical_percent(),
            max_span_name_len: default_max_span_name_len(),
            max_attribute_count: default_max_attribute_count(),
            max_attribute_value_len: default_max_attribute_value_len(),
            max_events_per_span: default_max_events_per_span(),
            sse_max_connections: default_sse_max_connections(),
            sse_timeout_secs: default_sse_timeout_secs(),
            sse_keepalive_secs: default_sse_keepalive_secs(),
        }
    }
}

/// Configuration manager that handles loading, merging, and providing access to configuration
#[derive(Debug, Clone)]
pub struct ConfigManager {
    config: Config,
    sources: Vec<ConfigSource>,
}

impl ConfigManager {
    /// Initialize configuration from all sources
    ///
    /// Loads configuration in priority order (lowest to highest):
    /// 1. Defaults
    /// 2. User config file (`~/.sideseat/config.json`)
    /// 3. Workdir config file (`./sideseat.json`)
    /// 4. Environment variables
    /// 5. CLI arguments
    ///
    /// # Errors
    ///
    /// Returns an error if a config file exists but contains invalid JSON.
    /// Missing config files are silently skipped.
    pub fn init(storage: &StorageManager, cli_args: &CliConfig) -> Result<Self> {
        let mut config = Config::default();
        let mut sources = Vec::new();

        // 1. Load user config (lowest priority file)
        let user_config_path = storage.user_config_dir().join(CONFIG_FILE_USER);
        match Self::load_json_file(&user_config_path)? {
            Some(user_config) => {
                config = Self::deep_merge_configs(config, user_config);
                sources.push(ConfigSource::loaded("user_config", user_config_path));
            }
            None => {
                sources.push(ConfigSource::skipped("user_config", user_config_path));
            }
        }

        // 2. Load workdir config (higher priority)
        let workdir_config_path = storage.work_dir().join(CONFIG_FILE_WORKDIR);
        match Self::load_json_file(&workdir_config_path)? {
            Some(workdir_config) => {
                config = Self::deep_merge_configs(config, workdir_config);
                sources.push(ConfigSource::loaded("workdir", workdir_config_path));
            }
            None => {
                sources.push(ConfigSource::skipped("workdir", workdir_config_path));
            }
        }

        // 3. Apply environment variables
        Self::apply_env_vars(&mut config);

        // 4. Apply CLI arguments (highest priority)
        Self::apply_cli_args(&mut config, cli_args);

        Ok(Self { config, sources })
    }

    /// Get a reference to the configuration
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get information about loaded configuration sources
    pub fn sources(&self) -> &[ConfigSource] {
        &self.sources
    }

    /// Get only the sources that were successfully loaded
    pub fn loaded_sources(&self) -> impl Iterator<Item = &ConfigSource> {
        self.sources.iter().filter(|s| s.loaded)
    }

    /// Load and parse a JSON configuration file
    ///
    /// Returns `Ok(None)` if the file doesn't exist.
    /// Returns `Err` if the file exists but contains invalid JSON.
    fn load_json_file(path: &Path) -> Result<Option<Config>> {
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(path).map_err(|e| {
            Error::Config(format!("Failed to read config file '{}': {}", path.display(), e))
        })?;

        // Handle empty files gracefully
        if content.trim().is_empty() {
            tracing::debug!("Config file '{}' is empty, skipping", path.display());
            return Ok(None);
        }

        // Parse JSON with detailed error messages
        let config: Config = serde_json::from_str(&content).map_err(|e| {
            let location = format!(" at line {}, column {}", e.line(), e.column());
            Error::Config(format!("Invalid JSON in '{}'{}: {}", path.display(), location, e))
        })?;

        tracing::debug!("Loaded config from '{}'", path.display());
        Ok(Some(config))
    }

    /// Deep merge two configs using JSON values
    ///
    /// The overlay config takes priority over the base config.
    /// - Objects are recursively merged
    /// - Arrays and primitives from overlay replace base values
    /// - Null values in overlay don't override base values
    fn deep_merge_configs(base: Config, overlay: Config) -> Config {
        let base_value = serde_json::to_value(&base).unwrap_or(Value::Null);
        let overlay_value = serde_json::to_value(&overlay).unwrap_or(Value::Null);

        let merged = Self::deep_merge_values(base_value, overlay_value);

        serde_json::from_value(merged).unwrap_or(base)
    }

    /// Recursively merge two JSON values
    fn deep_merge_values(base: Value, overlay: Value) -> Value {
        match (base, overlay) {
            // Both are objects: merge recursively
            (Value::Object(mut base_map), Value::Object(overlay_map)) => {
                for (key, overlay_val) in overlay_map {
                    let merged = match base_map.remove(&key) {
                        Some(base_val) => Self::deep_merge_values(base_val, overlay_val),
                        None => overlay_val,
                    };
                    base_map.insert(key, merged);
                }
                Value::Object(base_map)
            }
            // Overlay is null: keep base value
            (base, Value::Null) => base,
            // Otherwise overlay wins
            (_, overlay) => overlay,
        }
    }

    /// Apply environment variables to config
    fn apply_env_vars(config: &mut Config) {
        // Server host
        if let Ok(host) = std::env::var(ENV_HOST)
            && !host.is_empty()
        {
            config.server.host = host;
        }

        // Server port
        if let Ok(port_str) = std::env::var(ENV_PORT)
            && !port_str.is_empty()
        {
            match port_str.parse::<u16>() {
                Ok(port) => config.server.port = port,
                Err(_) => {
                    tracing::warn!(
                        "Invalid {} value '{}': must be a valid port number (0-65535), ignoring",
                        ENV_PORT,
                        port_str
                    );
                }
            }
        }

        // Auth enabled
        if let Ok(auth_enabled) = std::env::var(ENV_AUTH_ENABLED)
            && !auth_enabled.is_empty()
        {
            match auth_enabled.to_lowercase().as_str() {
                "false" | "0" | "no" | "off" => config.auth.enabled = false,
                "true" | "1" | "yes" | "on" => config.auth.enabled = true,
                _ => {
                    tracing::warn!(
                        "Invalid {} value '{}': use true/false, ignoring",
                        ENV_AUTH_ENABLED,
                        auth_enabled
                    );
                }
            }
        }

        // Note: ENV_LOG, ENV_CONFIG_DIR, ENV_DATA_DIR, ENV_CACHE_DIR are handled by StorageManager
    }

    /// Apply CLI arguments to config (highest priority)
    fn apply_cli_args(config: &mut Config, cli_args: &CliConfig) {
        if let Some(ref host) = cli_args.host {
            config.server.host = host.clone();
        }
        if let Some(port) = cli_args.port {
            config.server.port = port;
        }
        if cli_args.no_auth {
            config.auth.enabled = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.host, DEFAULT_HOST);
        assert_eq!(config.server.port, DEFAULT_PORT);
        assert_eq!(config.logging.level, DEFAULT_LOG_LEVEL);
        assert_eq!(config.logging.format, DEFAULT_LOG_FORMAT);
    }

    #[test]
    fn test_deep_merge_values() {
        let base = serde_json::json!({
            "server": {
                "host": "127.0.0.1",
                "port": 5001
            },
            "logging": {
                "level": "info"
            }
        });

        let overlay = serde_json::json!({
            "server": {
                "port": 3000
            },
            "logging": {
                "level": "debug",
                "format": "json"
            }
        });

        let merged = ConfigManager::deep_merge_values(base, overlay);

        assert_eq!(merged["server"]["host"], "127.0.0.1"); // preserved from base
        assert_eq!(merged["server"]["port"], 3000); // from overlay
        assert_eq!(merged["logging"]["level"], "debug"); // from overlay
        assert_eq!(merged["logging"]["format"], "json"); // from overlay
    }

    #[test]
    fn test_null_doesnt_override() {
        let base = serde_json::json!({"key": "value"});
        let overlay = serde_json::json!({"key": null});

        let merged = ConfigManager::deep_merge_values(base, overlay);
        assert_eq!(merged["key"], "value"); // null doesn't override
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: Config = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.server.host, config.server.host);
        assert_eq!(deserialized.server.port, config.server.port);
    }

    #[test]
    fn test_partial_config_parsing() {
        // Should parse a partial config and use defaults for missing fields
        let json = r#"{ "server": { "port": 3000 } }"#;
        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.host, DEFAULT_HOST); // default
        assert_eq!(config.logging.level, DEFAULT_LOG_LEVEL); // default
    }

    #[test]
    fn test_cli_config_default() {
        let cli = CliConfig::default();
        assert!(cli.host.is_none());
        assert!(cli.port.is_none());
        assert!(!cli.no_auth);
    }

    #[test]
    fn test_config_source_new() {
        let source = ConfigSource::new("test", Some(PathBuf::from("/test/path")), true);
        assert_eq!(source.name, "test");
        assert_eq!(source.path, Some(PathBuf::from("/test/path")));
        assert!(source.loaded);
    }

    #[test]
    fn test_config_source_loaded() {
        let source = ConfigSource::loaded("workdir", PathBuf::from("/work/config.json"));
        assert_eq!(source.name, "workdir");
        assert_eq!(source.path, Some(PathBuf::from("/work/config.json")));
        assert!(source.loaded);
    }

    #[test]
    fn test_config_source_skipped() {
        let source = ConfigSource::skipped("user", PathBuf::from("/home/user/.config"));
        assert_eq!(source.name, "user");
        assert_eq!(source.path, Some(PathBuf::from("/home/user/.config")));
        assert!(!source.loaded);
    }

    #[test]
    fn test_apply_cli_args_host() {
        let mut config = Config::default();
        let cli = CliConfig { host: Some("0.0.0.0".to_string()), port: None, no_auth: false };

        ConfigManager::apply_cli_args(&mut config, &cli);
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, DEFAULT_PORT); // unchanged
    }

    #[test]
    fn test_apply_cli_args_port() {
        let mut config = Config::default();
        let cli = CliConfig { host: None, port: Some(8080), no_auth: false };

        ConfigManager::apply_cli_args(&mut config, &cli);
        assert_eq!(config.server.host, DEFAULT_HOST); // unchanged
        assert_eq!(config.server.port, 8080);
    }

    #[test]
    fn test_apply_cli_args_no_auth() {
        let mut config = Config::default();
        assert!(config.auth.enabled); // default is true

        let cli = CliConfig { host: None, port: None, no_auth: true };
        ConfigManager::apply_cli_args(&mut config, &cli);
        assert!(!config.auth.enabled);
    }

    #[test]
    fn test_apply_cli_args_all() {
        let mut config = Config::default();
        let cli =
            CliConfig { host: Some("localhost".to_string()), port: Some(3000), no_auth: true };

        ConfigManager::apply_cli_args(&mut config, &cli);
        assert_eq!(config.server.host, "localhost");
        assert_eq!(config.server.port, 3000);
        assert!(!config.auth.enabled);
    }

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.host, DEFAULT_HOST);
        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn test_logging_config_default() {
        let config = LoggingConfig::default();
        assert_eq!(config.level, DEFAULT_LOG_LEVEL);
        assert_eq!(config.format, DEFAULT_LOG_FORMAT);
    }

    #[test]
    fn test_auth_config_default() {
        let config = AuthConfig::default();
        assert!(config.enabled);
    }

    #[test]
    fn test_storage_config_default() {
        let config = StorageConfig::default();
        assert!(config.config_dir.is_none());
        assert!(config.data_dir.is_none());
        assert!(config.cache_dir.is_none());
    }

    #[test]
    fn test_otel_config_default() {
        let config = OtelConfig::default();
        assert!(config.enabled);
        assert!(config.grpc_enabled);
        assert_eq!(config.grpc_port, 4317);
        assert_eq!(config.channel_capacity, 10000);
        assert_eq!(config.buffer_max_spans, 5000);
        assert_eq!(config.buffer_max_bytes, 10 * 1024 * 1024);
        assert_eq!(config.flush_interval_ms, 100);
        assert_eq!(config.flush_batch_size, 500);
        assert_eq!(config.max_file_size_mb, 64);
        assert_eq!(config.row_group_size, 10_000);
        assert!(config.retention_days.is_none());
        assert_eq!(config.retention_max_gb, 20);
        assert_eq!(config.retention_check_interval_secs, 300);
        assert_eq!(config.disk_warning_percent, 80);
        assert_eq!(config.disk_critical_percent, 95);
        assert_eq!(config.max_span_name_len, 1000);
        assert_eq!(config.max_attribute_count, 100);
        assert_eq!(config.max_attribute_value_len, 10 * 1024);
        assert_eq!(config.max_events_per_span, 100);
        assert_eq!(config.sse_max_connections, 100);
        assert_eq!(config.sse_timeout_secs, 3600);
        assert_eq!(config.sse_keepalive_secs, 30);
    }

    #[test]
    fn test_otel_config_serialization() {
        let config = OtelConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: OtelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.grpc_port, deserialized.grpc_port);
        assert_eq!(config.buffer_max_spans, deserialized.buffer_max_spans);
    }

    #[test]
    fn test_deep_merge_nested_objects() {
        let base = serde_json::json!({
            "a": {
                "b": {
                    "c": 1,
                    "d": 2
                }
            }
        });

        let overlay = serde_json::json!({
            "a": {
                "b": {
                    "c": 10
                }
            }
        });

        let merged = ConfigManager::deep_merge_values(base, overlay);
        assert_eq!(merged["a"]["b"]["c"], 10); // from overlay
        assert_eq!(merged["a"]["b"]["d"], 2); // preserved from base
    }

    #[test]
    fn test_deep_merge_array_replacement() {
        let base = serde_json::json!({"arr": [1, 2, 3]});
        let overlay = serde_json::json!({"arr": [4, 5]});

        let merged = ConfigManager::deep_merge_values(base, overlay);
        assert_eq!(merged["arr"], serde_json::json!([4, 5])); // arrays are replaced, not merged
    }

    #[test]
    fn test_deep_merge_primitive_replacement() {
        let base = serde_json::json!({"num": 42, "str": "hello"});
        let overlay = serde_json::json!({"num": 100, "str": "world"});

        let merged = ConfigManager::deep_merge_values(base, overlay);
        assert_eq!(merged["num"], 100);
        assert_eq!(merged["str"], "world");
    }

    #[test]
    fn test_deep_merge_add_new_keys() {
        let base = serde_json::json!({"existing": "value"});
        let overlay = serde_json::json!({"new": "added"});

        let merged = ConfigManager::deep_merge_values(base, overlay);
        assert_eq!(merged["existing"], "value");
        assert_eq!(merged["new"], "added");
    }
}
