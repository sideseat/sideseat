//! Cross-platform secret storage manager
//!
//! Provides secure storage for credentials, API keys, and other secrets using
//! OS-native credential stores. All secrets are stored in a single keychain entry
//! (vault) to minimize permission prompts.
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
//! use sideseat::core::SecretManager;
//!
//! let storage = StorageManager::init().await?;
//! let secrets = SecretManager::init(&storage).await?;
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
use super::storage::StorageManager;
use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Filename for file-based secret storage
const FILE_SECRETS_FILENAME: &str = "secrets.json";

/// Keychain entry name for the secret vault
const VAULT_KEY: &str = "vault";

/// Secret storage backend type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretBackend {
    /// macOS Keychain
    AppleKeychain,
    /// Windows Credential Manager
    WindowsCredential,
    /// Linux Secret Service (GNOME Keyring, KWallet, etc.)
    LinuxSecretService,
    /// Linux keyutils (kernel keyring, session-scoped)
    LinuxKeyutils,
    /// File-based storage (for development only - NOT SECURE)
    File(PathBuf),
}

impl SecretBackend {
    /// Returns true if secrets persist across reboots
    pub fn is_persistent(&self) -> bool {
        match self {
            Self::AppleKeychain | Self::WindowsCredential | Self::LinuxSecretService => true,
            Self::LinuxKeyutils => false, // Session-scoped only
            Self::File(_) => true,        // File persists, but NOT SECURE
        }
    }

    /// Get a human-readable name for the backend
    pub fn name(&self) -> &'static str {
        match self {
            Self::AppleKeychain => "macOS Keychain",
            Self::WindowsCredential => "Windows Credential Manager",
            Self::LinuxSecretService => "Secret Service",
            Self::LinuxKeyutils => "Linux keyutils",
            Self::File(_) => "File (INSECURE - dev only)",
        }
    }

    /// Returns true if this is the insecure file backend
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
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
        self.expires_at.is_some_and(|exp| exp < Utc::now())
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

/// Secret vault - stores all secrets in a single structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SecretVault {
    secrets: HashMap<String, Secret>,
}

/// Cross-platform secret storage manager
///
/// Provides secure storage for credentials using OS-native credential stores.
/// All secrets are stored in a single keychain entry (vault) to minimize
/// permission prompts - one "Always Allow" grants access to all secrets.
#[derive(Debug, Clone)]
pub struct SecretManager {
    backend: SecretBackend,
    service_name: String,
    /// In-memory cache of the vault
    vault: Arc<RwLock<SecretVault>>,
}

impl SecretManager {
    /// Initialize the secret manager
    ///
    /// Detects the appropriate backend for the current platform and loads
    /// the secret vault from storage. On macOS, the keychain will prompt
    /// once - click "Always Allow" to grant permanent access to all secrets.
    ///
    /// For development, set `SIDESEAT_SECRET_BACKEND=file` to use insecure
    /// file-based storage and avoid keychain prompts. The file will be stored
    /// in the data directory provided by StorageManager.
    pub async fn init(storage: &StorageManager) -> Result<Self> {
        let backend = Self::detect_backend(storage).await;
        let service_name = SECRET_SERVICE_NAME.to_string();

        // Load existing vault from appropriate backend
        let vault = Self::load_vault(&backend, &service_name).await?;

        let manager =
            Self { backend: backend.clone(), service_name, vault: Arc::new(RwLock::new(vault)) };

        if manager.backend.is_file() {
            tracing::warn!(
                "⚠️  Using INSECURE file-based secret storage. DO NOT use in production!"
            );
        } else if !manager.backend.is_persistent() {
            tracing::warn!(
                "Secret storage backend '{}' is session-scoped. Secrets will not persist across reboots.",
                manager.backend.name()
            );
        } else {
            tracing::debug!("Secret storage initialized with backend: {}", manager.backend.name());
        }

        Ok(manager)
    }

    /// Get the active backend type
    pub fn backend(&self) -> &SecretBackend {
        &self.backend
    }

    /// Check if the backend provides persistent storage
    pub fn is_persistent(&self) -> bool {
        self.backend.is_persistent()
    }

    /// Store a secret with metadata
    pub async fn set(&self, key: &SecretKey, secret: &Secret) -> Result<()> {
        {
            let mut vault = self.vault.write().await;
            vault.secrets.insert(key.name.clone(), secret.clone());
        }
        self.save_vault().await?;
        tracing::debug!("Stored secret: {}", key.name);
        Ok(())
    }

    /// Retrieve a secret
    ///
    /// Returns `None` if the secret doesn't exist. Returns an error if the
    /// secret exists but is expired.
    pub async fn get(&self, key: &SecretKey) -> Result<Option<Secret>> {
        let vault = self.vault.read().await;
        match vault.secrets.get(&key.name) {
            Some(secret) => {
                if secret.is_expired() {
                    tracing::warn!("Secret '{}' has expired", key.name);
                    return Err(Error::Secret(format!("Secret '{}' has expired", key.name)));
                }
                Ok(Some(secret.clone()))
            }
            None => Ok(None),
        }
    }

    /// Delete a secret
    pub async fn delete(&self, key: &SecretKey) -> Result<()> {
        {
            let mut vault = self.vault.write().await;
            if vault.secrets.remove(&key.name).is_none() {
                return Err(Error::Secret(format!("Secret '{}' not found", key.name)));
            }
        }
        self.save_vault().await?;
        tracing::debug!("Deleted secret: {}", key.name);
        Ok(())
    }

