use std::sync::Arc;

use async_trait::async_trait;
use sideseat_ports::clock::Clock;

use super::error::SecretError;
use super::provider::SecretProvider;
use super::types::Secret;
use super::types::SecretKey;

#[derive(Debug)]
pub struct EnvProvider {
    prefix: String,
    clock: Arc<dyn Clock>,
}

impl EnvProvider {
    pub fn new(prefix: String, clock: Arc<dyn Clock>) -> Self {
        Self { prefix, clock }
    }

    fn key_to_env_var(&self, key: &SecretKey) -> String {
        let path = key.to_string().to_uppercase().replace(['/', '-'], "_");
        format!("{}{}", self.prefix, path)
    }

    fn read_with(
        &self,
        key: &SecretKey,
        read: impl FnOnce(&str) -> Result<String, std::env::VarError>,
    ) -> Result<Option<Secret>, SecretError> {
        let var = self.key_to_env_var(key);
        match read(&var) {
            Ok(value) => Ok(Some(Secret::new(value, self.clock.now()))),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(e) => Err(SecretError::backend(
                "env",
                format!("failed to read {var}: {e}"),
            )),
        }
    }
}

#[async_trait]
impl SecretProvider for EnvProvider {
    async fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        self.read_with(key, |name| std::env::var(name))
    }

    async fn set(&self, _key: &SecretKey, _secret: &Secret) -> Result<(), SecretError> {
        Err(SecretError::ReadOnly { backend: "env" })
    }

    async fn delete(&self, _key: &SecretKey) -> Result<(), SecretError> {
        Err(SecretError::ReadOnly { backend: "env" })
    }

    fn name(&self) -> &'static str {
        "Environment Variables"
    }
    fn is_persistent(&self) -> bool {
        false
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SecretScope;
    use crate::{test_clock, test_secret};

    #[test]
    fn test_key_to_env_var() {
        let provider = EnvProvider::new("SIDESEAT_SECRET_".to_string(), test_clock());

        assert_eq!(
            provider.key_to_env_var(&SecretKey::global("jwt_signing_key")),
            "SIDESEAT_SECRET_GLOBAL_JWT_SIGNING_KEY"
        );
        assert_eq!(
            provider.key_to_env_var(&SecretKey::new("api_key", SecretScope::org("acme"))),
            "SIDESEAT_SECRET_ORG_ACME_API_KEY"
        );
        assert_eq!(
            provider.key_to_env_var(&SecretKey::new("token", SecretScope::project("my-proj"))),
            "SIDESEAT_SECRET_PROJECT_MY_PROJ_TOKEN"
        );
        assert_eq!(
            provider.key_to_env_var(&SecretKey::new("pref", SecretScope::user("u1"))),
            "SIDESEAT_SECRET_USER_U1_PREF"
        );
    }

    #[test]
    fn test_read_with_returns_secret() {
        let provider = EnvProvider::new("TEST_SECRET_".to_string(), test_clock());
        let key = SecretKey::global("test_key");

        let result = provider
            .read_with(&key, |name| {
                assert_eq!(name, "TEST_SECRET_GLOBAL_TEST_KEY");
                Ok("secret_value".to_string())
            })
            .unwrap();
        let secret = result.unwrap();
        assert_eq!(secret.value, "secret_value");
        assert_eq!(secret.metadata.created_at, chrono::DateTime::UNIX_EPOCH);
        assert_eq!(secret.metadata.updated_at, chrono::DateTime::UNIX_EPOCH);
    }

    #[test]
    fn test_read_with_returns_none_for_missing_variable() {
        let provider = EnvProvider::new("TEST_MISSING_SECRET_".to_string(), test_clock());
        let key = SecretKey::global("nonexistent");
        let result = provider
            .read_with(&key, |_| Err(std::env::VarError::NotPresent))
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_set_returns_read_only() {
        let provider = EnvProvider::new("TEST_SECRET_".to_string(), test_clock());
        let key = SecretKey::global("test");
        let result = provider.set(&key, &test_secret("val")).await;
        assert!(matches!(result, Err(SecretError::ReadOnly { .. })));
    }

    #[tokio::test]
    async fn test_delete_returns_read_only() {
        let provider = EnvProvider::new("TEST_SECRET_".to_string(), test_clock());
        let key = SecretKey::global("test");
        let result = provider.delete(&key).await;
        assert!(matches!(result, Err(SecretError::ReadOnly { .. })));
    }

    #[test]
    fn test_properties() {
        let provider = EnvProvider::new("TEST_".to_string(), test_clock());
        assert_eq!(provider.name(), "Environment Variables");
        assert!(!provider.is_persistent());
        assert!(provider.is_read_only());
    }
}
