use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::ProviderError;
use crate::provider::{Provider, ProviderStream};
use crate::types::{
    EmbeddingRequest, EmbeddingResponse, ImageGenerationRequest, ImageGenerationResponse, Message,
    ModelInfo, ProviderConfig, Response, SpeechRequest, SpeechResponse, TokenCount,
    TranscriptionRequest, TranscriptionResponse, VideoGenerationRequest, VideoGenerationResponse,
};

/// A registry of named providers that routes requests by model prefix.
///
/// Model identifiers must use the format `"prefix:model"`. The registry splits
/// on the first `:` to locate the provider, then calls it with the suffix as
/// the model name.
///
/// # Example
///
/// ```no_run
/// use sideseat::{registry::ProviderRegistry, ProviderConfig, Provider};
///
/// let mut reg = ProviderRegistry::new();
/// // reg.register("openai", OpenAIChatProvider::from_env().unwrap());
/// // let config = ProviderConfig::new("openai:gpt-4o");
/// // let response = reg.complete(messages, config).await?;
/// ```
pub struct ProviderRegistry {
    providers: HashMap<String, Box<dyn Provider + Send + Sync>>,
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
        }
    }

    /// Register a provider under the given prefix.
    pub fn register(
        &mut self,
        prefix: impl Into<String>,
        provider: impl Provider + 'static,
    ) -> &mut Self {
        self.providers.insert(prefix.into(), Box::new(provider));
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

    fn resolve_prefix<'a>(&'a self, model_id: &str) -> Option<(&'a dyn Provider, String)> {
        if let Some(colon) = model_id.find(':') {
            let prefix = &model_id[..colon];
            let model = model_id[colon + 1..].to_string();
            if let Some(p) = self.providers.get(prefix) {
                return Some((p.as_ref(), model));
            }
        }
        // Fallback: try the whole string as prefix with empty model
        if let Some(p) = self.providers.get(model_id) {
            return Some((p.as_ref(), String::new()));
        }
        None
    }

    fn resolve<'a>(&'a self, model_id: &str) -> Result<(&'a dyn Provider, String), ProviderError> {
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

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut all = Vec::new();
        for p in self.providers.values() {
            if let Ok(models) = p.list_models().await {
                all.extend(models);
            }
        }
        Ok(all)
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

    async fn embed(
        &self,
        request: EmbeddingRequest,
        model: &str,
    ) -> Result<EmbeddingResponse, ProviderError> {
        let (provider, resolved_model) = self.resolve(model)?;
        provider.embed(request, &resolved_model).await
    }

    async fn generate_image(
        &self,
        mut request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        let (provider, model) = self.resolve(&request.model)?;
        request.model = model;
        provider.generate_image(request).await
    }

    async fn generate_video(
        &self,
        mut request: VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        let (provider, model) = self.resolve(&request.model)?;
        request.model = model;
        provider.generate_video(request).await
    }

    async fn generate_speech(
        &self,
        mut request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        let (provider, model) = self.resolve(&request.model)?;
        request.model = model;
        provider.generate_speech(request).await
    }

    async fn transcribe(
        &self,
        mut request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        let (provider, model) = self.resolve(&request.model)?;
        request.model = model;
        provider.transcribe(request).await
    }
}
