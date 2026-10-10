//! Multi-backend secret manager with scoping
//!
//! Supports local (keychain/file), environment variables, AWS Secrets Manager,
//! and HashiCorp Vault backends. Secrets are scoped (global, org, project, user).

// A map's iteration order differs from one process to the next: where it reaches stored or answered bytes, a
// hash or the order of a write, iterate in order; elsewhere say why order cannot matter.
#![deny(clippy::iter_over_hash_type)]

mod aws;
mod cached;
mod env;
mod error;
mod file;
mod hashicorp;
mod keyring;
mod provider;
mod types;

use provider::SecretProvider;
use types::{Secret, SecretKey, SecretScope};

use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use sideseat_core::config::{SecretsBackend, SecretsConfig};
use sideseat_core::constants::{SECRET_KEY_API_KEY, SECRET_KEY_JWT_SIGNING};
use sideseat_core::storage::AppStorage;
use sideseat_core::utils::crypto;
use sideseat_ports::clock::Clock;

pub use error::SecretError;

/// Every root secret is a 256-bit key: the JWT signing key and the API-key pepper are both HMAC-SHA256
/// inputs.
const ROOT_SECRET_BYTES: usize = 32;

#[cfg(test)]
#[derive(Debug)]
struct TestClock(chrono::DateTime<chrono::Utc>);

#[cfg(test)]
impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.0
    }
}

#[cfg(test)]
fn test_clock() -> Arc<dyn Clock> {
    test_clock_at(chrono::DateTime::UNIX_EPOCH)
}

#[cfg(test)]
fn test_clock_at(now: chrono::DateTime<chrono::Utc>) -> Arc<dyn Clock> {
    Arc::new(TestClock(now))
}

#[cfg(test)]
fn test_secret(value: impl Into<String>) -> Secret {
    Secret::new(value, chrono::DateTime::UNIX_EPOCH)
}

#[derive(Debug, Clone)]
pub struct SecretManager {
    provider: Arc<dyn SecretProvider>,
    clock: Arc<dyn Clock>,
}

