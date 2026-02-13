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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::constants::{
    ENV_SECRET_BACKEND, SECRET_KEY_API_KEY, SECRET_KEY_JWT_SIGNING, SECRET_SERVICE_NAME,
};
use super::storage::AppStorage;
use crate::utils::crypto;

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
            Self::LinuxKeyutils => false,
            Self::File(_) => true,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SecretMetadata {
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            provider: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_provider(provider: impl Into<String>) -> Self {
        let mut meta = Self::new();
        meta.provider = Some(provider.into());
        meta
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
    pub value: String,
    pub metadata: SecretMetadata,
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secret")
            .field("value", &"[REDACTED]")
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            metadata: SecretMetadata::new(),
        }
    }

    pub fn with_provider(value: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            metadata: SecretMetadata::with_provider(provider),
        }
    }
}

/// Secret vault - stores all secrets in a single structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SecretVault {
    secrets: HashMap<String, Secret>,
}

/// Cross-platform secret storage manager
///
/// All secrets are cached in memory after initial load.
/// Changes update both memory and storage.
#[derive(Debug, Clone)]
pub struct SecretManager {
    backend: SecretBackend,
    service_name: String,
    /// In-memory cache of all secrets
    vault: Arc<RwLock<SecretVault>>,
    /// Mutex to serialize save operations and prevent race conditions
    save_mutex: Arc<tokio::sync::Mutex<()>>,
}

impl SecretManager {
    /// Initialize the secret manager
    pub async fn init(storage: &AppStorage) -> Result<Self> {
        let backend = Self::detect_backend(storage).await;
        let service_name = SECRET_SERVICE_NAME.to_string();

        let vault = Self::load_vault(&backend, &service_name).await?;

        let manager = Self {
            backend: backend.clone(),
            service_name,
            vault: Arc::new(RwLock::new(vault)),
            save_mutex: Arc::new(tokio::sync::Mutex::new(())),
        };

        if manager.backend.is_file() {
            tracing::warn!("Using INSECURE file-based secret storage. DO NOT use in production!");
        } else if !manager.backend.is_persistent() {
            tracing::warn!(
                "Secret backend '{}' is session-scoped. Secrets won't persist across reboots.",
                manager.backend.name()
            );
        }

        tracing::debug!(
            backend = manager.backend.name(),
            "Secret manager initialized"
        );
        Ok(manager)
    }

    /// Get the active backend type
    pub fn backend(&self) -> &SecretBackend {
        &self.backend
    }

    /// Store a secret
    pub async fn set(&self, name: &str, mut secret: Secret) -> Result<()> {
        {
            let mut vault = self.vault.write().await;
            // Preserve created_at if updating existing secret
            if let Some(existing) = vault.secrets.get(name) {
                secret.metadata.created_at = existing.metadata.created_at;
            }
            secret.metadata.updated_at = Utc::now();
            vault.secrets.insert(name.to_string(), secret);
        }
        self.save_vault().await?;
        tracing::debug!(name, "Stored secret");
        Ok(())
    }

    /// Retrieve a secret
    pub async fn get(&self, name: &str) -> Result<Option<Secret>> {
        let vault = self.vault.read().await;
        Ok(vault.secrets.get(name).cloned())
    }

    /// Delete a secret
    pub async fn delete(&self, name: &str) -> Result<()> {
        {
            let mut vault = self.vault.write().await;
            if vault.secrets.remove(name).is_none() {
                return Err(anyhow!("Secret '{}' not found", name));
            }
        }
        self.save_vault().await?;
        tracing::debug!(name, "Deleted secret");
        Ok(())
    }

    /// Check if a secret exists
    pub async fn exists(&self, name: &str) -> bool {
        let vault = self.vault.read().await;
        vault.secrets.contains_key(name)
    }

    /// Store a simple API key
    pub async fn set_api_key(&self, name: &str, value: &str, provider: Option<&str>) -> Result<()> {
        let secret = match provider {
            Some(p) => Secret::with_provider(value, p),
            None => Secret::new(value),
        };
        self.set(name, secret).await
    }

    /// Get just the secret value
    pub async fn get_value(&self, name: &str) -> Result<Option<String>> {
        Ok(self.get(name).await?.map(|s| s.value))
    }

    // === Common Secrets ===

    /// Ensure all required secrets exist, creating them if needed
    pub async fn ensure_secrets(&self) -> Result<()> {
        self.ensure_jwt_signing_key().await?;
        self.ensure_api_key_secret().await?;
        Ok(())
    }

