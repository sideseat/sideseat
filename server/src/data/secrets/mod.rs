//! Multi-backend secret manager with scoping
//!
//! Supports local (keychain/file), environment variables, AWS Secrets Manager,
//! and HashiCorp Vault backends. Secrets are scoped (global, org, project, user).

mod aws;
mod cached;
mod env;
mod error;
mod file;
mod hashicorp;
mod keyring;
mod provider;
mod types;

pub use error::SecretError;
pub use types::{Secret, SecretKey, SecretScope};

use provider::SecretProvider;

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::core::config::{SecretsBackend, SecretsConfig};
use crate::core::constants::{SECRET_KEY_API_KEY, SECRET_KEY_JWT_SIGNING};
use crate::core::storage::AppStorage;
use crate::utils::crypto;

#[derive(Debug, Clone)]
pub struct SecretManager {
    provider: Arc<dyn SecretProvider>,
}

impl SecretManager {
    /// Initialize from config. Constructs the appropriate provider.
    pub async fn init(storage: &AppStorage, config: &SecretsConfig) -> Result<Self> {
        let provider: Arc<dyn SecretProvider> = match config.backend {
            SecretsBackend::File => Arc::new(file::FileProvider::init(storage.data_dir()).await?),
            SecretsBackend::Keychain
            | SecretsBackend::CredentialManager
            | SecretsBackend::SecretService
            | SecretsBackend::Keyutils => {
                match keyring::KeyringProvider::init(config.backend).await {
                    Ok(p) => Arc::new(p),
                    Err(e) if config.backend == SecretsBackend::SecretService => {
                        tracing::warn!(
                            error = %e,
                            "Secret Service unavailable, falling back to file-based storage"
                        );
                        Arc::new(file::FileProvider::init(storage.data_dir()).await?)
                    }
                    Err(e) => return Err(e),
                }
            }
            SecretsBackend::Env => {
                let prefix = config
                    .env
                    .as_ref()
                    .map(|e| e.prefix.clone())
                    .unwrap_or_else(|| {
                        crate::core::constants::SECRETS_DEFAULT_ENV_PREFIX.to_string()
                    });
                Arc::new(env::EnvProvider::new(prefix))
            }
            SecretsBackend::Aws => {
                let aws_cfg = config.aws.as_ref().context("AWS secrets config missing")?;
                let p = aws::AwsProvider::new(
                    aws_cfg.region.clone(),
                    aws_cfg.prefix.clone(),
                    aws_cfg.recovery_window_days,
                )
                .await?;
                Arc::new(cached::CachedProvider::new(Arc::new(p)))
            }
            SecretsBackend::Vault => {
                let v = config
                    .vault
                    .as_ref()
                    .context("Vault secrets config missing")?;
                let p = hashicorp::HashiVaultProvider::new(
                    v.address.clone(),
                    &v.token,
                    v.mount.clone(),
                    v.prefix.clone(),
                )?;
                Arc::new(cached::CachedProvider::new(Arc::new(p)))
            }
        };

        if provider.is_read_only() {
            tracing::warn!(
                backend = provider.name(),
                "Secret backend is read-only. Auto-generated secrets (JWT key, API key) must be pre-configured."
            );
        } else if !provider.is_persistent() {
            tracing::warn!(
                backend = provider.name(),
                "Secret backend is session-scoped. Secrets won't persist across reboots."
            );
        }

        tracing::debug!(backend = provider.name(), "Secret manager initialized");
        Ok(Self { provider })
    }

    // -- Scoped API --

    pub async fn get_scoped(&self, key: &SecretKey) -> Result<Option<Secret>> {
        self.provider.get(key).await.map_err(Into::into)
    }

    pub async fn set_scoped(&self, key: &SecretKey, secret: Secret) -> Result<()> {
        self.provider.set(key, &secret).await.map_err(Into::into)
    }

    pub async fn delete_scoped(&self, key: &SecretKey) -> Result<()> {
        self.provider.delete(key).await.map_err(Into::into)
    }

