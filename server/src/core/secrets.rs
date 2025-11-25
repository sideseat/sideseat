//! Cross-platform secret storage manager
//!
//! Provides secure storage for credentials, API keys, and other secrets using
//! OS-native credential stores:
//!
//! | Platform | Backend |
//! |----------|---------|
//! | macOS | Keychain |
//! | Windows | Credential Manager |
//! | Linux | Secret Service (GNOME Keyring/KWallet) or keyutils fallback |
//!
//! ## Usage
//!
//! ```rust,ignore
//! use sideseat::core::{SecretManager, SecretKey};
//!
//! let secrets = SecretManager::init().await?;
//!
//! // Store an API key
//! secrets.set_api_key("OPENAI_API_KEY", "sk-xxx", Some("openai")).await?;
//!
//! // Retrieve a secret value
//! if let Some(value) = secrets.get_value("OPENAI_API_KEY").await? {
//!     println!("Got API key");
//! }
//! ```

use super::constants::{ENV_SECRET_BACKEND, SECRET_SERVICE_NAME};
use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use keyring::Entry;
use serde::{Deserialize, Serialize};

/// Secret storage backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretBackend {
    /// macOS Keychain
    AppleKeychain,
    /// Windows Credential Manager
    WindowsCredential,
    /// Linux Secret Service (GNOME Keyring, KWallet, etc.)
    LinuxSecretService,
    /// Linux keyutils (kernel keyring, session-scoped)
    LinuxKeyutils,
}

impl SecretBackend {
    /// Returns true if secrets persist across reboots
    pub fn is_persistent(&self) -> bool {
        match self {
            Self::AppleKeychain | Self::WindowsCredential | Self::LinuxSecretService => true,
            Self::LinuxKeyutils => false, // Session-scoped only
        }
    }

    /// Get a human-readable name for the backend
    pub fn name(&self) -> &'static str {
        match self {
            Self::AppleKeychain => "macOS Keychain",
            Self::WindowsCredential => "Windows Credential Manager",
            Self::LinuxSecretService => "Secret Service",
            Self::LinuxKeyutils => "Linux keyutils",
        }
    }
}

/// Metadata associated with a stored secret
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMetadata {
    /// Provider/service name (e.g., "openai", "anthropic", "github")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Scope or category (e.g., "api", "oauth", "database")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Optional expiration timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// When the secret was first stored
    pub created_at: DateTime<Utc>,
    /// When the secret was last updated
    pub updated_at: DateTime<Utc>,
}

impl SecretMetadata {
    /// Create new metadata with current timestamps
    pub fn new() -> Self {
        let now = Utc::now();
        Self { provider: None, scope: None, expires_at: None, created_at: now, updated_at: now }
    }

    /// Create metadata with a provider
    pub fn with_provider(provider: impl Into<String>) -> Self {
        let mut meta = Self::new();
        meta.provider = Some(provider.into());
        meta
    }

    /// Check if the secret has expired
    pub fn is_expired(&self) -> bool {
        self.expires_at.map(|exp| exp < Utc::now()).unwrap_or(false)
    }
}

impl Default for SecretMetadata {
    fn default() -> Self {
        Self::new()
    }
}

/// A secret with its value and metadata
#[derive(Clone, Serialize, Deserialize)]
pub struct Secret {
    /// The secret value (API key, password, token, etc.)
    pub value: String,
    /// Associated metadata
    pub metadata: SecretMetadata,
}

// Custom Debug implementation to prevent accidental secret exposure in logs
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secret")
            .field("value", &"[REDACTED]")
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl Secret {
    /// Create a new secret with default metadata
    pub fn new(value: impl Into<String>) -> Self {
        Self { value: value.into(), metadata: SecretMetadata::new() }
    }

    /// Create a new secret with provider metadata
    pub fn with_provider(value: impl Into<String>, provider: impl Into<String>) -> Self {
        Self { value: value.into(), metadata: SecretMetadata::with_provider(provider) }
    }

    /// Check if the secret has expired
    pub fn is_expired(&self) -> bool {
        self.metadata.is_expired()
    }
}

