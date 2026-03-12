use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::ProviderError;
use crate::provider::{ChatProvider, Provider, ProviderStream};
use crate::types::{Message, ModelInfo, ProviderConfig, Response, TokenCount};

/// A registry of named providers that routes requests by model prefix.
///
/// Model identifiers must use the format `"prefix:model"`. The registry splits
/// on the first `:` to locate the provider, then calls it with the suffix as
/// the model name.
///
/// # Example
///
/// ```no_run
/// use sideseat::{registry::ProviderRegistry, ProviderConfig, ChatProvider};
///
/// let mut reg = ProviderRegistry::new();
/// // reg.register("openai", OpenAIChatProvider::from_env().unwrap());
/// // let config = ProviderConfig::new("openai:gpt-4o");
/// // let response = reg.complete(messages, config).await?;
/// ```
pub struct ProviderRegistry {
    providers: HashMap<String, Box<dyn ChatProvider + Send + Sync>>,
    default_models: HashMap<String, String>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            default_models: HashMap::new(),
        }
    }

    /// Register a provider under the given prefix.
    pub fn register(
        &mut self,
        prefix: impl Into<String>,
        provider: impl ChatProvider + 'static,
    ) -> &mut Self {
        self.providers.insert(prefix.into(), Box::new(provider));
        self
    }

    /// Register a provider with a default model.
    ///
    /// When the registry is called with just the prefix (no `:model` suffix), the
    /// `default_model` string is forwarded to the provider as the model name.
    pub fn register_with_default(
        &mut self,
        prefix: impl Into<String>,
        default_model: impl Into<String>,
        provider: impl ChatProvider + 'static,
    ) -> &mut Self {
        let prefix = prefix.into();
        self.default_models
            .insert(prefix.clone(), default_model.into());
        self.providers.insert(prefix, Box::new(provider));
        self
    }

    /// List all registered prefixes.
    pub fn prefixes(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }

    /// Returns true if `model_id` (in `"prefix:model"` format) is handled by this registry.
    pub fn has_model(&self, model_id: &str) -> bool {
        self.resolve_prefix(model_id).is_some()
    }

    fn resolve_prefix<'a>(&'a self, model_id: &str) -> Option<(&'a dyn ChatProvider, String)> {
        if let Some(colon) = model_id.find(':') {
            let prefix = &model_id[..colon];
            let model = model_id[colon + 1..].to_string();
            if let Some(p) = self.providers.get(prefix) {
                return Some((p.as_ref(), model));
            }
        }
        // Fallback: model_id itself is a registered prefix (e.g. "openai" with no colon).
        // Use the registered default model when available; otherwise send empty string.
        if let Some(p) = self.providers.get(model_id) {
            let model = self
                .default_models
                .get(model_id)
                .cloned()
                .unwrap_or_default();
            return Some((p.as_ref(), model));
        }
        None
    }

    fn resolve<'a>(
        &'a self,
        model_id: &str,
    ) -> Result<(&'a dyn ChatProvider, String), ProviderError> {
        self.resolve_prefix(model_id)
            .ok_or_else(|| ProviderError::ModelNotFound {
                model: model_id.to_string(),
            })
    }
}

#[async_trait]
impl Provider for ProviderRegistry {
    fn provider_name(&self) -> &'static str {
        "registry"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut all = Vec::new();
        for p in self.providers.values() {
            if let Ok(models) = p.list_models().await {
                all.extend(models);
            }
        }
        Ok(all)
    }
}

#[async_trait]
impl ChatProvider for ProviderRegistry {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let (provider, model) = match self.resolve(&config.model) {
            Ok(r) => r,
            Err(e) => return Box::pin(futures::stream::once(async move { Err(e) })),
        };
        let config = ProviderConfig { model, ..config };
        provider.stream(messages, config)
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        let (provider, model) = self.resolve(&config.model)?;
        let config = ProviderConfig { model, ..config };
        provider.complete(messages, config).await
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        let (provider, model) = self.resolve(&config.model)?;
        let config = ProviderConfig { model, ..config };
        provider.count_tokens(messages, config).await
    }
}
