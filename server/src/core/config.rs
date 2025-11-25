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
    DEFAULT_PORT, ENV_HOST, ENV_PORT,
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
    }
}
