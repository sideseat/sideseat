use async_trait::async_trait;

use super::error::SecretError;
use super::types::{Secret, SecretKey, SecretScope};

#[async_trait]
pub trait SecretProvider: Send + Sync + std::fmt::Debug {
    /// Retrieve a secret by key
    async fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError>;

    /// Store a secret
    async fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretError>;

    /// Store a secret **only if none exists**, and return the currently stored value on either outcome.
    ///
    /// The point is a *cross-instance* race: two fresh replicas of a horizontally scaled deployment both
    /// see no root secret in a shared backend, both auto-provision, and the last writer wins - so the two
    /// replicas cache different peppers and every API key verifies on one and reads as forged on the
    /// other. A backend that supports compare-and-set (AWS Secrets Manager's `CreateSecret` returns
    /// `ResourceExistsException`, Vault KV v2's `cas=0`) resolves it atomically: whichever call succeeds
    /// wrote the secret; the losers read what is now there and use it.
    ///
    /// The default is racy - it exists-then-sets - because most backends here are single-instance by
    /// design and cannot race with anyone. AWS overrides it with a real CAS. A shared backend without a
    /// CAS is documented as an incompatible pairing rather than silently pretending to be safe.
    async fn create_if_absent(
        &self,
        key: &SecretKey,
        secret: &Secret,
    ) -> Result<Secret, SecretError> {
        if let Some(existing) = self.get(key).await? {
            return Ok(existing);
        }
        self.set(key, secret).await?;
        // Read back, because on a real CAS-supporting backend the race resolved before this line and the
        // stored value is not necessarily what we wrote. This is where the racy default is racy.
        Ok(self.get(key).await?.unwrap_or_else(|| secret.clone()))
    }

    /// Delete a secret
    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError>;

    /// List all keys matching a scope
    async fn list(&self, scope: &SecretScope) -> Result<Vec<SecretKey>, SecretError>;

    /// Check if a secret exists (default: delegate to get)
    async fn exists(&self, key: &SecretKey) -> Result<bool, SecretError> {
        Ok(self.get(key).await?.is_some())
    }

    /// Human-readable backend name
    fn name(&self) -> &'static str;

    /// Whether secrets persist across reboots
    fn is_persistent(&self) -> bool;

    /// Whether backend is read-only (env provider)
    fn is_read_only(&self) -> bool {
        false
    }

    /// Health check (cloud backends validate connectivity). Default no-op.
    async fn health_check(&self) -> Result<(), SecretError> {
        Ok(())
    }
}