    /// Check if a secret exists
    pub async fn exists(&self, key: &SecretKey) -> Result<bool> {
        let vault = self.vault.read().await;
        Ok(vault.secrets.contains_key(&key.name))
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

        {
            let mut vault = self.vault.write().await;
            match vault.secrets.get_mut(&key.name) {
                Some(secret) => {
                    secret.value = new_value.to_string();
                    secret.metadata.updated_at = Utc::now();
                }
                None => return Err(Error::Secret(format!("Secret '{}' not found", name))),
            }
        }
        self.save_vault().await
    }

    // === Internal Methods ===

    /// Load the secret vault from the appropriate backend
    async fn load_vault(backend: &SecretBackend, service_name: &str) -> Result<SecretVault> {
        match backend {
            SecretBackend::File(path) => Self::load_vault_from_file(path).await,
            _ => Self::load_vault_from_keychain(service_name).await,
        }
    }

    /// Load vault from file (for development)
    async fn load_vault_from_file(path: &PathBuf) -> Result<SecretVault> {
        match tokio::fs::read_to_string(path).await {
            Ok(json) => {
                let vault: SecretVault = serde_json::from_str(&json)
                    .map_err(|e| Error::Secret(format!("Vault deserialization error: {}", e)))?;
                tracing::debug!(
                    "Loaded secret vault from file with {} entries",
                    vault.secrets.len()
                );
                Ok(vault)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("No existing secret vault file, creating new one");
                Ok(SecretVault::default())
            }
            Err(e) => Err(Error::Secret(format!("Failed to load vault file: {}", e))),
        }
    }

    /// Load vault from keychain
    async fn load_vault_from_keychain(service_name: &str) -> Result<SecretVault> {
        let entry = Entry::new(service_name, VAULT_KEY)
            .map_err(|e| Error::Secret(format!("Failed to create entry: {}", e)))?;

        let result = tokio::task::spawn_blocking(move || entry.get_password())
            .await
            .map_err(|e| Error::Secret(format!("Task join error: {}", e)))?;

        match result {
            Ok(json) => {
                let vault: SecretVault = serde_json::from_str(&json)
                    .map_err(|e| Error::Secret(format!("Vault deserialization error: {}", e)))?;
                tracing::debug!("Loaded secret vault with {} entries", vault.secrets.len());
                Ok(vault)
            }
            Err(keyring::Error::NoEntry) => {
                tracing::debug!("No existing secret vault, creating new one");
                Ok(SecretVault::default())
            }
            Err(e) => Err(Error::Secret(format!("Failed to load vault: {}", e))),
        }
    }

    /// Save the secret vault to the appropriate backend
    async fn save_vault(&self) -> Result<()> {
        match &self.backend {
            SecretBackend::File(path) => self.save_vault_to_file(path).await,
            _ => self.save_vault_to_keychain().await,
        }
    }

    /// Save vault to file (for development)
    async fn save_vault_to_file(&self, path: &PathBuf) -> Result<()> {
        let vault = self.vault.read().await;
        let json = serde_json::to_string_pretty(&*vault)
            .map_err(|e| Error::Secret(format!("Vault serialization error: {}", e)))?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::Secret(format!("Failed to create secrets directory: {}", e)))?;
        }

        tokio::fs::write(path, json)
            .await
            .map_err(|e| Error::Secret(format!("Failed to save vault file: {}", e)))?;

        Ok(())
    }

    /// Save vault to keychain
    async fn save_vault_to_keychain(&self) -> Result<()> {
        let vault = self.vault.read().await;
        let json = serde_json::to_string(&*vault)
            .map_err(|e| Error::Secret(format!("Vault serialization error: {}", e)))?;

        let service_name = self.service_name.clone();
        let entry = Entry::new(&service_name, VAULT_KEY)
            .map_err(|e| Error::Secret(format!("Failed to create entry: {}", e)))?;

        tokio::task::spawn_blocking(move || entry.set_password(&json))
            .await
            .map_err(|e| Error::Secret(format!("Task join error: {}", e)))?
            .map_err(|e| Error::Secret(format!("Failed to save vault: {}", e)))?;

        Ok(())
    }

    /// Detect the appropriate backend for the current platform
    async fn detect_backend(storage: &StorageManager) -> SecretBackend {
        // Check for environment variable override (with platform validation)
        if let Ok(override_backend) = std::env::var(ENV_SECRET_BACKEND)
            && let Some(backend) = Self::parse_backend_override(&override_backend, storage)
        {
            return backend;
        }

        Self::detect_platform_backend().await
    }

    /// Parse and validate a backend override from environment variable
    /// Returns None if the override is invalid or not applicable for this platform
    fn parse_backend_override(value: &str, storage: &StorageManager) -> Option<SecretBackend> {
        match value.to_lowercase().as_str() {
            // File backend available on all platforms (for development only)
            "file" => {
                let path = storage.data_dir().join(FILE_SECRETS_FILENAME);
                Some(SecretBackend::File(path))
            }

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
            "keychain, file"
        }
        #[cfg(target_os = "windows")]
        {
            "credential-manager, file"
        }
        #[cfg(target_os = "linux")]
        {
            "secret-service, keyutils, file"
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            "file"
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

    #[test]
    fn test_vault_serialization() {
        let mut vault = SecretVault::default();
        vault.secrets.insert("key1".to_string(), Secret::new("value1"));
        vault.secrets.insert("key2".to_string(), Secret::with_provider("value2", "provider2"));

        let json = serde_json::to_string(&vault).unwrap();
        let deserialized: SecretVault = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.secrets.len(), 2);
        assert_eq!(deserialized.secrets.get("key1").unwrap().value, "value1");
        assert_eq!(deserialized.secrets.get("key2").unwrap().value, "value2");
    }
}