/// Identifier for a stored secret
#[derive(Debug, Clone)]
pub struct SecretKey {
    /// The secret name (e.g., "OPENAI_API_KEY")
    pub name: String,
    /// Optional target for disambiguating entries with the same name
    pub target: Option<String>,
}

impl SecretKey {
    /// Create a new secret key
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), target: None }
    }

    /// Create a secret key with a target
    pub fn with_target(name: impl Into<String>, target: impl Into<String>) -> Self {
        Self { name: name.into(), target: Some(target.into()) }
    }
}

impl<S: Into<String>> From<S> for SecretKey {
    fn from(name: S) -> Self {
        Self::new(name)
    }
}

/// Cross-platform secret storage manager
///
/// Provides secure storage for credentials using OS-native credential stores.
/// The manager automatically detects the best available backend for the current
/// platform and provides a consistent async API across all platforms.
#[derive(Debug, Clone)]
pub struct SecretManager {
    backend: SecretBackend,
    service_name: String,
}

impl SecretManager {
    /// Initialize the secret manager
    ///
    /// Detects the appropriate backend for the current platform. On Linux,
    /// attempts to use Secret Service first, falling back to keyutils if
    /// unavailable.
    ///
    /// The backend can be overridden via the `SIDESEAT_SECRET_BACKEND` environment
    /// variable (values: "keychain", "credential-manager", "secret-service", "keyutils").
    pub async fn init() -> Result<Self> {
        let backend = Self::detect_backend().await;
        let service_name = SECRET_SERVICE_NAME.to_string();

        let manager = Self { backend, service_name };

        if !backend.is_persistent() {
            tracing::warn!(
                "Secret storage backend '{}' is session-scoped. Secrets will not persist across reboots.",
                backend.name()
            );
        } else {
            tracing::info!("Secret storage initialized with backend: {}", backend.name());
        }

        Ok(manager)
    }

    /// Get the active backend type
    pub fn backend(&self) -> SecretBackend {
        self.backend
    }

    /// Check if the backend provides persistent storage
    pub fn is_persistent(&self) -> bool {
        self.backend.is_persistent()
    }

    /// Store a secret with metadata
    pub async fn set(&self, key: &SecretKey, secret: &Secret) -> Result<()> {
        let entry = self.create_entry(key)?;
        let json = serde_json::to_string(secret)
            .map_err(|e| Error::Secret(format!("Serialization error: {}", e)))?;

        // keyring operations are sync, wrap in spawn_blocking
        tokio::task::spawn_blocking(move || entry.set_password(&json))
            .await
            .map_err(|e| Error::Secret(format!("Task join error: {}", e)))?
            .map_err(|e| Error::Secret(format!("Failed to store secret: {}", e)))?;

        tracing::debug!("Stored secret: {}", key.name);
        Ok(())
    }

    /// Retrieve a secret
    ///
    /// Returns `None` if the secret doesn't exist. Returns an error if the
    /// secret exists but is expired.
    pub async fn get(&self, key: &SecretKey) -> Result<Option<Secret>> {
        let entry = self.create_entry(key)?;

        let result = tokio::task::spawn_blocking(move || entry.get_password())
            .await
            .map_err(|e| Error::Secret(format!("Task join error: {}", e)))?;

        match result {
            Ok(json) => {
                let secret: Secret = serde_json::from_str(&json)
                    .map_err(|e| Error::Secret(format!("Deserialization error: {}", e)))?;

                if secret.is_expired() {
                    tracing::warn!("Secret '{}' has expired", key.name);
                    return Err(Error::Secret(format!("Secret '{}' has expired", key.name)));
                }

                Ok(Some(secret))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Secret(format!("Failed to retrieve secret: {}", e))),
        }
    }