    /// Get the JWT signing key, creating it if it doesn't exist
    pub async fn get_jwt_signing_key(&self) -> Result<Vec<u8>> {
        match self.get_value(SECRET_KEY_JWT_SIGNING).await {
            Ok(Some(key_hex)) => {
                if let Ok(key) = crypto::decode_hex(&key_hex)
                    && key.len() == 32
                {
                    return Ok(key);
                }
                tracing::warn!("Stored JWT signing key has invalid format, regenerating");
                self.create_jwt_signing_key().await
            }
            Ok(None) => self.create_jwt_signing_key().await,
            Err(e) => {
                tracing::warn!("Failed to read JWT signing key: {}, regenerating", e);
                self.create_jwt_signing_key().await
            }
        }
    }

    /// Ensure JWT signing key exists
    async fn ensure_jwt_signing_key(&self) -> Result<()> {
        if self.exists(SECRET_KEY_JWT_SIGNING).await {
            tracing::debug!("JWT signing key exists");
            return Ok(());
        }

        self.create_jwt_signing_key().await?;
        Ok(())
    }

    /// Create a new JWT signing key
    async fn create_jwt_signing_key(&self) -> Result<Vec<u8>> {
        let key = crypto::generate_signing_key();
        let key_hex = crypto::encode_hex(&key);

        self.set_api_key(SECRET_KEY_JWT_SIGNING, &key_hex, Some("sideseat"))
            .await?;
        tracing::debug!("Created new JWT signing key");

        Ok(key)
    }

    /// Get the API key HMAC secret, creating it if it doesn't exist
    pub async fn get_api_key_secret(&self) -> Result<Vec<u8>> {
        match self.get_value(SECRET_KEY_API_KEY).await {
            Ok(Some(key_hex)) => {
                if let Ok(key) = crypto::decode_hex(&key_hex)
                    && key.len() == 32
                {
                    return Ok(key);
                }
                tracing::warn!("Stored API key secret has invalid format, regenerating");
                self.create_api_key_secret().await
            }
            Ok(None) => self.create_api_key_secret().await,
            Err(e) => {
                tracing::warn!("Failed to read API key secret: {}, regenerating", e);
                self.create_api_key_secret().await
            }
        }
    }

    /// Ensure API key secret exists
    async fn ensure_api_key_secret(&self) -> Result<()> {
        if self.exists(SECRET_KEY_API_KEY).await {
            tracing::debug!("API key secret exists");
            return Ok(());
        }

        self.create_api_key_secret().await?;
        Ok(())
    }

    /// Create a new API key HMAC secret
    async fn create_api_key_secret(&self) -> Result<Vec<u8>> {
        let key = crypto::generate_signing_key(); // 32 bytes
        let key_hex = crypto::encode_hex(&key);

        self.set_api_key(SECRET_KEY_API_KEY, &key_hex, Some("sideseat"))
            .await?;
        tracing::debug!("Created new API key HMAC secret");

        Ok(key)
    }

    // === Internal Methods ===

    async fn load_vault(backend: &SecretBackend, service_name: &str) -> Result<SecretVault> {
        match backend {
            SecretBackend::File(path) => Self::load_vault_from_file(path).await,
            _ => Self::load_vault_from_keychain(service_name).await,
        }
    }

    async fn load_vault_from_file(path: &PathBuf) -> Result<SecretVault> {
        match tokio::fs::read_to_string(path).await {
            Ok(json) => {
                let vault: SecretVault =
                    serde_json::from_str(&json).context("Failed to parse secrets file")?;
                tracing::debug!(count = vault.secrets.len(), "Loaded secrets from file");
                Ok(vault)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("No existing secrets file, creating new vault");
                Ok(SecretVault::default())
            }
            Err(e) => Err(anyhow!("Failed to load secrets file: {}", e)),
        }
    }

    async fn load_vault_from_keychain(service_name: &str) -> Result<SecretVault> {
        let entry = Entry::new(service_name, VAULT_KEY)
            .map_err(|e| anyhow!("Failed to create keychain entry: {}", e))?;

        let result = tokio::task::spawn_blocking(move || entry.get_password())
            .await
            .context("Keychain task failed")?;

        match result {
            Ok(json) => {
                let vault: SecretVault =
                    serde_json::from_str(&json).context("Failed to parse vault from keychain")?;
                tracing::debug!(count = vault.secrets.len(), "Loaded secrets from keychain");
                Ok(vault)
            }
            Err(keyring::Error::NoEntry) => {
                tracing::debug!("No existing vault in keychain, creating new one");
                Ok(SecretVault::default())
            }
            Err(e) => Err(anyhow!("Failed to load from keychain: {}", e)),
        }
    }

