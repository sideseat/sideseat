use async_trait::async_trait;
use aws_sdk_secretsmanager::Client;

use super::error::SecretError;
use super::provider::SecretProvider;
use super::types::{Secret, SecretKey, SecretScope};

#[derive(Debug)]
pub struct AwsProvider {
    client: Client,
    prefix: String,
    recovery_window_days: Option<u32>,
}

impl AwsProvider {
    pub async fn new(
        region: Option<String>,
        prefix: String,
        recovery_window_days: Option<u32>,
    ) -> Result<Self, SecretError> {
        let mut config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
        if let Some(region) = region {
            config_loader =
                config_loader.region(aws_sdk_secretsmanager::config::Region::new(region));
        }
        let config = config_loader.load().await;
        let client = Client::new(&config);

        tracing::debug!(prefix = %prefix, ?recovery_window_days, "AWS Secrets Manager provider initialized");
        Ok(Self {
            client,
            prefix,
            recovery_window_days,
        })
    }

    fn secret_name(&self, key: &SecretKey) -> String {
        format!("{}/{}", self.prefix, key)
    }

    fn parse_secret_value(json: &str) -> Result<Secret, SecretError> {
        serde_json::from_str(json).map_err(|e| SecretError::Serialization(e.to_string()))
    }

    async fn try_restore(&self, name: &str) -> bool {
        match self.client.restore_secret().secret_id(name).send().await {
            Ok(_) => {
                tracing::debug!(secret = name, "Restored secret pending deletion");
                true
            }
            Err(e) => {
                tracing::debug!(secret = name, error = %e, "Restore attempt failed (secret may not be pending deletion)");
                false
            }
        }
    }
}

#[async_trait]
impl SecretProvider for AwsProvider {
    async fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        let name = self.secret_name(key);
        match self.client.get_secret_value().secret_id(&name).send().await {
            Ok(output) => {
                let value = output
                    .secret_string()
                    .ok_or_else(|| SecretError::backend("aws", "secret has no string value"))?;
                Ok(Some(Self::parse_secret_value(value)?))
            }
            Err(sdk_err) => {
                let is_absent = sdk_err.as_service_error().is_some_and(|e| {
                    // ResourceNotFound: secret doesn't exist
                    // InvalidRequest: secret pending deletion (inaccessible after soft-delete)
                    e.is_resource_not_found_exception() || e.is_invalid_request_exception()
                });
                if is_absent {
                    return Ok(None);
                }
                Err(SecretError::backend("aws", sdk_err.to_string()))
            }
        }
    }

    async fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretError> {
        let name = self.secret_name(key);
        let json =
            serde_json::to_string(secret).map_err(|e| SecretError::Serialization(e.to_string()))?;

        // Try update first (PutSecretValue), fall back to create
        match self
            .client
            .put_secret_value()
            .secret_id(&name)
            .secret_string(&json)
            .send()
            .await
        {
            Ok(_) => return Ok(()),
            Err(sdk_err) => {
                if !sdk_err
                    .as_service_error()
                    .is_some_and(|e| e.is_resource_not_found_exception())
                {
                    // Not "not found" — could be pending deletion or other error.
                    // Try restore in case it's pending deletion, then retry put.
                    if self.try_restore(&name).await
                        && self
                            .client
                            .put_secret_value()
                            .secret_id(&name)
                            .secret_string(&json)
                            .send()
                            .await
                            .is_ok()
                    {
                        return Ok(());
                    }
                    return Err(SecretError::backend("aws", sdk_err.to_string()));
                }
            }
        }

        // Secret doesn't exist — create
        self.client
            .create_secret()
            .name(&name)
            .secret_string(&json)
            .send()
            .await
            .map_err(|e| SecretError::backend("aws", e.to_string()))?;
        Ok(())
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        let name = self.secret_name(key);
        let mut req = self.client.delete_secret().secret_id(&name);
        if let Some(days) = self.recovery_window_days {
            req = req.recovery_window_in_days(days as i64);
        }
        match req.send().await {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                if sdk_err
                    .as_service_error()
                    .is_some_and(|e| e.is_resource_not_found_exception())
                {
                    Err(SecretError::NotFound(name))
                } else {
                    Err(SecretError::backend("aws", sdk_err.to_string()))
                }
            }
        }
    }

    async fn list(&self, scope: &SecretScope) -> Result<Vec<SecretKey>, SecretError> {
        let prefix = match &scope.id {
            None => format!("{}/{}/", self.prefix, scope.kind),
            Some(id) => format!("{}/{}/{}/", self.prefix, scope.kind, id),
        };
        let mut keys = Vec::new();
        let mut next_token: Option<String> = None;
        let strip_prefix = format!("{}/", self.prefix);

        loop {
            let mut req = self.client.list_secrets().filters(
                aws_sdk_secretsmanager::types::Filter::builder()
                    .key(aws_sdk_secretsmanager::types::FilterNameStringType::Name)
                    .values(&prefix)
                    .build(),
            );
            if let Some(token) = next_token {
                req = req.next_token(token);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| SecretError::backend("aws", e.to_string()))?;

            for secret in resp.secret_list() {
                if let Some(name) = secret.name()
                    && let Some(key_str) = name.strip_prefix(&strip_prefix)
                    && let Ok(key) = key_str.parse()
                {
                    keys.push(key);
                }
            }

            next_token = resp.next_token().map(|s| s.to_string());
            if next_token.is_none() {
                break;
            }
        }
        Ok(keys)
    }

    fn name(&self) -> &'static str {
        "AWS Secrets Manager"
    }
    fn is_persistent(&self) -> bool {
        true
    }

    async fn health_check(&self) -> Result<(), SecretError> {
        self.client
            .list_secrets()
            .max_results(1)
            .send()
            .await
            .map_err(|e| SecretError::backend("aws", e.to_string()))?;
        Ok(())
    }
}
