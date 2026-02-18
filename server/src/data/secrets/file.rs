use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::RwLock;

use super::error::SecretError;
use super::provider::SecretProvider;
use super::types::{Secret, SecretKey, SecretScope, SecretVault};

const FILE_SECRETS_FILENAME: &str = "secrets.json";

#[derive(Debug)]
pub struct FileProvider {
    path: PathBuf,
    vault: Arc<RwLock<SecretVault>>,
    save_mutex: Arc<tokio::sync::Mutex<()>>,
}

impl FileProvider {
    pub async fn init(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(FILE_SECRETS_FILENAME);
        let mut vault = Self::load(&path).await?;

        if vault.migrate() {
            tracing::info!("Migrated secret vault from v1 to v2 (added global/ prefix)");
            let json = serde_json::to_string_pretty(&vault)
                .context("Failed to serialize vault after migration")?;
            Self::atomic_write(&path, &json)
                .await
                .context("Failed to save migrated vault")?;
        }

        Ok(Self {
            path,
            vault: Arc::new(RwLock::new(vault)),
            save_mutex: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    async fn load(path: &Path) -> Result<SecretVault> {
        match tokio::fs::read_to_string(path).await {
            Ok(json) => match serde_json::from_str::<SecretVault>(&json) {
                Ok(vault) => {
                    tracing::debug!(count = vault.secrets.len(), "Loaded secrets from file");
                    Ok(vault)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Corrupted secrets file, creating backup and starting fresh"
                    );
                    let backup = format!(
                        "{}.corrupt.{}",
                        path.display(),
                        chrono::Utc::now().timestamp()
                    );
                    if let Err(rename_err) = tokio::fs::rename(path, &backup).await {
                        tracing::warn!(error = %rename_err, "Failed to backup corrupted secrets file");
                    } else {
                        tracing::info!(backup = %backup, "Backed up corrupted secrets file");
                    }
                    Ok(SecretVault::default())
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("No existing secrets file, creating new vault");
                Ok(SecretVault::default())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to load secrets file: {}", e)),
        }
    }

    async fn atomic_write(path: &Path, json: &str) -> Result<(), SecretError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp_path = path.with_extension("json.tmp");
        tokio::fs::write(&tmp_path, json).await?;
        tokio::fs::rename(&tmp_path, path).await?;
        Ok(())
    }

    async fn save(&self) -> Result<(), SecretError> {
        let _guard = self.save_mutex.lock().await;
        let json = {
            let vault = self.vault.read().await;
            serde_json::to_string_pretty(&*vault)
                .map_err(|e| SecretError::Serialization(e.to_string()))?
        };
        Self::atomic_write(&self.path, &json).await
    }
}

#[async_trait]
impl SecretProvider for FileProvider {
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
        "File (INSECURE - dev only)"
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_file_provider_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileProvider::init(dir.path()).await.unwrap();

        let key = SecretKey::global("test_key");
        let secret = Secret::new("test_value");

        provider.set(&key, &secret).await.unwrap();

        let got = provider.get(&key).await.unwrap().unwrap();
        assert_eq!(got.value, "test_value");

        assert!(provider.exists(&key).await.unwrap());

        let keys = provider.list(&SecretScope::global()).await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].to_string(), "global/test_key");

        provider.delete(&key).await.unwrap();
        assert!(provider.get(&key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_created_at_preserved_on_update() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileProvider::init(dir.path()).await.unwrap();

        let key = SecretKey::global("test_key");
        let secret = Secret::new("v1");
        provider.set(&key, &secret).await.unwrap();
        let original_created = provider
            .get(&key)
            .await
            .unwrap()
            .unwrap()
            .metadata
            .created_at;

        let secret2 = Secret::new("v2");
        provider.set(&key, &secret2).await.unwrap();
        let updated = provider.get(&key).await.unwrap().unwrap();
        assert_eq!(updated.value, "v2");
        assert_eq!(updated.metadata.created_at, original_created);
    }

    #[tokio::test]
    async fn test_vault_migration_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_SECRETS_FILENAME);

        let v1_json = r#"{"secrets":{"jwt_signing_key":{"value":"abc","metadata":{"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}}}}"#;
        tokio::fs::create_dir_all(dir.path()).await.unwrap();
        tokio::fs::write(&path, v1_json).await.unwrap();

        let provider = FileProvider::init(dir.path()).await.unwrap();

        let key = SecretKey::global("jwt_signing_key");
        let got = provider.get(&key).await.unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().value, "abc");
    }

    #[tokio::test]
    async fn test_corruption_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_SECRETS_FILENAME);

        tokio::fs::create_dir_all(dir.path()).await.unwrap();
        tokio::fs::write(&path, "not-json{{{").await.unwrap();

        let provider = FileProvider::init(dir.path()).await.unwrap();

        let keys = provider.list(&SecretScope::global()).await.unwrap();
        assert!(keys.is_empty());

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("secrets.json.corrupt.")
            })
            .collect();
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn test_scoped_keys() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FileProvider::init(dir.path()).await.unwrap();

        let global = SecretKey::global("key1");
        let org = SecretKey::new("key2", SecretScope::org("acme"));
        let proj = SecretKey::new("key3", SecretScope::project("p1"));

        provider.set(&global, &Secret::new("g")).await.unwrap();
        provider.set(&org, &Secret::new("o")).await.unwrap();
        provider.set(&proj, &Secret::new("p")).await.unwrap();

        let global_keys = provider.list(&SecretScope::global()).await.unwrap();
        assert_eq!(global_keys.len(), 1);

        let org_keys = provider.list(&SecretScope::org("acme")).await.unwrap();
        assert_eq!(org_keys.len(), 1);

        let proj_keys = provider.list(&SecretScope::project("p1")).await.unwrap();
        assert_eq!(proj_keys.len(), 1);

        let empty = provider.list(&SecretScope::user("nobody")).await.unwrap();
        assert!(empty.is_empty());
    }
}