    /// Delete a secret
    pub async fn delete(&self, key: &SecretKey) -> Result<()> {
        let entry = self.create_entry(key)?;

        tokio::task::spawn_blocking(move || entry.delete_credential())
            .await
            .map_err(|e| Error::Secret(format!("Task join error: {}", e)))?
            .map_err(|e| match e {
                keyring::Error::NoEntry => {
                    Error::Secret(format!("Secret '{}' not found", key.name))
                }
                e => Error::Secret(format!("Failed to delete secret: {}", e)),
            })?;

        tracing::debug!("Deleted secret: {}", key.name);
        Ok(())
    }

    /// Check if a secret exists
    pub async fn exists(&self, key: &SecretKey) -> Result<bool> {
        let entry = self.create_entry(key)?;

        let result = tokio::task::spawn_blocking(move || entry.get_password())
            .await
            .map_err(|e| Error::Secret(format!("Task join error: {}", e)))?;

        match result {
            Ok(_) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(Error::Secret(format!("Failed to check secret: {}", e))),
        }
    }

    // === Convenience Methods ===

    /// Store a simple API key with minimal metadata
    pub async fn set_api_key(&self, name: &str, value: &str, provider: Option<&str>) -> Result<()> {
        let secret = match provider {
            Some(p) => Secret::with_provider(value, p),
            None => Secret::new(value),
        };
        let key = SecretKey::new(name);
        self.set(&key, &secret).await
    }

    /// Get just the secret value (ignores metadata)
    ///
    /// Returns `None` if the secret doesn't exist.
    pub async fn get_value(&self, name: &str) -> Result<Option<String>> {
        let key = SecretKey::new(name);
        match self.get(&key).await? {
            Some(secret) => Ok(Some(secret.value)),
            None => Ok(None),
        }
    }

    /// Update an existing secret's value while preserving metadata
    ///
    /// Returns an error if the secret doesn't exist.
    pub async fn update_value(&self, name: &str, new_value: &str) -> Result<()> {
        let key = SecretKey::new(name);

        match self.get(&key).await? {
            Some(mut secret) => {
                secret.value = new_value.to_string();
                secret.metadata.updated_at = Utc::now();
                self.set(&key, &secret).await
            }
            None => Err(Error::Secret(format!("Secret '{}' not found", name))),
        }
    }

    // === Internal Methods ===

    /// Create a keyring entry for the given key
    fn create_entry(&self, key: &SecretKey) -> Result<Entry> {
        match &key.target {
            Some(target) => Entry::new_with_target(target, &self.service_name, &key.name),
            None => Entry::new(&self.service_name, &key.name),
        }
        .map_err(|e| Error::Secret(format!("Failed to create entry: {}", e)))
    }

    /// Detect the appropriate backend for the current platform
    async fn detect_backend() -> SecretBackend {
        // Check for environment variable override (with platform validation)
        if let Ok(override_backend) = std::env::var(ENV_SECRET_BACKEND)
            && let Some(backend) = Self::parse_backend_override(&override_backend)
        {
            return backend;
        }

        Self::detect_platform_backend().await
    }

    /// Parse and validate a backend override from environment variable
    /// Returns None if the override is invalid or not applicable for this platform
    fn parse_backend_override(value: &str) -> Option<SecretBackend> {
        match value.to_lowercase().as_str() {
            #[cfg(target_os = "macos")]
            "keychain" => Some(SecretBackend::AppleKeychain),

            #[cfg(target_os = "windows")]
            "credential-manager" => Some(SecretBackend::WindowsCredential),

            #[cfg(target_os = "linux")]
            "secret-service" => Some(SecretBackend::LinuxSecretService),

            #[cfg(target_os = "linux")]
            "keyutils" => Some(SecretBackend::LinuxKeyutils),

            // Invalid or platform-mismatched override
            other => {
                let valid_options = Self::valid_backend_options();
                tracing::warn!(
                    "Invalid secret backend '{}' for this platform. Valid options: {}",
                    other,
                    valid_options
                );
                None
            }
        }
    }

