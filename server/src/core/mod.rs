//! Core application infrastructure
//!
//! This module provides the foundational components for the application:
//!
//! - [`ConfigManager`] - Multi-source configuration loading with smart merging
//! - [`StorageManager`] - Platform-aware storage directory management
//! - [`SecretManager`] - Cross-platform secure credential storage
//! - [`constants`] - Application-wide constants and defaults
//! - [`utils`] - Utility functions

pub mod config;
pub mod constants;
pub mod secrets;
pub mod storage;
pub mod utils;

// Config exports
pub use config::{
    AuthConfig, CliConfig, Config, ConfigManager, ConfigSource, LoggingConfig, ServerConfig,
    StorageConfig,
};

// Storage exports
pub use storage::{DataSubdir, StorageManager, StorageType};

// Secret exports
pub use secrets::{Secret, SecretBackend, SecretKey, SecretManager, SecretMetadata};
