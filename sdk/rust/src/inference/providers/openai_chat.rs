use std::collections::HashMap;
use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{
        AudioProvider, ChatProvider, EmbeddingProvider, ImageProvider, ModerationProvider,
        Provider, ProviderStream,
    },
    providers::{
        openai_common::{OpenAIInnerClient, parse_finish_reason, parse_usage},
        sse::{check_response, sse_data_stream},
    },
    types::{
        AudioContent, AudioFormat, ContentBlock, ContentBlockStart, ContentDelta, EmbeddingRequest,
        EmbeddingResponse, ImageContent, ImageEditRequest, ImageGenerationRequest,
        ImageGenerationResponse, MediaSource, Message, ModelInfo, ModerationRequest,
        ModerationResponse, ProviderConfig, ResponseFormat, Role, SpeechRequest, SpeechResponse,
        StreamEvent, TokenCount, Tool, ToolChoice, ToolUseBlock, TranscriptionRequest,
        TranscriptionResponse, WebSearchConfig,
    },
};

const OPENAI_CHAT_URL: &str = "https://api.openai.com/v1/chat/completions";
const OPENAI_API_BASE: &str = "https://api.openai.com/v1";

/// OpenAI Chat Completions API provider.
///
/// Supports all GPT and o-series models with streaming, tool calling,
/// multi-modal inputs (text, images, audio), structured output, and reasoning effort.
///
/// Also serves as the base for OpenAI-compatible providers:
/// use `for_groq()`, `for_deepseek()`, `for_xai()`, `for_together()`,
/// `for_fireworks()`, `for_mistral()`, `for_ollama()`, or `for_bedrock_openai()`.
pub struct OpenAIChatProvider {
    shared: OpenAIInnerClient,
    base_url: String,
    /// Optional prefix prepended to model names (e.g. `accounts/fireworks/models/` for Fireworks).
    model_prefix: Option<String>,
}

