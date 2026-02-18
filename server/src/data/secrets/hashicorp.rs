use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue};

use super::error::SecretError;
use super::provider::SecretProvider;
use super::types::{Secret, SecretKey, SecretScope};

const VAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug)]
pub struct HashiVaultProvider {
    client: reqwest::Client,
    address: String,
    mount: String,
    prefix: String,
}

impl HashiVaultProvider {
    pub fn new(
        address: String,
        token: &str,
        mount: String,
        prefix: String,
    ) -> Result<Self, SecretError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Vault-Token",
            HeaderValue::from_str(token)
                .map_err(|e| SecretError::Config(format!("invalid vault token: {}", e)))?,
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(VAULT_TIMEOUT_SECS))
            .default_headers(headers)
            .build()
            .map_err(|e| SecretError::Config(format!("failed to build HTTP client: {}", e)))?;

        tracing::debug!(
            address = %address,
            mount = %mount,
            prefix = %prefix,
            "HashiCorp Vault provider initialized"
        );
        Ok(Self {
            client,
            address,
            mount,
            prefix,
        })
    }

    /// KV v2 data path: {address}/v1/{mount}/data/{prefix}/{key_path}
    fn data_url(&self, key: &SecretKey) -> String {
        format!(
            "{}/v1/{}/data/{}/{}",
            self.address, self.mount, self.prefix, key
        )
    }

    /// KV v2 metadata path for LIST
    fn metadata_url(&self, scope: &SecretScope) -> String {
        let scope_path = match &scope.id {
            None => format!("{}", scope.kind),
            Some(id) => format!("{}/{}", scope.kind, id),
        };
        format!(
            "{}/v1/{}/metadata/{}/{}/",
            self.address, self.mount, self.prefix, scope_path
        )
    }
}

#[async_trait]
impl SecretProvider for HashiVaultProvider {
    async fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        let url = self.data_url(key);
        let resp = self.client.get(&url).send().await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(SecretError::backend(
                "vault",
                format!("GET {} returned {}", url, resp.status()),
            ));
        }

        // KV v2 response: { "data": { "data": { "value": "...", "metadata": {...} } } }
        let body: serde_json::Value = resp.json().await?;
        let data = &body["data"]["data"];
        let secret: Secret = serde_json::from_value(data.clone())
            .map_err(|e| SecretError::Serialization(e.to_string()))?;
        Ok(Some(secret))
    }

    async fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretError> {
        let url = self.data_url(key);
        let payload = serde_json::json!({
            "data": secret,
        });
        let resp = self.client.post(&url).json(&payload).send().await?;

        if !resp.status().is_success() {
            return Err(SecretError::backend(
                "vault",
                format!("POST {} returned {}", url, resp.status()),
            ));
        }
        Ok(())
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        // Delete metadata (permanent delete in KV v2)
        let url = format!(
            "{}/v1/{}/metadata/{}/{}",
            self.address, self.mount, self.prefix, key
        );
        let resp = self.client.delete(&url).send().await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SecretError::NotFound(key.to_string()));
        }
        if !resp.status().is_success() {
            return Err(SecretError::backend(
                "vault",
                format!("DELETE {} returned {}", url, resp.status()),
            ));
        }
        Ok(())
    }

    async fn list(&self, scope: &SecretScope) -> Result<Vec<SecretKey>, SecretError> {
        // Use GET ?list=true (not custom LIST method) to avoid proxy/WAF issues
        let url = format!("{}?list=true", self.metadata_url(scope));
        let resp = self.client.get(&url).send().await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !resp.status().is_success() {
            return Err(SecretError::backend(
                "vault",
                format!("LIST {} returned {}", url, resp.status()),
            ));
        }

        let body: serde_json::Value = resp.json().await?;
        let empty = Vec::new();
        let entries = body["data"]["keys"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|s| !s.ends_with('/'))
            .filter_map(|name| {
                let full = match &scope.id {
                    None => format!("{}/{}", scope.kind, name),
                    Some(id) => format!("{}/{}/{}", scope.kind, id, name),
                };
                full.parse().ok()
            })
            .collect();
        Ok(entries)
    }

    fn name(&self) -> &'static str {
        "HashiCorp Vault"
    }
    fn is_persistent(&self) -> bool {
        true
    }

    async fn health_check(&self) -> Result<(), SecretError> {
        let url = format!("{}/v1/auth/token/lookup-self", self.address);
        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Err(SecretError::backend(
                "vault",
                format!("token lookup-self returned {}", resp.status()),
            ));
        }

        let body: serde_json::Value = resp.json().await?;
        let data = &body["data"];

        let renewable = data["renewable"].as_bool().unwrap_or(false);
        let ttl = data["ttl"].as_u64().unwrap_or(0);
        let creation_ttl = data["creation_ttl"].as_u64().unwrap_or(0);

        if renewable && creation_ttl > 0 && ttl < creation_ttl / 2 {
            let renew_url = format!("{}/v1/auth/token/renew-self", self.address);
            match self
                .client
                .post(&renew_url)
                .json(&serde_json::json!({}))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    tracing::debug!(ttl, creation_ttl, "Vault token renewed");
                }
                Ok(r) => {
                    tracing::warn!(
                        status = %r.status(),
                        "Vault token renewal failed"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Vault token renewal request failed");
                }
            }
        }

        Ok(())
    }
}
