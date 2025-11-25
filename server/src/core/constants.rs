//! Application-wide constants
//!
//! Centralized constants for application naming, paths, environment variables,
//! and default configuration values.
//!
//! ## Environment Variables
//!
//! All environment variables use the `SIDESEAT_` prefix:
//!
//! | Variable | Description |
//! |----------|-------------|
//! | `SIDESEAT_HOST` | Server host address |
//! | `SIDESEAT_PORT` | Server port |
//! | `SIDESEAT_LOG` | Log level/filter |
//! | `SIDESEAT_CONFIG_DIR` | Override config directory |
//! | `SIDESEAT_DATA_DIR` | Override data directory |
//! | `SIDESEAT_CACHE_DIR` | Override cache directory |
//! | `SIDESEAT_SECRET_BACKEND` | Force secret storage backend |

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

/// User config file name (in user_config_dir: ~/.sideseat/config.json)
pub const CONFIG_FILE_USER: &str = "config.json";

/// Workdir config file name (in current working directory: ./sideseat.json)
pub const CONFIG_FILE_WORKDIR: &str = "sideseat.json";

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
// Environment Variables - Storage
// =============================================================================

/// Environment variable to override config directory
pub const ENV_CONFIG_DIR: &str = "SIDESEAT_CONFIG_DIR";

/// Environment variable to override data directory
pub const ENV_DATA_DIR: &str = "SIDESEAT_DATA_DIR";

/// Environment variable to override cache directory
pub const ENV_CACHE_DIR: &str = "SIDESEAT_CACHE_DIR";

// =============================================================================
// Environment Variables - Secrets
// =============================================================================

/// Environment variable to force specific secret backend
///
/// Valid values by platform:
/// - macOS: `keychain`
/// - Windows: `credential-manager`
/// - Linux: `secret-service`, `keyutils`
pub const ENV_SECRET_BACKEND: &str = "SIDESEAT_SECRET_BACKEND";

// =============================================================================
// Default Values
// =============================================================================

/// Default server host (localhost only for security)
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Default server port
pub const DEFAULT_PORT: u16 = 5001;

/// Default log level
pub const DEFAULT_LOG_LEVEL: &str = "info";

/// Default log format
pub const DEFAULT_LOG_FORMAT: &str = "compact";

// =============================================================================
// Internal Constants
// =============================================================================

/// File used to verify directory access during initialization
pub const ACCESS_CHECK_FILE: &str = ".sideseat_access_check";

/// Service name for keyring entries (groups all secrets under this identifier)
pub const SECRET_SERVICE_NAME: &str = "sideseat";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_name_consistency() {
        assert_eq!(APP_NAME.to_lowercase(), APP_NAME_LOWER);
    }

    #[test]
    fn test_dot_folder_starts_with_dot() {
        assert!(APP_DOT_FOLDER.starts_with('.'));
    }

    #[test]
    fn test_env_vars_have_prefix() {
        let env_vars = [
            ENV_HOST,
            ENV_PORT,
            ENV_LOG,
            ENV_CONFIG_DIR,
            ENV_DATA_DIR,
            ENV_CACHE_DIR,
            ENV_SECRET_BACKEND,
        ];

        for var in env_vars {
            assert!(var.starts_with("SIDESEAT_"), "Env var {} should have SIDESEAT_ prefix", var);
        }
    }

    #[test]
    fn test_default_port_valid() {
        // Use a variable to avoid clippy::assertions_on_constants
        let port = DEFAULT_PORT;
        assert!(port > 0);
        assert!(port < 65535);
    }

    #[test]
    fn test_config_files_are_json() {
        assert!(CONFIG_FILE_USER.ends_with(".json"));
        assert!(CONFIG_FILE_WORKDIR.ends_with(".json"));
    }
}