impl OpenAIChatProvider {
    /// Create a provider from the `OPENAI_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ProviderError> {
        Ok(Self::new(crate::env::require(
            crate::env::keys::OPENAI_API_KEY,
        )?))
    }

    /// Create a Groq provider from the `GROQ_API_KEY` environment variable.
    pub fn for_groq_from_env() -> Result<Self, ProviderError> {
        Ok(Self::for_groq(crate::env::require(
            crate::env::keys::GROQ_API_KEY,
        )?))
    }

    /// Create a DeepSeek provider from the `DEEPSEEK_API_KEY` environment variable.
    pub fn for_deepseek_from_env() -> Result<Self, ProviderError> {
        Ok(Self::for_deepseek(crate::env::require(
            crate::env::keys::DEEPSEEK_API_KEY,
        )?))
    }

    /// Create an xAI provider from the `XAI_API_KEY` environment variable.
    pub fn for_xai_from_env() -> Result<Self, ProviderError> {
        Ok(Self::for_xai(crate::env::require(
            crate::env::keys::XAI_API_KEY,
        )?))
    }

    /// Create a Mistral provider from the `MISTRAL_API_KEY` environment variable.
    pub fn for_mistral_from_env() -> Result<Self, ProviderError> {
        Ok(Self::for_mistral(crate::env::require(
            crate::env::keys::MISTRAL_API_KEY,
        )?))
    }

    /// Create a Together AI provider from the `TOGETHER_API_KEY` environment variable.
    pub fn for_together_from_env() -> Result<Self, ProviderError> {
        Ok(Self::for_together(crate::env::require(
            crate::env::keys::TOGETHER_API_KEY,
        )?))
    }

    pub fn new(api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        let client = Arc::new(reqwest::Client::new());
        Self {
            shared: OpenAIInnerClient::new(api_key, client, OPENAI_API_BASE),
            base_url: OPENAI_CHAT_URL.to_string(),
            model_prefix: None,
        }
    }

    /// Replace the HTTP client. Useful for custom TLS, proxies, or testing.
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.shared.client = Arc::new(client);
        self
    }

    /// Set the full chat completions endpoint URL.
    /// The API base for models/embeddings is derived by stripping `/chat/completions`.
    /// For Ollama or other OpenAI-compatible proxies prefer `with_api_base()`.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let url = base_url.into();
        if let Some(pos) = url.find("/chat/completions") {
            self.shared.api_base = url[..pos].to_string();
        } else {
            self.shared.api_base = url.clone();
        }
        self.base_url = url;
        self
    }

    /// Set the API base URL for use with Ollama, LiteLLM, OpenRouter, or any
    /// OpenAI-compatible proxy.  All endpoints are derived from this base:
    /// - Chat:       `{base}/chat/completions`
    /// - Models:     `{base}/models`
    /// - Embeddings: `{base}/embeddings`
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        let base = api_base.into();
        self.base_url = format!("{}/chat/completions", base);
        self.shared.api_base = base;
        self
    }

    // -----------------------------------------------------------------------
    // OpenAI-compatible provider factories
    // -----------------------------------------------------------------------

    /// [Groq](https://console.groq.com) — ultra-fast inference.
    /// API key from `GROQ_API_KEY` env var or pass explicitly.
    pub fn for_groq(api_key: impl Into<String>) -> Self {
        Self::new(api_key).with_api_base("https://api.groq.com/openai/v1")
    }

    /// [DeepSeek](https://platform.deepseek.com) — reasoning and coding models.
    /// API key from `DEEPSEEK_API_KEY` env var or pass explicitly.
    pub fn for_deepseek(api_key: impl Into<String>) -> Self {
        Self::new(api_key).with_api_base("https://api.deepseek.com/v1")
    }

    /// [xAI Grok](https://x.ai) — Grok models.
    /// API key from `XAI_API_KEY` env var or pass explicitly.
    pub fn for_xai(api_key: impl Into<String>) -> Self {
        Self::new(api_key).with_api_base("https://api.x.ai/v1")
    }

    /// [Together AI](https://www.together.ai) — open-source model hosting.
    /// API key from `TOGETHER_API_KEY` env var or pass explicitly.
    pub fn for_together(api_key: impl Into<String>) -> Self {
        Self::new(api_key).with_api_base("https://api.together.xyz/v1")
    }

    /// [Fireworks AI](https://fireworks.ai) — fast open model inference.
    /// API key from `FIREWORKS_API_KEY` env var or pass explicitly.
    /// Model names without a `/` are automatically prefixed with `accounts/fireworks/models/`.
    pub fn for_fireworks(api_key: impl Into<String>) -> Self {
        let mut p = Self::new(api_key).with_api_base("https://api.fireworks.ai/inference/v1");
        p.model_prefix = Some("accounts/fireworks/models/".to_string());
        p
    }

    /// [Mistral AI](https://mistral.ai) — Mistral and Mixtral models.
    /// API key from `MISTRAL_API_KEY` env var or pass explicitly.
    pub fn for_mistral(api_key: impl Into<String>) -> Self {
        Self::new(api_key).with_api_base("https://api.mistral.ai/v1")
    }

    /// [Cerebras](https://cerebras.ai) — high-speed wafer-scale inference.
    /// API key from `CEREBRAS_API_KEY` env var or pass explicitly.
    pub fn for_cerebras(api_key: impl Into<String>) -> Self {
        Self::new(api_key).with_api_base("https://api.cerebras.ai/v1")
    }

    /// [Perplexity](https://www.perplexity.ai) — search-grounded models.
    /// API key from `PERPLEXITY_API_KEY` env var or pass explicitly.
    pub fn for_perplexity(api_key: impl Into<String>) -> Self {
        Self::new(api_key).with_api_base("https://api.perplexity.ai")
    }

    /// [Ollama](https://ollama.ai) — local model runner.
    /// Pass a custom endpoint to override the default `http://localhost:11434/v1`.
    pub fn for_ollama(endpoint: Option<&str>) -> Self {
        let base = endpoint.unwrap_or("http://localhost:11434/v1");
        // Ollama does not require auth; pass a placeholder key
        Self::new("ollama").with_api_base(base)
    }

    /// [OpenRouter](https://openrouter.ai) — unified multi-provider gateway.
    /// API key from `OPENROUTER_API_KEY` env var or pass explicitly.
    pub fn for_openrouter(api_key: impl Into<String>) -> Self {
        Self::new(api_key).with_api_base("https://openrouter.ai/api/v1")
    }

    /// [Amazon Bedrock OpenAI-compatible API](https://docs.aws.amazon.com/bedrock/latest/userguide/bedrock-mantle.html) —
    /// OpenAI-compatible endpoints backed by Amazon Bedrock.
    ///
    /// `region`: AWS region, e.g. `"us-east-1"`.
    /// `api_key`: a Bedrock API key (bearer token).
    ///
    /// Use `openai.` prefixed model names, e.g. `"openai.gpt-oss-120b"`.
    pub fn for_bedrock_openai(region: impl Into<String>, api_key: impl Into<String>) -> Self {
        let region = region.into();
        Self::new(api_key).with_api_base(format!("https://bedrock-mantle.{region}.api.aws/v1"))
    }

    /// Create an Amazon Bedrock OpenAI-compatible API provider from environment variables.
    ///
    /// Reads `BEDROCK_API_KEY` (or `AWS_BEARER_TOKEN_BEDROCK`) for the API key
    /// and `BEDROCK_REGION` / `AWS_REGION` / `AWS_DEFAULT_REGION` for the region
    /// (defaulting to `"us-east-1"`).
    pub fn for_bedrock_openai_from_env() -> Result<Self, crate::error::ProviderError> {
        let api_key = crate::env::require(crate::env::keys::BEDROCK_API_KEY)
            .or_else(|_| crate::env::require("AWS_BEARER_TOKEN_BEDROCK"))?;
        let region = crate::env::optional("BEDROCK_REGION")
            .or_else(|| crate::env::optional("AWS_REGION"))
            .or_else(|| crate::env::optional("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|| "us-east-1".to_string());
        Ok(Self::for_bedrock_openai(region, api_key))
    }

    /// Initialize Azure AI Foundry with default credential discovery.
    ///
    /// Tries `AZURE_OPENAI_API_KEY` env var first; falls back to Azure Managed
    /// Identity via the IMDS endpoint (Azure VMs, Container Apps, AKS pod identity).
    pub async fn for_azure_default(
        endpoint: impl Into<String>,
    ) -> Result<Self, crate::error::ProviderError> {
        let ep = endpoint.into();
        if let Ok(key) = std::env::var("AZURE_OPENAI_API_KEY")
            && !key.is_empty()
        {
            return Ok(Self::new(key).with_api_base(ep));
        }
        let token = fetch_azure_imds_token("https://cognitiveservices.azure.com").await?;
        Ok(Self::new(token).with_api_base(ep))
    }
}

async fn fetch_azure_imds_token(resource: &str) -> Result<String, crate::error::ProviderError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| crate::error::ProviderError::Auth(format!("HTTP client error: {e}")))?;
    let resp: serde_json::Value = client
        .get(format!(
            "http://169.254.169.254/metadata/identity/oauth2/token\
             ?api-version=2018-02-01&resource={resource}"
        ))
        .header("Metadata", "true")
        .send()
        .await
        .map_err(|e| {
            crate::error::ProviderError::Auth(format!(
                "Azure IMDS unreachable (not an Azure host?): {e}"
            ))
        })?
        .json()
        .await
        .map_err(|e| {
            crate::error::ProviderError::Auth(format!("Azure IMDS response parse error: {e}"))
        })?;
    resp.get("access_token")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .ok_or_else(|| {
            crate::error::ProviderError::Auth(format!("Azure IMDS missing access_token: {resp}"))
        })
}