    /// Get valid backend options for the current platform
    fn valid_backend_options() -> &'static str {
        #[cfg(target_os = "macos")]
        {
            "keychain"
        }
        #[cfg(target_os = "windows")]
        {
            "credential-manager"
        }
        #[cfg(target_os = "linux")]
        {
            "secret-service, keyutils"
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            "(none available)"
        }
    }

    /// Detect the platform-specific backend
    async fn detect_platform_backend() -> SecretBackend {
        #[cfg(target_os = "macos")]
        return SecretBackend::AppleKeychain;

        #[cfg(target_os = "windows")]
        return SecretBackend::WindowsCredential;

        #[cfg(target_os = "linux")]
        return Self::detect_linux_backend().await;

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        return SecretBackend::LinuxKeyutils;
    }

    /// Detect the best available Linux backend
    #[cfg(target_os = "linux")]
    async fn detect_linux_backend() -> SecretBackend {
        // Try Secret Service first (requires D-Bus and a secret service daemon)
        // We do this by attempting to create a test entry
        let test_result = tokio::task::spawn_blocking(|| {
            // Try to create an entry - this will fail fast if Secret Service is unavailable
            match Entry::new(SECRET_SERVICE_NAME, "__sideseat_backend_test__") {
                Ok(entry) => {
                    // Try to get a non-existent password - NoEntry error means service is working
                    match entry.get_password() {
                        Err(keyring::Error::NoEntry) => true,
                        Ok(_) => true,   // Unlikely but service is working
                        Err(_) => false, // Service not available
                    }
                }
                Err(_) => false,
            }
        })
        .await;

        match test_result {
            Ok(true) => {
                tracing::debug!("Secret Service detected and available");
                SecretBackend::LinuxSecretService
            }
            _ => {
                tracing::debug!("Secret Service not available, falling back to keyutils");
                SecretBackend::LinuxKeyutils
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_metadata_expiry() {
        let mut meta = SecretMetadata::new();
        assert!(!meta.is_expired());

        // Set expiry in the past
        meta.expires_at = Some(Utc::now() - chrono::Duration::hours(1));
        assert!(meta.is_expired());

        // Set expiry in the future
        meta.expires_at = Some(Utc::now() + chrono::Duration::hours(1));
        assert!(!meta.is_expired());
    }

    #[test]
    fn test_secret_serialization() {
        let secret = Secret::with_provider("test-value", "test-provider");
        let json = serde_json::to_string(&secret).unwrap();
        let deserialized: Secret = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.value, "test-value");
        assert_eq!(deserialized.metadata.provider, Some("test-provider".to_string()));
    }

    #[test]
    fn test_secret_key_from_string() {
        let key: SecretKey = "MY_API_KEY".into();
        assert_eq!(key.name, "MY_API_KEY");
        assert!(key.target.is_none());
    }

    #[test]
    fn test_backend_persistence() {
        assert!(SecretBackend::AppleKeychain.is_persistent());
        assert!(SecretBackend::WindowsCredential.is_persistent());
        assert!(SecretBackend::LinuxSecretService.is_persistent());
        assert!(!SecretBackend::LinuxKeyutils.is_persistent());
    }

    #[test]
    fn test_secret_debug_redacts_value() {
        let secret = Secret::new("super-secret-api-key-12345");
        let debug_output = format!("{:?}", secret);

        // Should contain [REDACTED] instead of actual value
        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("super-secret-api-key-12345"));
    }

    #[test]
    fn test_valid_backend_options() {
        let options = SecretManager::valid_backend_options();
        // Should return non-empty string for supported platforms
        assert!(!options.is_empty());

        #[cfg(target_os = "macos")]
        assert!(options.contains("keychain"));

        #[cfg(target_os = "linux")]
        assert!(options.contains("secret-service"));
    }
}
