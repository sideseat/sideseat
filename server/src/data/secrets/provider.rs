use async_trait::async_trait;

use super::error::SecretError;
use super::types::{Secret, SecretKey, SecretScope};

#[async_trait]
pub trait SecretProvider: Send + Sync + std::fmt::Debug {
    /// Retrieve a secret by key
    async fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError>;

    /// Store a secret
    async fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretError>;

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
