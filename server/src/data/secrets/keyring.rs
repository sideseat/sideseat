use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use keyring::Entry;
use tokio::sync::RwLock;

use crate::core::config::SecretsBackend;
use crate::core::constants::SECRET_SERVICE_NAME;

use super::error::SecretError;
use super::provider::SecretProvider;
use super::types::{Secret, SecretKey, SecretScope, SecretVault};

const VAULT_KEY: &str = "vault";

#[derive(Debug)]
pub struct KeyringProvider {
    backend: SecretsBackend,
    service_name: String,
    vault: Arc<RwLock<SecretVault>>,
    save_mutex: Arc<tokio::sync::Mutex<()>>,
}

impl KeyringProvider {
    pub async fn init(backend: SecretsBackend) -> Result<Self> {
        debug_assert!(
            backend.is_vault_based() && backend != SecretsBackend::File,
            "KeyringProvider does not handle File backend"
        );

        let service_name = SECRET_SERVICE_NAME.to_string();
        let mut vault = Self::load(&service_name).await?;

        if vault.migrate() {
            tracing::info!("Migrated secret vault from v1 to v2 (added global/ prefix)");
            Self::save_static(&service_name, &vault).await?;
        }

        Ok(Self {
            backend,
            service_name,
            vault: Arc::new(RwLock::new(vault)),
            save_mutex: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    async fn load(service_name: &str) -> Result<SecretVault> {
        let entry = Entry::new(service_name, VAULT_KEY)
            .map_err(|e| anyhow::anyhow!("Failed to create keychain entry: {}", e))?;

        let result = tokio::task::spawn_blocking(move || entry.get_password())
            .await
            .context("Keychain task failed")?;

        match result {
            Ok(json) => match serde_json::from_str::<SecretVault>(&json) {
                Ok(vault) => {
                    tracing::debug!(count = vault.secrets.len(), "Loaded secrets from keychain");
                    Ok(vault)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Corrupted vault in keychain, deleting and starting fresh"
                    );
                    let entry = Entry::new(service_name, VAULT_KEY)
                        .map_err(|e| anyhow::anyhow!("Failed to create keychain entry: {}", e))?;
                    let _ = tokio::task::spawn_blocking(move || entry.delete_credential()).await;
                    Ok(SecretVault::default())
                }
            },
            Err(keyring::Error::NoEntry) => {
                tracing::debug!("No existing vault in keychain, creating new one");
                Ok(SecretVault::default())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to load from keychain: {}", e)),
        }
    }

    async fn save(&self) -> Result<(), SecretError> {
        let _guard = self.save_mutex.lock().await;
        let backend = self.name();
        let json = {
            let vault = self.vault.read().await;
            serde_json::to_string(&*vault).map_err(|e| SecretError::Serialization(e.to_string()))?
        };
        let service_name = self.service_name.clone();
        let entry = Entry::new(&service_name, VAULT_KEY)
            .map_err(|e| SecretError::backend(backend, format!("keyring entry: {}", e)))?;

        tokio::task::spawn_blocking(move || entry.set_password(&json))
            .await
            .map_err(|e| SecretError::backend(backend, format!("task failed: {}", e)))?
            .map_err(|e| SecretError::backend(backend, format!("save failed: {}", e)))?;

        Ok(())
    }

    async fn save_static(service_name: &str, vault: &SecretVault) -> Result<()> {
        let json = serde_json::to_string(vault).context("Failed to serialize vault")?;
        let sn = service_name.to_string();
        let entry = Entry::new(&sn, VAULT_KEY)
            .map_err(|e| anyhow::anyhow!("Failed to create keychain entry: {}", e))?;
        tokio::task::spawn_blocking(move || entry.set_password(&json))
            .await
            .context("Keychain task failed")?
            .map_err(|e| anyhow::anyhow!("Failed to save to keychain: {}", e))?;
        Ok(())
    }
}

#[async_trait]
impl SecretProvider for KeyringProvider {
    async fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        let vault = self.vault.read().await;
        Ok(vault.get_secret(key))
    }

    async fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretError> {
        {
            let mut vault = self.vault.write().await;
            vault.set_secret(key, secret);
        }
        self.save().await
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        {
            let mut vault = self.vault.write().await;
            if !vault.delete_secret(key) {
                return Err(SecretError::NotFound(key.to_string()));
            }
        }
        self.save().await
    }

    async fn list(&self, scope: &SecretScope) -> Result<Vec<SecretKey>, SecretError> {
        let vault = self.vault.read().await;
        Ok(vault.list_secrets(scope))
    }

    fn name(&self) -> &'static str {
        match self.backend {
            SecretsBackend::Keychain => "macOS Keychain",
            SecretsBackend::CredentialManager => "Windows Credential Manager",
            SecretsBackend::SecretService => "Secret Service",
            SecretsBackend::Keyutils => "Linux keyutils",
            _ => unreachable!("KeyringProvider only handles keyring-based backends"),
        }
    }

    fn is_persistent(&self) -> bool {
        !matches!(self.backend, SecretsBackend::Keyutils)
    }
}
