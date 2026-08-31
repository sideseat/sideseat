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
        self.get_or_create_root_secret(
            SECRET_KEY_JWT_SIGNING,
            "JWT signing key",
            "every issued \
             session token is rejected and users must sign in again",
        )
        .await
    }

    pub async fn get_api_key_secret(&self) -> Result<Vec<u8>> {
        self.get_or_create_root_secret(
            SECRET_KEY_API_KEY,
            "API key secret",
            "every stored API key \
             hash becomes unverifiable and authenticated ingestion fails with 401",
        )
        .await
    }

    /// Read a 32-byte root secret, creating one only when the backend says there is none.
    ///
    /// # Why a read failure is fatal rather than a regeneration
    ///
    /// These two secrets are the *only* copy of something the database's contents depend on: an API key
    /// row stores `HMAC(key, secret)` and nothing else, and a session token is only a signature. Both
    /// getters used to answer a read *error* by generating a fresh secret, which is the one response that
    /// cannot be undone - the old secret was probably still there and merely unreadable (an expired Vault
    /// token, an AWS throttle, a keychain the OS had locked), and the new one silently invalidated every
    /// key and session in a database shared with every other instance. A backend that cannot be read is a
    /// reason not to start; it is never evidence that a secret is absent.
    ///
    /// `Ok(None)` is different in kind - the backend answered, and its answer was that nothing is stored -
    /// so first start still provisions itself with no operator step.
    ///
    /// A stored value that is present but malformed is regenerated, because no amount of retrying will
    /// repair it, but at `error!` and saying what it costs: an operator who sees this has lost their keys
    /// and needs to know now rather than from a user's 401.
    async fn get_or_create_root_secret(
        &self,
        key: &str,
        label: &str,
        consequence: &str,
    ) -> Result<Vec<u8>> {
        match self.get_value(key).await {
            Ok(Some(value_hex)) => {
                if let Ok(secret) = crypto::decode_hex(&value_hex)
                    && secret.len() == 32
                {
                    return Ok(secret);
                }
                tracing::error!(
                    secret = key,
                    "Stored {label} is present but malformed; generating a new one, after which {consequence}"
                );
                self.create_root_secret(key).await
            }
            Ok(None) => self.create_root_secret(key).await,
            Err(e) => Err(anyhow::anyhow!(
                "could not read the {label} from the {} secrets backend: {e}. Refusing to start: \
                 generating a replacement would mean {consequence}. Restore access to the backend, or \
                 set the secret explicitly.",
                self.provider.name(),
            )),
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
        self.ensure_root_secret(SECRET_KEY_JWT_SIGNING, "JWT signing key")
            .await
    }

    /// Provision a root secret if the backend reports it absent, and fail if it cannot say.
    ///
    /// `exists` answers `false` for both "not stored" and "could not tell" (`unwrap_or(false)`), and this
    /// path runs at startup *before* anything reads the secret - so a backend that was merely unreachable
    /// made the server generate a replacement and overwrite the live one, which is the loss
    /// `get_or_create_root_secret` refuses. Asking the provider directly keeps the two answers apart.
    async fn ensure_root_secret(&self, key: &str, label: &str) -> Result<()> {
        let present = self
            .provider
            .exists(&SecretKey::global(key))
            .await
            .with_context(|| {
                format!(
                    "could not tell whether the {label} is already stored in the {} secrets backend; \
                     refusing to overwrite a secret that may exist",
                    self.provider.name()
                )
            })?;
        if present {
            tracing::debug!(secret = key, "{label} exists");
            return Ok(());
        }
        self.create_root_secret(key).await?;
        Ok(())
    }

    /// Generate a fresh 32-byte root secret and store it only if none exists.
    ///
    /// `create_if_absent` on the provider is a compare-and-set on backends that support it (AWS Secrets
    /// Manager uses `CreateSecret`, which returns `ResourceExistsException` atomically), so two fresh
    /// replicas of a horizontally-scaled deployment cannot both provision and cache different values.
    /// The winner writes; the losers read what the winner wrote. Backends without a native CAS keep the
    /// legacy exists-then-set behaviour, which is safe only for the single-instance secret stores that
    /// the shared-store rule (`validate_store_sharing`) allows here anyway.
    async fn create_root_secret(&self, key: &str) -> Result<Vec<u8>> {
        let proposed = crypto::generate_signing_key();
        let stored = self
            .provider
            .create_if_absent(
                &SecretKey::global(key),
                &Secret::new(crypto::encode_hex(&proposed)),
            )
            .await?;
        let decoded = crypto::decode_hex(&stored.value)
            .with_context(|| format!("secret {key} is not valid hex"))?;
        if decoded.len() != 32 {
            anyhow::bail!("secret {key} is not 32 bytes ({} stored)", decoded.len());
        }
        tracing::debug!(secret = key, "Root secret is provisioned");
        Ok(decoded)
    }

    async fn ensure_api_key_secret(&self) -> Result<()> {
        self.ensure_root_secret(SECRET_KEY_API_KEY, "API key secret")
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::AppStorage;

    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A provider holding one secret, whose reads can be made to fail on demand.
    ///
    /// The failure being modelled is the ordinary one: an expired Vault token, a throttled AWS call, a
    /// keychain the OS has locked. The secret is still there; this instance simply cannot see it.
    #[derive(Debug, Default)]
    struct FlakyProvider {
        stored: parking_lot::Mutex<Option<String>>,
        reads_fail: AtomicBool,
        writes: AtomicUsize,
    }

    #[async_trait]
    impl SecretProvider for FlakyProvider {
        async fn get(&self, _key: &SecretKey) -> Result<Option<Secret>, SecretError> {
            if self.reads_fail.load(Ordering::SeqCst) {
                return Err(SecretError::backend("flaky", "backend unreachable"));
            }
            Ok(self.stored.lock().clone().map(Secret::new))
        }
        async fn set(&self, _key: &SecretKey, secret: &Secret) -> Result<(), SecretError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            *self.stored.lock() = Some(secret.value.clone());
            Ok(())
        }
        async fn delete(&self, _key: &SecretKey) -> Result<(), SecretError> {
            *self.stored.lock() = None;
            Ok(())
        }
        async fn list(&self, _scope: &SecretScope) -> Result<Vec<SecretKey>, SecretError> {
            Ok(Vec::new())
        }
        fn name(&self) -> &'static str {
            "flaky"
        }
        fn is_persistent(&self) -> bool {
            true
        }
    }

    /// An unreadable backend never causes a root secret to be replaced.
    ///
    /// Both getters used to answer a read *error* by generating a fresh secret and storing it. That is the
    /// one unrecoverable response: an API key row holds `HMAC(key, secret)` and nothing else, so the
    /// overwrite invalidates every key in a database that every other instance shares - and a momentary
    /// outage was enough to trigger it. `ensure_*` had the same hole one layer earlier, through
    /// `exists()`'s `unwrap_or(false)`, and it runs first at startup.
    #[tokio::test]
    async fn an_unreadable_backend_never_replaces_a_root_secret() {
        let provider = Arc::new(FlakyProvider::default());
        let mgr = SecretManager {
            provider: Arc::clone(&provider) as Arc<dyn SecretProvider>,
        };

        // First start provisions itself: the backend answered, and said there was nothing.
        let original = mgr.get_api_key_secret().await.unwrap();
        assert_eq!(provider.writes.load(Ordering::SeqCst), 1);

        provider.reads_fail.store(true, Ordering::SeqCst);
        assert!(
            mgr.get_api_key_secret().await.is_err(),
            "an unreadable backend must be fatal, not an invitation to generate a new secret"
        );
        assert!(
            mgr.ensure_secrets().await.is_err(),
            "startup provisioning must not treat 'cannot tell' as 'absent'"
        );
        assert_eq!(
            provider.writes.load(Ordering::SeqCst),
            1,
            "nothing was written while the backend was unreadable"
        );

        // And the secret the database's hashes depend on is still the one it was.
        provider.reads_fail.store(false, Ordering::SeqCst);
        assert_eq!(
            mgr.get_api_key_secret().await.unwrap(),
            original,
            "the surviving secret still verifies every key hashed with it"
        );
        assert_eq!(provider.writes.load(Ordering::SeqCst), 1);
    }

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