    async fn save_vault(&self) -> Result<()> {
        // Serialize save operations to prevent race conditions
        let _guard = self.save_mutex.lock().await;
        match &self.backend {
            SecretBackend::File(path) => self.save_vault_to_file(path).await,
            _ => self.save_vault_to_keychain().await,
        }
    }

    async fn save_vault_to_file(&self, path: &PathBuf) -> Result<()> {
        let vault = self.vault.read().await;
        let json = serde_json::to_string_pretty(&*vault).context("Failed to serialize vault")?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("Failed to create secrets directory")?;
        }

        tokio::fs::write(path, json)
            .await
            .context("Failed to write secrets file")?;
        Ok(())
    }

    async fn save_vault_to_keychain(&self) -> Result<()> {
        let vault = self.vault.read().await;
        let json = serde_json::to_string(&*vault).context("Failed to serialize vault")?;

        let service_name = self.service_name.clone();
        let entry = Entry::new(&service_name, VAULT_KEY)
            .map_err(|e| anyhow!("Failed to create keychain entry: {}", e))?;

        tokio::task::spawn_blocking(move || entry.set_password(&json))
            .await
            .context("Keychain task failed")?
            .map_err(|e| anyhow!("Failed to save to keychain: {}", e))?;

        Ok(())
    }

    async fn detect_backend(storage: &AppStorage) -> SecretBackend {
        if let Ok(override_backend) = std::env::var(ENV_SECRET_BACKEND)
            && let Some(backend) = Self::parse_backend_override(&override_backend, storage)
        {
            return backend;
        }
        Self::detect_platform_backend().await
    }

    fn parse_backend_override(value: &str, storage: &AppStorage) -> Option<SecretBackend> {
        match value.to_lowercase().as_str() {
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

            other => {
                tracing::warn!(
                    "Invalid secret backend '{}'. Valid: {}",
                    other,
                    Self::valid_backend_options()
                );
                None
            }
        }
    }

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

    async fn detect_platform_backend() -> SecretBackend {
        #[cfg(target_os = "macos")]
        {
            SecretBackend::AppleKeychain
        }

        #[cfg(target_os = "windows")]
        {
            SecretBackend::WindowsCredential
        }

        #[cfg(target_os = "linux")]
        {
            Self::detect_linux_backend().await
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            SecretBackend::LinuxKeyutils
        }
    }

    #[cfg(target_os = "linux")]
    async fn detect_linux_backend() -> SecretBackend {
        let test_result = tokio::task::spawn_blocking(|| {
            match Entry::new(SECRET_SERVICE_NAME, "__sideseat_backend_test__") {
                Ok(entry) => matches!(entry.get_password(), Err(keyring::Error::NoEntry) | Ok(_)),
                Err(_) => false,
            }
        })
        .await;

        match test_result {
            Ok(true) => {
                tracing::debug!("Secret Service available");
                SecretBackend::LinuxSecretService
            }
            _ => {
                tracing::debug!("Secret Service unavailable, using keyutils");
                SecretBackend::LinuxKeyutils
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_debug_redacts_value() {
        let secret = Secret::new("super-secret-key");
        let debug = format!("{:?}", secret);
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret-key"));
    }

    #[test]
    fn test_backend_persistence() {
        assert!(SecretBackend::AppleKeychain.is_persistent());
        assert!(SecretBackend::WindowsCredential.is_persistent());
        assert!(SecretBackend::LinuxSecretService.is_persistent());
        assert!(!SecretBackend::LinuxKeyutils.is_persistent());
        assert!(SecretBackend::File(PathBuf::from("/tmp/secrets.json")).is_persistent());
    }

    #[test]
    fn test_backend_is_file() {
        assert!(!SecretBackend::AppleKeychain.is_file());
        assert!(SecretBackend::File(PathBuf::from("/tmp/secrets.json")).is_file());
    }

    #[test]
    fn test_secret_with_provider() {
        let secret = Secret::with_provider("my-key", "openai");
        assert_eq!(secret.value, "my-key");
        assert_eq!(secret.metadata.provider, Some("openai".to_string()));
    }

    #[test]
    fn test_vault_serialization() {
        let mut vault = SecretVault::default();
        vault
            .secrets
            .insert("key1".to_string(), Secret::new("value1"));

        let json = serde_json::to_string(&vault).unwrap();
        let deserialized: SecretVault = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.secrets.len(), 1);
        assert_eq!(deserialized.secrets.get("key1").unwrap().value, "value1");
    }

    #[test]
    fn test_valid_backend_options() {
        let options = SecretManager::valid_backend_options();
        assert!(!options.is_empty());
        assert!(options.contains("file"));
    }
}
