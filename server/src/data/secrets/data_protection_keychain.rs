//! macOS Data Protection Keychain provider
//!
//! Hardware-backed, prompt-free secret storage on macOS 10.15+.
//! Requires the binary to be signed with a Developer ID certificate
//! and the keychain-access-groups entitlement.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use core_foundation::data::CFData;
use security_framework::base::Error as SfError;
use security_framework::item::{
    ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, ItemUpdateOptions, ItemUpdateValue,
    Limit, Location, SearchResult, update_item,
};
use tokio::sync::RwLock;

use super::error::SecretError;
use super::provider::SecretProvider;
use super::types::{Secret, SecretKey, SecretScope, SecretVault};

/// Distinct service name from KeyringProvider's "sideseat" to avoid
/// cross-keychain collisions when ItemSearchOptions queries all keychains.
const SERVICE_NAME: &str = "com.sideseat.vault";
const VAULT_ACCOUNT: &str = "vault";

// macOS Security framework error codes
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;

#[derive(Debug)]
pub struct DataProtectionKeychainProvider {
    vault: Arc<RwLock<SecretVault>>,
    save_mutex: Arc<tokio::sync::Mutex<()>>,
}

/// Probe whether the Data Protection Keychain is accessible by attempting
/// a write. Search alone is unreliable because ItemSearchOptions queries ALL
/// keychains — it could return "not found" from the login keychain even when
/// Data Protection is inaccessible, giving a false positive.
fn probe_access() -> Result<(), SfError> {
    let data = CFData::from_buffer(b"probe");
    let value = ItemAddValue::Data {
        class: ItemClass::generic_password(),
        data,
    };
    let mut opts = ItemAddOptions::new(value);
    opts.set_service(SERVICE_NAME)
        .set_account_name("__probe__")
        .set_location(Location::DataProtectionKeychain);

    let cleanup = || {
        let mut search = ItemSearchOptions::new();
        search
            .class(ItemClass::generic_password())
            .service(SERVICE_NAME)
            .account("__probe__");
        let _ = search.delete();
    };

    match opts.add() {
        Ok(()) => {
            cleanup();
            Ok(())
        }
        Err(e) if e.code() == ERR_SEC_DUPLICATE_ITEM => {
            cleanup();
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn load_vault_blocking() -> Result<SecretVault, SecretError> {
    let mut search = ItemSearchOptions::new();
    search
        .class(ItemClass::generic_password())
        .service(SERVICE_NAME)
        .account(VAULT_ACCOUNT)
        .load_data(true)
        .limit(Limit::Max(1));

    match search.search() {
        Ok(results) => match results.into_iter().next() {
            Some(SearchResult::Data(bytes)) => {
                match serde_json::from_slice::<SecretVault>(&bytes) {
                    Ok(vault) => Ok(vault),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "Corrupted vault in Data Protection Keychain, starting fresh"
                        );
                        Ok(SecretVault::default())
                    }
                }
            }
            Some(_) => {
                tracing::warn!("Unexpected search result type from Data Protection Keychain");
                Ok(SecretVault::default())
            }
            None => Ok(SecretVault::default()),
        },
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(SecretVault::default()),
        Err(e) => Err(SecretError::backend(
            "Data Protection Keychain",
            format!("load failed ({}): {}", e.code(), e),
        )),
    }
}

/// Save vault using update-first-then-add pattern.
/// Avoids the race window of delete-then-add (where another process could
/// insert between our delete and add, causing errSecDuplicateItem).
fn save_vault_blocking(json_bytes: &[u8]) -> Result<(), SecretError> {
    let mut search = ItemSearchOptions::new();
    search
        .class(ItemClass::generic_password())
        .service(SERVICE_NAME)
        .account(VAULT_ACCOUNT);

    let mut update_opts = ItemUpdateOptions::new();
    update_opts.set_value(ItemUpdateValue::Data(CFData::from_buffer(json_bytes)));

    match update_item(&search, &update_opts) {
        Ok(()) => return Ok(()),
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => {
            // Item doesn't exist yet, fall through to add
        }
        Err(e) => {
            return Err(SecretError::backend(
                "Data Protection Keychain",
                format!("update failed ({}): {}", e.code(), e),
            ));
        }
    }

    let data = CFData::from_buffer(json_bytes);
    let value = ItemAddValue::Data {
        class: ItemClass::generic_password(),
        data,
    };
    let mut add_opts = ItemAddOptions::new(value);
    add_opts
        .set_service(SERVICE_NAME)
        .set_account_name(VAULT_ACCOUNT)
        .set_label("SideSeat Secret Vault")
        .set_location(Location::DataProtectionKeychain);

    match add_opts.add() {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERR_SEC_DUPLICATE_ITEM => {
            // Race: another process added between our failed update and add.
            // Retry update — this time the item exists.
            let mut retry_update = ItemUpdateOptions::new();
            retry_update.set_value(ItemUpdateValue::Data(CFData::from_buffer(json_bytes)));
            update_item(&search, &retry_update).map_err(|e| {
                SecretError::backend(
                    "Data Protection Keychain",
                    format!("retry update failed ({}): {}", e.code(), e),
                )
            })
        }
        Err(e) => Err(SecretError::backend(
            "Data Protection Keychain",
            format!("add failed ({}): {}", e.code(), e),
        )),
    }
}

impl DataProtectionKeychainProvider {
    pub async fn init() -> Result<Self> {
        tokio::task::spawn_blocking(probe_access)
            .await
            .map_err(|e| anyhow::anyhow!("keychain probe task failed: {}", e))?
            .map_err(|e| {
                anyhow::anyhow!(
                    "Data Protection Keychain not accessible ({}): {}. \
                     Binary must be signed with Developer ID and keychain-access-groups entitlement.",
                    e.code(),
                    e
                )
            })?;

        let mut vault = tokio::task::spawn_blocking(load_vault_blocking)
            .await
            .map_err(|e| anyhow::anyhow!("keychain load task failed: {}", e))??;

        if vault.migrate() {
            tracing::info!("Migrated vault from v1 to v2 in Data Protection Keychain");
            let json = serde_json::to_string(&vault)?;
            let json_bytes = json.into_bytes();
            tokio::task::spawn_blocking(move || save_vault_blocking(&json_bytes))
                .await
                .map_err(|e| anyhow::anyhow!("keychain save task failed: {}", e))??;
        }

        tracing::debug!("Data Protection Keychain provider initialized");
        Ok(Self {
            vault: Arc::new(RwLock::new(vault)),
            save_mutex: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    async fn save(&self) -> Result<(), SecretError> {
        let _guard = self.save_mutex.lock().await;
        let json_bytes = {
            let vault = self.vault.read().await;
            serde_json::to_string(&*vault)
                .map_err(|e| SecretError::Serialization(e.to_string()))?
                .into_bytes()
        };
        tokio::task::spawn_blocking(move || save_vault_blocking(&json_bytes))
            .await
            .map_err(|e| {
                SecretError::backend("Data Protection Keychain", format!("task failed: {}", e))
            })?
    }
}

#[async_trait]
impl SecretProvider for DataProtectionKeychainProvider {
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
        "macOS Data Protection Keychain"
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires Developer ID signed binary"]
    async fn test_roundtrip() {
        let provider = DataProtectionKeychainProvider::init().await.unwrap();
        let key = SecretKey::global("dp_test_key");
        let secret = Secret::new("dp_test_value");

        provider.set(&key, &secret).await.unwrap();
        let got = provider.get(&key).await.unwrap().unwrap();
        assert_eq!(got.value, "dp_test_value");

        provider.delete(&key).await.unwrap();
        assert!(provider.get(&key).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore = "requires Developer ID signed binary"]
    async fn test_probe_access() {
        let result = tokio::task::spawn_blocking(probe_access).await.unwrap();
        assert!(result.is_ok(), "probe_access failed: {:?}", result.err());
    }
}