    /// Try scopes in order, return first match
    pub async fn get_with_fallback(
        &self,
        name: &str,
        scopes: &[SecretScope],
    ) -> Result<Option<Secret>> {
        for scope in scopes {
            let key = SecretKey::new(name, scope.clone());
            if let Some(secret) = self.get_scoped(&key).await? {
                return Ok(Some(secret));
            }
        }
        Ok(None)
    }

    // -- Global convenience (backward compat) --

    pub async fn get(&self, name: &str) -> Result<Option<Secret>> {
        self.get_scoped(&SecretKey::global(name)).await
    }

    pub async fn set(&self, name: &str, secret: Secret) -> Result<()> {
        self.set_scoped(&SecretKey::global(name), secret).await
    }

    pub async fn get_value(&self, name: &str) -> Result<Option<String>> {
        Ok(self.get(name).await?.map(|s| s.value))
    }

    pub async fn set_api_key(&self, name: &str, value: &str) -> Result<()> {
        self.set(name, Secret::new(value)).await
    }

    pub async fn exists(&self, name: &str) -> bool {
        self.provider
            .exists(&SecretKey::global(name))
            .await
            .unwrap_or(false)
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        self.delete_scoped(&SecretKey::global(name)).await
    }

    // -- Org-scoped secret operations --

    pub async fn set_org_api_key(&self, org_id: &str, name: &str, value: &str) -> Result<()> {
        let key = SecretKey::new(name, SecretScope::org(org_id));
        self.set_scoped(&key, Secret::new(value)).await
    }

    pub async fn get_org_api_key(&self, org_id: &str, name: &str) -> Result<Option<String>> {
        let key = SecretKey::new(name, SecretScope::org(org_id));
        Ok(self.get_scoped(&key).await?.map(|s| s.value))
    }

    pub async fn list_org_secrets(&self, org_id: &str) -> Result<Vec<SecretKey>> {
        let scope = SecretScope::org(org_id);
        self.provider.list(&scope).await.map_err(Into::into)
    }

    pub async fn delete_org_secret(&self, org_id: &str, name: &str) -> Result<()> {
        let key = SecretKey::new(name, SecretScope::org(org_id));
        self.delete_scoped(&key).await
    }

    // -- Internal secrets (global scope) --

    /// Ensure all required secrets exist, creating them if needed.
    /// On read-only backends, verifies they exist and fails with clear error if not.
    pub async fn ensure_secrets(&self) -> Result<()> {
        if self.provider.is_read_only() {
            // Use get_scoped (not exists()) so backend errors propagate instead of
            // being swallowed as "missing secret"
            let jwt_exists = self
                .get_scoped(&SecretKey::global(SECRET_KEY_JWT_SIGNING))
                .await?
                .is_some();
            let api_exists = self
                .get_scoped(&SecretKey::global(SECRET_KEY_API_KEY))
                .await?
                .is_some();
            if !jwt_exists || !api_exists {
                let missing: Vec<&str> = [
                    (!jwt_exists).then_some(SECRET_KEY_JWT_SIGNING),
                    (!api_exists).then_some(SECRET_KEY_API_KEY),
                ]
                .into_iter()
                .flatten()
                .collect();
                anyhow::bail!(
                    "Secret backend '{}' is read-only. Required secrets missing: {}. \
                     Pre-configure these before starting the server.",
                    self.provider.name(),
                    missing.join(", ")
                );
            }
            return Ok(());
        }
        self.ensure_jwt_signing_key().await?;
        self.ensure_api_key_secret().await?;
        Ok(())
    }

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

    // -- Health check task --