impl SecretManager {
    /// Initialize from config. Constructs the appropriate provider.
    pub async fn init(
        storage: &AppStorage,
        config: &SecretsConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, SecretError> {
        let provider: Arc<dyn SecretProvider> = match config.backend {
            SecretsBackend::File => {
                Arc::new(file::FileProvider::init(storage.data_dir(), Arc::clone(&clock)).await?)
            }
            SecretsBackend::Keychain
            | SecretsBackend::CredentialManager
            | SecretsBackend::SecretService
            | SecretsBackend::Keyutils => {
                match keyring::KeyringProvider::init(config.backend, Arc::clone(&clock)).await {
                    Ok(p) => Arc::new(p),
                    Err(e) if config.backend == SecretsBackend::SecretService => {
                        tracing::warn!(
                            error = %e,
                            "Secret Service unavailable, falling back to file-based storage"
                        );
                        Arc::new(
                            file::FileProvider::init(storage.data_dir(), Arc::clone(&clock))
                                .await?,
                        )
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
                        sideseat_core::constants::SECRETS_DEFAULT_ENV_PREFIX.to_string()
                    });
                Arc::new(env::EnvProvider::new(prefix, Arc::clone(&clock)))
            }
            SecretsBackend::Aws => {
                let aws_cfg = config
                    .aws
                    .as_ref()
                    .ok_or_else(|| SecretError::Config("AWS secrets config missing".into()))?;
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
                    .ok_or_else(|| SecretError::Config("Vault secrets config missing".into()))?;
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
        Ok(Self { provider, clock })
    }

    // -- Scoped API --

    async fn get_scoped(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        self.provider.get(key).await
    }

    async fn set_scoped(&self, key: &SecretKey, secret: Secret) -> Result<(), SecretError> {
        self.provider.set(key, &secret).await
    }

    async fn set_scoped_value(
        &self,
        key: &SecretKey,
        value: impl Into<String>,
    ) -> Result<(), SecretError> {
        self.set_scoped(key, Secret::new(value, self.clock.now()))
            .await
    }

    async fn delete_scoped(&self, key: &SecretKey) -> Result<(), SecretError> {
        self.provider.delete(key).await
    }

    async fn get_value(&self, name: &str) -> Result<Option<String>, SecretError> {
        Ok(self
            .get_scoped(&SecretKey::global(name))
            .await?
            .map(|secret| secret.value))
    }

    // -- Internal secrets (global scope) --

    /// Ensure all required secrets exist, creating them if needed.
    /// On read-only backends, verifies they exist and fails with clear error if not.
    pub async fn ensure_secrets(&self) -> Result<(), SecretError> {
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
                return Err(SecretError::ReadOnlyMissingSecrets {
                    backend: self.provider.name(),
                    missing: missing.join(", "),
                });
            }
            return Ok(());
        }
        self.ensure_jwt_signing_key().await?;
        self.ensure_api_key_secret().await?;
        Ok(())
    }

    pub async fn get_jwt_signing_key(&self) -> Result<Vec<u8>, SecretError> {
        self.get_or_create_root_secret(
            SECRET_KEY_JWT_SIGNING,
            "JWT signing key",
            "every issued \
             session token is rejected and users must sign in again",
        )
        .await
    }

    pub async fn get_api_key_secret(&self) -> Result<Vec<u8>, SecretError> {
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
        key: &'static str,
        label: &'static str,
        consequence: &'static str,
    ) -> Result<Vec<u8>, SecretError> {
        match self.get_value(key).await {
            Ok(Some(value_hex)) => {
                if let Ok(secret) = crypto::decode_hex(&value_hex)
                    && secret.len() == ROOT_SECRET_BYTES
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
            Err(source) => Err(SecretError::RootSecretUnreadable {
                label,
                backend: self.provider.name(),
                consequence,
                source: Box::new(source),
            }),
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
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
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

    async fn ensure_jwt_signing_key(&self) -> Result<(), SecretError> {
        self.ensure_root_secret(SECRET_KEY_JWT_SIGNING, "JWT signing key")
            .await
    }

    /// Provision a root secret if the backend reports it absent, and fail if it cannot say.
    ///
    /// `exists` answers `false` for both "not stored" and "could not tell" (`unwrap_or(false)`), and this
    /// path runs at startup *before* anything reads the secret - so a backend that was merely unreachable
    /// made the server generate a replacement and overwrite the live one, which is the loss
    /// `get_or_create_root_secret` refuses. Asking the provider directly keeps the two answers apart.
    async fn ensure_root_secret(
        &self,
        key: &'static str,
        label: &'static str,
    ) -> Result<(), SecretError> {
        let present = self
            .provider
            .exists(&SecretKey::global(key))
            .await
            .map_err(|source| SecretError::RootSecretPresenceUnknown {
                label,
                backend: self.provider.name(),
                source: Box::new(source),
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
    async fn create_root_secret(&self, key: &'static str) -> Result<Vec<u8>, SecretError> {
        let proposed = crypto::generate_signing_key();
        let stored = self
            .provider
            .create_if_absent(
                &SecretKey::global(key),
                &Secret::new(crypto::encode_hex(&proposed), self.clock.now()),
            )
            .await?;
        let decoded = crypto::decode_hex(&stored.value)
            .map_err(|source| SecretError::RootSecretNotHex { key, source })?;
        if decoded.len() != ROOT_SECRET_BYTES {
            return Err(SecretError::RootSecretWrongLength {
                key,
                expected: ROOT_SECRET_BYTES,
                stored: decoded.len(),
            });
        }
        tracing::debug!(secret = key, "Root secret is provisioned");
        Ok(decoded)
    }

    async fn ensure_api_key_secret(&self) -> Result<(), SecretError> {
        self.ensure_root_secret(SECRET_KEY_API_KEY, "API key secret")
            .await
    }
}

/// The secret-writing port, implemented over the real manager.
///
/// The port takes a string key; this maps it onto the scoped key the backends use, which is where the knowledge
/// of *how* a backend addresses a secret belongs.
#[async_trait::async_trait]
impl sideseat_ports::secrets::SecretWriter for SecretManager {
    async fn put(&self, key: &str, value: &str) -> Result<(), String> {
        self.set_scoped_value(&SecretKey::global(key), value)
            .await
            .map_err(|e| e.to_string())
    }

    async fn remove(&self, key: &str) -> Result<(), String> {
        self.delete_scoped(&SecretKey::global(key))
            .await
            .map_err(|e| e.to_string())
    }
}

#[async_trait::async_trait]
impl sideseat_ports::secrets::CredentialSecretStore for SecretManager {
    async fn get_credential_secret(
        &self,
        organization_id: &str,
        credential_id: &str,
    ) -> Result<Option<String>, String> {
        let key = SecretKey::new(
            format!(
                "{}{}",
                sideseat_core::constants::CRED_SECRET_PREFIX,
                credential_id
            ),
            SecretScope::org(organization_id),
        );
        self.get_scoped(&key)
            .await
            .map(|secret| secret.map(|secret| secret.value))
            .map_err(|error| error.to_string())
    }

    async fn put_credential_secret(
        &self,
        organization_id: &str,
        credential_id: &str,
        value: &str,
    ) -> Result<(), String> {
        let key = SecretKey::new(
            format!(
                "{}{}",
                sideseat_core::constants::CRED_SECRET_PREFIX,
                credential_id
            ),
            SecretScope::org(organization_id),
        );
        self.set_scoped_value(&key, value)
            .await
            .map_err(|error| error.to_string())
    }

    async fn delete_credential_secret(
        &self,
        organization_id: &str,
        credential_id: &str,
    ) -> Result<(), String> {
        let key = SecretKey::new(
            format!(
                "{}{}",
                sideseat_core::constants::CRED_SECRET_PREFIX,
                credential_id
            ),
            SecretScope::org(organization_id),
        );
        self.delete_scoped(&key)
            .await
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideseat_core::storage::AppStorage;

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
            Ok(self.stored.lock().clone().map(test_secret))
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
            clock: test_clock(),
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

    /// The two refusals above are now variants rather than formatted strings, and both still say the
    /// whole sentence an operator needs *and* keep the backend's own error reachable as the cause.
    #[tokio::test]
    async fn an_unreadable_backend_is_reported_as_itself_with_its_cause() {
        let provider = Arc::new(FlakyProvider::default());
        let mgr = SecretManager {
            provider: Arc::clone(&provider) as Arc<dyn SecretProvider>,
            clock: test_clock(),
        };
        provider.reads_fail.store(true, Ordering::SeqCst);

        let read = mgr.get_api_key_secret().await.unwrap_err();
        assert!(matches!(read, SecretError::RootSecretUnreadable { .. }));
        assert_eq!(
            read.to_string(),
            "could not read the API key secret from the flaky secrets backend: Secret backend \
             error (flaky): backend unreachable. Refusing to start: generating a replacement would \
             mean every stored API key hash becomes unverifiable and authenticated ingestion fails \
             with 401. Restore access to the backend, or set the secret explicitly."
        );
        assert!(std::error::Error::source(&read).is_some());

        let presence = mgr.ensure_secrets().await.unwrap_err();
        assert!(matches!(
            presence,
            SecretError::RootSecretPresenceUnknown { .. }
        ));
        assert_eq!(
            presence.to_string(),
            "could not tell whether the JWT signing key is already stored in the flaky secrets \
             backend; refusing to overwrite a secret that may exist"
        );
        assert!(std::error::Error::source(&presence).is_some());
    }

    /// The wording `anyhow::bail!` produced for the refusals that have no reachable backend fixture.
    #[test]
    fn provisioning_refusal_messages_are_unchanged() {
        assert_eq!(
            SecretError::ReadOnlyMissingSecrets {
                backend: "env",
                missing: "jwt_signing_key, api_key_secret".to_string(),
            }
            .to_string(),
            "Secret backend 'env' is read-only. Required secrets missing: jwt_signing_key, \
             api_key_secret. Pre-configure these before starting the server."
        );
        assert_eq!(
            SecretError::RootSecretNotHex {
                key: SECRET_KEY_JWT_SIGNING,
                source: sideseat_core::utils::crypto::HexDecodeError::OddLength,
            }
            .to_string(),
            format!("secret {SECRET_KEY_JWT_SIGNING} is not valid hex")
        );
        assert_eq!(
            SecretError::RootSecretWrongLength {
                key: SECRET_KEY_JWT_SIGNING,
                expected: ROOT_SECRET_BYTES,
                stored: 16,
            }
            .to_string(),
            format!("secret {SECRET_KEY_JWT_SIGNING} is not 32 bytes (16 stored)")
        );
    }

    async fn test_manager(dir: &tempfile::TempDir) -> SecretManager {
        let storage = AppStorage::init_for_test(dir.path().to_path_buf());
        let config = SecretsConfig {
            backend: SecretsBackend::File,
            env: None,
            aws: None,
            vault: None,
        };
        SecretManager::init(&storage, &config, test_clock())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_ensure_secrets_creates_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = test_manager(&dir).await;

        mgr.ensure_secrets().await.unwrap();
        assert!(
            mgr.get_scoped(&SecretKey::global(SECRET_KEY_JWT_SIGNING))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            mgr.get_scoped(&SecretKey::global(SECRET_KEY_API_KEY))
                .await
                .unwrap()
                .is_some()
        );
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