#[async_trait]
impl Provider for OpenAIChatProvider {
    fn provider_name(&self) -> &'static str {
        "openai"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, crate::error::ProviderError> {
        self.shared.list_models().await
    }
}

#[async_trait]
impl ChatProvider for OpenAIChatProvider {
    fn stream(&self, messages: Vec<Message>, mut config: ProviderConfig) -> ProviderStream {
        let api_key = self.shared.api_key.clone();
        let client = Arc::clone(&self.shared.client);
        let base_url = self.base_url.clone();
        // Apply model prefix before moving config into the stream
        if let Some(prefix) = &self.model_prefix
            && !config.model.contains('/')
        {
            config.model = format!("{}{}", prefix, config.model);
        }

        Box::pin(stream! {
            let body = match build_request(&messages, &config, true) {
                Ok(b) => b,
                Err(e) => { yield Err(e); return; }
            };

            let mut req_builder = client
                .post(&base_url)
                .bearer_auth(&api_key)
                .json(&body);
            if let Some(ms) = config.timeout_ms {
                req_builder = req_builder.timeout(std::time::Duration::from_millis(ms));
            }
            let resp = match req_builder.send().await {
                Ok(r) => r,
                Err(e) => { yield Err(e.into()); return; }
            };

            let resp = match check_response(resp).await {
                Ok(r) => r,
                Err(e) => { yield Err(e); return; }
            };

            yield Ok(StreamEvent::MessageStart { role: Role::Assistant });

            let text_index: usize = 0;
            let reasoning_index: usize = 1; // thinking block always at index 1 if present
            let audio_index: usize = 2;     // audio output block at index 2 if present
            let mut text_started = false;
            let mut reasoning_started = false;
            let mut audio_started = false;
            // Map from tool call stream index to content block index and arg buffer
            // Tools start at index 3 to leave room for text(0), reasoning(1), audio(2)
            let mut tool_calls: HashMap<usize, (String, String, usize)> = HashMap::new(); // idx -> (id, name, block_idx)
            let mut tool_arg_bufs: HashMap<usize, String> = HashMap::new();

            let mut data_stream = Box::pin(sse_data_stream(resp));
            use futures::StreamExt;

            while let Some(result) = data_stream.next().await {
                let data = match result {
                    Ok(d) => d,
                    Err(e) => { yield Err(e); return; }
                };

                let parsed: Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Usage-only chunk (final chunk with empty choices)
                if let Some(usage_obj) = parsed.get("usage").filter(|u| !u.is_null()) {
                    let usage = parse_usage(usage_obj);
                    let model = parsed["model"].as_str().map(|s| s.to_string());
                    yield Ok(StreamEvent::Metadata { usage, model, id: None });
                    continue;
                }

                let choices = match parsed["choices"].as_array() {
                    Some(c) if !c.is_empty() => c,
                    _ => continue,
                };
                let choice = &choices[0];
                let delta = &choice["delta"];
                let finish_reason = choice["finish_reason"].as_str();

                // Reasoning content delta (DeepSeek, xAI and other providers that use
                // delta.reasoning_content instead of embedding thinking in <think> tags)
                if let Some(thinking) = delta["reasoning_content"].as_str() && !thinking.is_empty() {
                    if !reasoning_started {
                        yield Ok(StreamEvent::ContentBlockStart {
                            index: reasoning_index,
                            block: ContentBlockStart::Thinking,
                        });
                        reasoning_started = true;
                    }
                    yield Ok(StreamEvent::ContentBlockDelta {
                        index: reasoning_index,
                        delta: ContentDelta::Thinking { text: thinking.to_string() },
                    });
                }

                // Text content delta
                if let Some(text) = delta["content"].as_str() {
                    // Close reasoning block before text if needed
                    if reasoning_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: reasoning_index });
                        reasoning_started = false;
                    }
                    if !text_started {
                        yield Ok(StreamEvent::ContentBlockStart {
                            index: text_index,
                            block: ContentBlockStart::Text,
                        });
                        text_started = true;
                    }
                    if !text.is_empty() {
                        yield Ok(StreamEvent::ContentBlockDelta {
                            index: text_index,
                            delta: ContentDelta::Text { text: text.to_string() },
                        });
                    }
                }

                // Audio output delta (gpt-4o-audio-preview and similar)
                if let Some(audio_data) = delta["audio"]["data"].as_str()
                    && !audio_data.is_empty()
                {
                    if !audio_started {
                        yield Ok(StreamEvent::ContentBlockStart {
                            index: audio_index,
                            block: ContentBlockStart::Audio,
                        });
                        audio_started = true;
                    }
                    yield Ok(StreamEvent::ContentBlockDelta {
                        index: audio_index,
                        delta: ContentDelta::AudioData { b64_data: audio_data.to_string() },
                    });
                }

                // Tool call deltas
                if let Some(tc_arr) = delta["tool_calls"].as_array() {
                    // Close open blocks
                    if reasoning_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: reasoning_index });
                        reasoning_started = false;
                    }
                    if text_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                        text_started = false;
                    }
                    // Tool calls start at index 3 (after text=0, reasoning=1, audio=2)
                    let tool_base_index = 3;

                    for tc_delta in tc_arr {
                        let stream_idx = tc_delta["index"].as_u64().unwrap_or(0) as usize;
                        let block_idx = tool_base_index + stream_idx;

                        // First delta for this tool call has the ID and name
                        if let Some(id) = tc_delta["id"].as_str() {
                            let name = tc_delta["function"]["name"]
                                .as_str().unwrap_or("").to_string();
                            tool_calls.insert(stream_idx, (id.to_string(), name.clone(), block_idx));
                            tool_arg_bufs.insert(stream_idx, String::new());
                            yield Ok(StreamEvent::ContentBlockStart {
                                index: block_idx,
                                block: ContentBlockStart::ToolUse {
                                    id: id.to_string(),
                                    name,
                                },
                            });
                        }
                        if let Some(args) = tc_delta["function"]["arguments"].as_str() {
                            if let Some(buf) = tool_arg_bufs.get_mut(&stream_idx) {
                                buf.push_str(args);
                            }
                            if !args.is_empty() {
                                let idx = tool_calls.get(&stream_idx).map(|(_, _, i)| *i)
                                    .unwrap_or(block_idx);
                                yield Ok(StreamEvent::ContentBlockDelta {
                                    index: idx,
                                    delta: ContentDelta::ToolInput {
                                        partial_json: args.to_string(),
                                    },
                                });
                            }
                        }
                    }
                }

                if let Some(reason) = finish_reason {
                    if reasoning_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: reasoning_index });
                    }
                    if text_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                    }
                    if audio_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: audio_index });
                    }
                    for (_, _, block_idx) in tool_calls.values() {
                        yield Ok(StreamEvent::ContentBlockStop { index: *block_idx });
                    }
                    yield Ok(StreamEvent::MessageStop {
                        stop_reason: parse_finish_reason(reason),
                    });
                }
            }
        })
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        mut config: ProviderConfig,
    ) -> Result<crate::types::Response, ProviderError> {
        if let Some(prefix) = &self.model_prefix
            && !config.model.contains('/')
        {
            config.model = format!("{}{}", prefix, config.model);
        }
        let body = build_request(&messages, &config, false)?;

        let mut req_builder = self
            .shared
            .client
            .post(&self.base_url)
            .bearer_auth(&self.shared.api_key)
            .json(&body);
        if let Some(ms) = config.timeout_ms {
            req_builder = req_builder.timeout(std::time::Duration::from_millis(ms));
        }
        let resp = req_builder.send().await?;

        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;
        let mut response = parse_response(&json)?;
        if config.stop_sequences.len() > 4 {
            response
                .warnings
                .push("stop_sequences truncated to 4 (OpenAI limit)".to_string());
        }
        Ok(response)
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        let mut count_config = config.clone();
        count_config.max_tokens = Some(1);
        let body = build_request(&messages, &count_config, false)?;

        let mut req_builder = self
            .shared
            .client
            .post(&self.base_url)
            .bearer_auth(&self.shared.api_key)
            .json(&body);
        if let Some(ms) = count_config.timeout_ms {
            req_builder = req_builder.timeout(std::time::Duration::from_millis(ms));
        }
        let resp = req_builder.send().await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let input_tokens = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        Ok(TokenCount { input_tokens })
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIChatProvider {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        self.shared.embed(request).await
    }
}

#[async_trait]
impl ImageProvider for OpenAIChatProvider {
    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        self.shared.generate_image(request).await
    }

    async fn edit_image(
        &self,
        request: ImageEditRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        self.shared.edit_image(request).await
    }
}

#[async_trait]
impl AudioProvider for OpenAIChatProvider {
    async fn generate_speech(
        &self,
        request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        self.shared.generate_speech(request).await
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        self.shared.transcribe(request).await
    }

    async fn translate(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        self.shared.translate(request).await
    }
}

#[async_trait]
impl ModerationProvider for OpenAIChatProvider {
    async fn moderate(
        &self,
        request: ModerationRequest,
    ) -> Result<ModerationResponse, ProviderError> {
        self.shared.moderate(request).await
    }
}

mod protocol;
use protocol::{build_request, parse_response};

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "openai_chat_tests.rs"]
mod tests;
