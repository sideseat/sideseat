use async_trait::async_trait;

use super::error::SecretError;
use super::provider::SecretProvider;
use super::types::{Secret, SecretKey, SecretScope, SecretScopeKind};

#[derive(Debug)]
pub struct EnvProvider {
    prefix: String,
}

impl EnvProvider {
    pub fn new(prefix: String) -> Self {
        Self { prefix }
    }

    fn key_to_env_var(&self, key: &SecretKey) -> String {
        let path = key.to_string().to_uppercase().replace(['/', '-'], "_");
        format!("{}{}", self.prefix, path)
    }

    fn env_var_to_key(&self, var: &str) -> Option<SecretKey> {
        let stripped = var.strip_prefix(&self.prefix)?;
        let lower = stripped.to_lowercase();
        for scope_prefix in ["global_", "org_", "project_", "user_"] {
            if let Some(rest) = lower.strip_prefix(scope_prefix) {
                let kind_str = scope_prefix.trim_end_matches('_');
                let kind: SecretScopeKind = kind_str.parse().ok()?;
                if kind == SecretScopeKind::Global {
                    return Some(SecretKey::new(rest, SecretScope::global()));
                }
                let (id, name) = rest.split_once('_')?;
                return Some(SecretKey::new(
                    name,
                    SecretScope {
                        kind,
                        id: Some(id.to_string()),
                    },
                ));
            }
        }
        None
    }
}

#[async_trait]
impl SecretProvider for EnvProvider {
    async fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        let var = self.key_to_env_var(key);
        match std::env::var(&var) {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(e) => Err(SecretError::backend(
                "env",
                format!("failed to read {}: {}", var, e),
            )),
        }
    }

    async fn set(&self, _key: &SecretKey, _secret: &Secret) -> Result<(), SecretError> {
        Err(SecretError::ReadOnly { backend: "env" })
    }

    async fn delete(&self, _key: &SecretKey) -> Result<(), SecretError> {
        Err(SecretError::ReadOnly { backend: "env" })
    }

    async fn list(&self, scope: &SecretScope) -> Result<Vec<SecretKey>, SecretError> {
        let prefix = match &scope.id {
            None => format!("{}{}_", self.prefix, scope.kind.as_str().to_uppercase()),
            Some(id) => format!(
                "{}{}_{}_",
                self.prefix,
                scope.kind.as_str().to_uppercase(),
                id.to_uppercase(),
            ),
        };
        let keys = std::env::vars()
            .filter(|(k, _)| k.starts_with(&prefix))
            .filter_map(|(k, _)| self.env_var_to_key(&k))
            .collect();
        Ok(keys)
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

    #[test]
    fn test_key_to_env_var() {
        let provider = EnvProvider::new("SIDESEAT_SECRET_".to_string());

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
    fn test_env_var_to_key() {
        let provider = EnvProvider::new("SIDESEAT_SECRET_".to_string());

        let key = provider
            .env_var_to_key("SIDESEAT_SECRET_GLOBAL_JWT_SIGNING_KEY")
            .unwrap();
        assert_eq!(key.to_string(), "global/jwt_signing_key");

        let key = provider
            .env_var_to_key("SIDESEAT_SECRET_ORG_ACME_API_KEY")
            .unwrap();
        assert_eq!(key.to_string(), "org/acme/api_key");

        assert!(provider.env_var_to_key("UNRELATED_VAR").is_none());
        assert!(
            provider
                .env_var_to_key("SIDESEAT_SECRET_UNKNOWN_FOO")
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_get_reads_env_var() {
        let provider = EnvProvider::new("TEST_SECRET_".to_string());
        let key = SecretKey::global("test_key");
        let env_var = provider.key_to_env_var(&key);

        // SAFETY: test runs single-threaded; no other thread reads this var
        unsafe { std::env::set_var(&env_var, "secret_value") };
        let result = provider.get(&key).await.unwrap();
        assert_eq!(result.unwrap().value, "secret_value");
        unsafe { std::env::remove_var(&env_var) };
    }

    #[tokio::test]
    async fn test_get_missing_returns_none() {
        let provider = EnvProvider::new("TEST_MISSING_SECRET_".to_string());
        let key = SecretKey::global("nonexistent");
        let result = provider.get(&key).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_set_returns_read_only() {
        let provider = EnvProvider::new("TEST_SECRET_".to_string());
        let key = SecretKey::global("test");
        let result = provider.set(&key, &Secret::new("val")).await;
        assert!(matches!(result, Err(SecretError::ReadOnly { .. })));
    }

    #[tokio::test]
    async fn test_delete_returns_read_only() {
        let provider = EnvProvider::new("TEST_SECRET_".to_string());
        let key = SecretKey::global("test");
        let result = provider.delete(&key).await;
        assert!(matches!(result, Err(SecretError::ReadOnly { .. })));
    }

    #[test]
    fn test_properties() {
        let provider = EnvProvider::new("TEST_".to_string());
        assert_eq!(provider.name(), "Environment Variables");
        assert!(!provider.is_persistent());
        assert!(provider.is_read_only());
    }
}