    pub fn start_health_check_task(
        &self,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let provider = Arc::clone(&self.provider);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("Secret health check task shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        if let Err(e) = provider.health_check().await {
                            tracing::warn!(error = %e, "Secret backend health check failed");
                        }
                    }
                }
            }
        })
    }

    // -- Private helpers --

    async fn ensure_jwt_signing_key(&self) -> Result<()> {
        if self.exists(SECRET_KEY_JWT_SIGNING).await {
            tracing::debug!("JWT signing key exists");
            return Ok(());
        }
        self.create_jwt_signing_key().await?;
        Ok(())
    }

    async fn create_jwt_signing_key(&self) -> Result<Vec<u8>> {
        let key = crypto::generate_signing_key();
        let key_hex = crypto::encode_hex(&key);
        self.set_api_key(SECRET_KEY_JWT_SIGNING, &key_hex).await?;
        tracing::debug!("Created new JWT signing key");
        Ok(key)
    }

    async fn ensure_api_key_secret(&self) -> Result<()> {
        if self.exists(SECRET_KEY_API_KEY).await {
            tracing::debug!("API key secret exists");
            return Ok(());
        }
        self.create_api_key_secret().await?;
        Ok(())
    }

    async fn create_api_key_secret(&self) -> Result<Vec<u8>> {
        let key = crypto::generate_signing_key();
        let key_hex = crypto::encode_hex(&key);
        self.set_api_key(SECRET_KEY_API_KEY, &key_hex).await?;
        tracing::debug!("Created new API key HMAC secret");
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::AppStorage;

    async fn test_manager(dir: &tempfile::TempDir) -> SecretManager {
        let storage = AppStorage::init_for_test(dir.path().to_path_buf());
        let config = SecretsConfig {
            backend: SecretsBackend::File,
            env: None,
            aws: None,
            vault: None,
        };
        SecretManager::init(&storage, &config).await.unwrap()
    }

    #[tokio::test]
    async fn test_global_convenience() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = test_manager(&dir).await;

        mgr.set("test_key", Secret::new("val")).await.unwrap();
        assert_eq!(mgr.get_value("test_key").await.unwrap().unwrap(), "val");
        assert!(mgr.exists("test_key").await);

        mgr.delete("test_key").await.unwrap();
        assert!(!mgr.exists("test_key").await);
    }

    #[tokio::test]
    async fn test_org_scoped_methods() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = test_manager(&dir).await;

        mgr.set_org_api_key("acme", "openai_key", "sk-123")
            .await
            .unwrap();
        assert_eq!(
            mgr.get_org_api_key("acme", "openai_key")
                .await
                .unwrap()
                .unwrap(),
            "sk-123"
        );

        let keys = mgr.list_org_secrets("acme").await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].to_string(), "org/acme/openai_key");

        mgr.delete_org_secret("acme", "openai_key").await.unwrap();
        assert!(
            mgr.get_org_api_key("acme", "openai_key")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_get_with_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = test_manager(&dir).await;

        // Set only at global scope
        mgr.set("api_key", Secret::new("global_val")).await.unwrap();

        // Fallback: org first, then global
        let result = mgr
            .get_with_fallback(
                "api_key",
                &[SecretScope::org("acme"), SecretScope::global()],
            )
            .await
            .unwrap();
        assert_eq!(result.unwrap().value, "global_val");

        // Set at org scope — should take priority
        mgr.set_org_api_key("acme", "api_key", "org_val")
            .await
            .unwrap();
        let result = mgr
            .get_with_fallback(
                "api_key",
                &[SecretScope::org("acme"), SecretScope::global()],
            )
            .await
            .unwrap();
        assert_eq!(result.unwrap().value, "org_val");
    }

    #[tokio::test]
    async fn test_ensure_secrets_creates_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = test_manager(&dir).await;

        mgr.ensure_secrets().await.unwrap();
        assert!(mgr.exists(SECRET_KEY_JWT_SIGNING).await);
        assert!(mgr.exists(SECRET_KEY_API_KEY).await);
    }

    #[test]
    fn test_backend_detection() {
        let backend = SecretsBackend::detect();
        assert!(backend.is_vault_based());
    }

    #[tokio::test]
    async fn test_jwt_and_api_key_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = test_manager(&dir).await;

        let jwt_key = mgr.get_jwt_signing_key().await.unwrap();
        assert_eq!(jwt_key.len(), 32);

        // Second call should return same key
        let jwt_key2 = mgr.get_jwt_signing_key().await.unwrap();
        assert_eq!(jwt_key, jwt_key2);

        let api_key = mgr.get_api_key_secret().await.unwrap();
        assert_eq!(api_key.len(), 32);
    }
}
