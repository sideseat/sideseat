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

// ---------------------------------------------------------------------------
// Request building (free function)
// ---------------------------------------------------------------------------

fn build_request(
    messages: &[Message],
    config: &ProviderConfig,
    stream: bool,
) -> Result<Value, ProviderError> {
    let openai_messages = format_messages(
        messages,
        config.system.as_deref(),
        config.inject_system_as_user_message,
    )?;

    let mut req = json!({
        "model": config.model,
        "messages": openai_messages,
        "stream": stream,
    });

    if stream {
        req["stream_options"] = json!({"include_usage": true});
    }

    if let Some(max_tokens) = config.max_tokens {
        req["max_completion_tokens"] = json!(max_tokens);
    }
    if let Some(temp) = config.temperature {
        req["temperature"] = json!(temp);
    }
    if let Some(top_p) = config.top_p {
        req["top_p"] = json!(top_p);
    }
    if let Some(seed) = config.seed {
        req["seed"] = json!(seed);
    }
    if !config.stop_sequences.is_empty() {
        let stop = if config.stop_sequences.len() > 4 {
            tracing::warn!(
                "OpenAI supports at most 4 stop sequences; truncating {} to 4",
                config.stop_sequences.len()
            );
            &config.stop_sequences[..4]
        } else {
            &config.stop_sequences[..]
        };
        req["stop"] = json!(stop);
    }
    if let Some(effort) = &config.reasoning_effort {
        req["reasoning_effort"] = json!(effort.as_str());
    }
    if let Some(tier) = &config.service_tier {
        req["service_tier"] = json!(tier.as_str());
    }

    if !config.tools.is_empty() {
        req["tools"] = format_tools(&config.tools);
    }
    // Built-in web search tool
    if let Some(ws) = &config.web_search {
        let tools = req["tools"].as_array_mut().cloned().unwrap_or_default();
        let mut all_tools = tools;
        all_tools.push(format_web_search_tool(ws));
        req["tools"] = json!(all_tools);
    }
    if let Some(tc) = &config.tool_choice {
        req["tool_choice"] = format_tool_choice(tc);
    }

    // Response format
    if let Some(fmt) = &config.response_format {
        req["response_format"] = format_response_format(fmt);
    } else if let Some(schema_val) = config.extra.get("output_schema") {
        // Legacy: support extra["output_schema"] for backward compat.
        // Accepts either a raw schema { "type": "object", ... } or an envelope
        // { "name": "...", "schema": { "type": "object", ... } }.
        let name = schema_val["name"].as_str().unwrap_or("structured_output");
        let inner_schema = schema_val.get("schema").unwrap_or(schema_val);
        req["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {
                "name": name,
                "schema": inner_schema,
                "strict": true,
            }
        });
    }

    if let Some(user) = &config.user {
        req["user"] = json!(user);
    }
    if let Some(penalty) = config.presence_penalty {
        req["presence_penalty"] = json!(penalty);
    }
    if let Some(penalty) = config.frequency_penalty {
        req["frequency_penalty"] = json!(penalty);
    }
    if let Some(ref bias) = config.logit_bias
        && !bias.is_empty()
    {
        req["logit_bias"] = json!(bias);
    }
    if let Some(parallel) = config.parallel_tool_calls
        && (!config.tools.is_empty() || config.web_search.is_some())
    {
        req["parallel_tool_calls"] = json!(parallel);
    }
    if let Some(n) = config.n {
        req["n"] = json!(n);
    }
    if let Some(logprobs) = config.logprobs {
        req["logprobs"] = json!(logprobs);
    }
    if let Some(top_n) = config.top_logprobs {
        req["top_logprobs"] = json!(top_n);
        // logprobs must be true for top_logprobs to be accepted
        if config.logprobs.is_none() {
            req["logprobs"] = json!(true);
        }
    }
    if let Some(store) = config.store {
        req["store"] = json!(store);
    }
    if let Some(retention) = &config.prompt_cache_retention {
        req["prompt_cache_retention"] = json!(retention);
    }
    if let Some(cache_key) = &config.prompt_cache_key {
        req["prompt_cache_key"] = json!(cache_key);
    }
    if let Some(audio) = &config.audio_output {
        req["modalities"] = json!(["text", "audio"]);
        let mut audio_obj = json!({"voice": audio.voice});
        if let Some(fmt) = &audio.format {
            audio_obj["format"] = serde_json::to_value(fmt).unwrap_or_else(|_| json!("mp3"));
        }
        req["audio"] = audio_obj;
    }

    for (k, v) in &config.extra {
        if k != "output_schema" {
            req[k] = v.clone();
        }
    }

    Ok(req)
}

fn format_response_format(fmt: &ResponseFormat) -> Value {
    match fmt {
        ResponseFormat::Text => json!({"type": "text"}),
        ResponseFormat::Json => json!({"type": "json_object"}),
        ResponseFormat::JsonSchema {
            name,
            schema,
            strict,
        } => {
            let mut s = schema.clone();
            // Add additionalProperties: false for strict validation
            if *strict && let Some(obj) = s.as_object_mut() {
                obj.entry("additionalProperties").or_insert(json!(false));
            }
            json!({
                "type": "json_schema",
                "json_schema": {
                    "name": name,
                    "schema": s,
                    "strict": strict,
                }
            })
        }
    }
}

fn format_web_search_tool(ws: &WebSearchConfig) -> Value {
    let mut tool = json!({"type": "web_search_preview"});
    if let Some(allowed) = &ws.allowed_domains {
        tool["allowed_domains"] = json!(allowed);
    }
    if let Some(blocked) = &ws.blocked_domains {
        tool["blocked_domains"] = json!(blocked);
    }
    if let Some(ctx_size) = &ws.search_context_size {
        tool["search_context_size"] = json!(ctx_size);
    }
    if let Some(loc) = &ws.user_location {
        tool["user_location"] = serde_json::to_value(loc).unwrap_or(serde_json::Value::Null);
    }
    tool
}

// ---------------------------------------------------------------------------
// Message formatting
// ---------------------------------------------------------------------------

fn format_messages(
    messages: &[Message],
    system: Option<&str>,
    inject_system_as_user: bool,
) -> Result<Value, ProviderError> {
    let mut result = Vec::new();

    if let Some(sys) = system {
        if inject_system_as_user {
            result.push(json!({"role": "user", "content": format!("<system>{}</system>", sys)}));
        } else {
            result.push(json!({"role": "system", "content": sys}));
        }
    }

    for msg in messages {
        let role = match &msg.role {
            Role::System => {
                let sys_content = format_content(&msg.content)?;
                if inject_system_as_user {
                    let text = sys_content.as_str().unwrap_or("");
                    result.push(
                        json!({"role": "user", "content": format!("<system>{}</system>", text)}),
                    );
                } else {
                    result.push(json!({"role": "system", "content": sys_content}));
                }
                continue;
            }
            Role::User => "user",
            Role::Tool => "tool",
            Role::Assistant => "assistant",
            Role::Other(s) => s.as_str(),
        };

        // Tool results → role=tool messages
        let has_tool_results = msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult(_)));
        if (role == "user" || role == "tool") && has_tool_results {
            let mut other_content: Vec<&ContentBlock> = Vec::new();
            for block in &msg.content {
                match block {
                    ContentBlock::ToolResult(tr) => {
                        let content_str: String = tr
                            .content
                            .iter()
                            .filter_map(|b| {
                                if let ContentBlock::Text(t) = b {
                                    Some(t.text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        result.push(json!({
                            "role": "tool",
                            "tool_call_id": tr.tool_use_id,
                            "content": content_str,
                        }));
                    }
                    other => other_content.push(other),
                }
            }
            if !other_content.is_empty() {
                let owned: Vec<ContentBlock> = other_content.into_iter().cloned().collect();
                result.push(json!({"role": "user", "content": format_content(&owned)?}));
            }
            continue;
        }

        // Tool calls in assistant messages
        if role == "assistant" {
            let tool_uses: Vec<_> = msg
                .content
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::ToolUse(tu) = b {
                        Some(tu)
                    } else {
                        None
                    }
                })
                .collect();
            if !tool_uses.is_empty() {
                let tool_calls: Vec<Value> = tool_uses
                    .iter()
                    .map(|tu| {
                        json!({
                            "id": tu.id,
                            "type": "function",
                            "function": {"name": tu.name, "arguments": tu.input.to_string()}
                        })
                    })
                    .collect();
                let text: String = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Text(t) = b {
                            Some(t.text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let mut am = json!({"role": "assistant", "tool_calls": tool_calls});
                if !text.is_empty() {
                    am["content"] = json!(text);
                }
                result.push(am);
                continue;
            }
        }

        let mut m = json!({"role": role, "content": format_content(&msg.content)?});
        if let Some(name) = &msg.name {
            m["name"] = json!(name);
        }
        result.push(m);
    }

    Ok(json!(result))
}

fn format_content(blocks: &[ContentBlock]) -> Result<Value, ProviderError> {
    if blocks.len() == 1
        && let ContentBlock::Text(t) = &blocks[0]
    {
        return Ok(json!(t.text));
    }
    let parts: Result<Vec<Value>, _> = blocks.iter().map(format_content_part).collect();
    // ToolResult/ToolUse are handled by specialised paths in format_messages; filter their
    // null sentinels so they never appear in the final content array sent to OpenAI.
    let parts: Vec<Value> = parts?.into_iter().filter(|v| !v.is_null()).collect();
    Ok(json!(parts))
}

fn format_content_part(block: &ContentBlock) -> Result<Value, ProviderError> {
    match block {
        ContentBlock::Text(t) => Ok(json!({"type": "text", "text": t.text})),
        ContentBlock::Image(img) => format_image_part(img),
        ContentBlock::Audio(audio) => match &audio.source {
            MediaSource::Base64(b64) => Ok(json!({
                "type": "input_audio",
                "input_audio": {
                    "data": b64.data,
                    "format": audio_format_str(&audio.format),
                }
            })),
            _ => Err(ProviderError::Unsupported(
                "OpenAI audio requires base64 source".into(),
            )),
        },
        ContentBlock::ToolResult(_) | ContentBlock::ToolUse(_) => Ok(json!(null)),
        _ => Err(ProviderError::Unsupported(
            "Content type not supported in OpenAI Chat messages".into(),
        )),
    }
}

fn format_image_part(img: &ImageContent) -> Result<Value, ProviderError> {
    use crate::types::ImageDetail;
    let detail = match img.detail.as_ref().unwrap_or(&ImageDetail::Auto) {
        ImageDetail::Auto => "auto",
        ImageDetail::Low => "low",
        ImageDetail::High => "high",
    };
    match &img.source {
        MediaSource::Url(url) => Ok(json!({
            "type": "image_url",
            "image_url": {"url": url, "detail": detail}
        })),
        MediaSource::Base64(b64) => {
            let data_url = format!("data:{};base64,{}", b64.media_type, b64.data);
            Ok(json!({
                "type": "image_url",
                "image_url": {"url": data_url, "detail": detail}
            }))
        }
        _ => Err(ProviderError::Unsupported(
            "OpenAI images require URL or base64 source".into(),
        )),
    }
}

fn audio_format_str(format: &crate::types::AudioFormat) -> &'static str {
    use crate::types::AudioFormat;
    match format {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Wav => "wav",
        AudioFormat::Aac => "aac",
        AudioFormat::Flac => "flac",
        AudioFormat::Ogg => "ogg",
        AudioFormat::Webm => "webm",
        AudioFormat::Opus => "opus",
        _ => "mp3",
    }
}

fn format_tools(tools: &[Tool]) -> Value {
    json!(
        tools
            .iter()
            .map(|t| {
                let mut schema = t.input_schema.clone();
                if t.strict
                    && let Some(obj) = schema.as_object_mut()
                {
                    obj.entry("additionalProperties").or_insert(json!(false));
                }
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": schema,
                        "strict": t.strict,
                    }
                })
            })
            .collect::<Vec<_>>()
    )
}

fn format_tool_choice(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Any => json!("required"),
        ToolChoice::None => json!("none"),
        ToolChoice::Tool { name } => json!({"type": "function", "function": {"name": name}}),
        // OpenAI Chat Completions has no native "allowed tools" directive.
        // Best approximation: auto mode — the model may call any tool in the list.
        // Callers that need strict restriction should filter config.tools before calling.
        ToolChoice::AllowedTools { .. } => json!("auto"),
    }
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

fn parse_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let choice = &json["choices"][0];
    let message = &choice["message"];
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop");

    let mut content: Vec<ContentBlock> = Vec::new();

    // Reasoning content (DeepSeek, xAI) — emitted before text
    if let Some(thinking) = message["reasoning_content"].as_str()
        && !thinking.is_empty()
    {
        use crate::types::ThinkingBlock;
        content.push(ContentBlock::Thinking(ThinkingBlock {
            text: thinking.to_string(),
            signature: None,
        }));
    }

    if let Some(text) = message["content"].as_str()
        && !text.is_empty()
    {
        content.push(ContentBlock::text(text));
    }

    // Audio output (gpt-4o-audio-preview)
    if let Some(audio_data) = message["audio"]["data"].as_str()
        && !audio_data.is_empty()
    {
        content.push(ContentBlock::Audio(AudioContent {
            source: MediaSource::base64("audio/mpeg", audio_data),
            format: AudioFormat::Mp3,
        }));
    }

    if let Some(tool_calls) = message["tool_calls"].as_array() {
        for tc in tool_calls {
            let id = tc["id"].as_str().unwrap_or("").to_string();
            let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");

            // OpenAI sometimes hallucinates a `multi_tool_use.parallel` wrapper
            // instead of returning real parallel tool calls. Expand it.
            if name == "multi_tool_use.parallel" {
                let wrapper: Value = serde_json::from_str(args_str).unwrap_or(Value::Null);
                if let Some(uses) = wrapper["tool_uses"].as_array() {
                    for (i, use_item) in uses.iter().enumerate() {
                        let real_name = use_item["recipient_name"]
                            .as_str()
                            .unwrap_or("")
                            .trim_start_matches("functions.")
                            .to_string();
                        let real_input = use_item["parameters"].clone();
                        content.push(ContentBlock::ToolUse(ToolUseBlock {
                            id: format!("{}_{}", id, i),
                            name: real_name,
                            input: real_input,
                        }));
                    }
                    continue;
                }
            }

            let input = serde_json::from_str(args_str).unwrap_or(Value::Null);
            content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
        }
    }

    let usage = parse_usage(&json["usage"]);
    let stop_reason = parse_finish_reason(finish_reason);
    let model = json["model"].as_str().map(|s| s.to_string());
    let id = json["id"].as_str().map(|s| s.to_string());
    let logprobs = parse_logprobs(&choice["logprobs"]);

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model,
        id,
        container: None,
        logprobs,
        grounding_metadata: None,
        warnings: vec![],
    })
}

fn parse_logprobs(logprobs_val: &Value) -> Option<Vec<crate::types::TokenLogprob>> {
    let content = logprobs_val["content"].as_array()?;
    let tokens: Vec<crate::types::TokenLogprob> = content
        .iter()
        .map(|item| {
            let top_logprobs = item["top_logprobs"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|t| crate::types::TopLogprob {
                            token: t["token"].as_str().unwrap_or("").to_string(),
                            logprob: t["logprob"].as_f64().unwrap_or(0.0),
                            bytes: t["bytes"].as_array().map(|b| {
                                b.iter()
                                    .filter_map(|v| v.as_u64().map(|n| n as u8))
                                    .collect()
                            }),
                        })
                        .collect()
                })
                .unwrap_or_default();
            crate::types::TokenLogprob {
                token: item["token"].as_str().unwrap_or("").to_string(),
                logprob: item["logprob"].as_f64().unwrap_or(0.0),
                bytes: item["bytes"].as_array().map(|b| {
                    b.iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u8))
                        .collect()
                }),
                top_logprobs,
            }
        })
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AudioFormat, AudioOutputConfig, StopReason};
    use serde_json::json;

    #[test]
    fn test_build_request_basic() {
        let config = ProviderConfig::new("gpt-4.1").with_max_tokens(512);
        let messages = vec![Message::user("Hello")];
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["model"], "gpt-4.1");
        assert_eq!(req["max_completion_tokens"], 512);
        assert_eq!(req["messages"][0]["role"], "user");
    }

    #[test]
    fn test_system_injection() {
        let config = ProviderConfig::new("gpt-4.1")
            .with_system("Be helpful")
            .with_max_tokens(256);
        let messages = vec![Message::user("Hi")];
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["messages"][0]["role"], "system");
        assert_eq!(req["messages"][0]["content"], "Be helpful");
        assert_eq!(req["messages"][1]["role"], "user");
    }

    #[test]
    fn test_tool_choice_required() {
        let config = ProviderConfig::new("gpt-4.1")
            .with_tools(vec![Tool::new(
                "search",
                "Search",
                json!({"type": "object"}),
            )])
            .with_tool_choice(ToolChoice::Any);
        let messages = vec![Message::user("Search")];
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["tool_choice"], "required");
    }

    #[test]
    fn test_logprobs() {
        let config = ProviderConfig::new("gpt-4.1")
            .with_logprobs(true)
            .with_top_logprobs(3);
        let req = build_request(&[Message::user("Hi")], &config, false).unwrap();
        assert_eq!(req["logprobs"], true);
        assert_eq!(req["top_logprobs"], 3);
    }

    #[test]
    fn test_top_logprobs_implies_logprobs() {
        // When only top_logprobs is set, logprobs=true must be injected automatically
        let config = ProviderConfig::new("gpt-4.1").with_top_logprobs(5);
        let req = build_request(&[Message::user("Hi")], &config, false).unwrap();
        assert_eq!(req["logprobs"], true);
        assert_eq!(req["top_logprobs"], 5);
    }

    #[test]
    fn test_store() {
        let config = ProviderConfig::new("gpt-4.1").with_store(false);
        let req = build_request(&[Message::user("Hi")], &config, false).unwrap();
        assert_eq!(req["store"], false);
    }

    #[test]
    fn test_audio_output() {
        let config = ProviderConfig::new("gpt-4o-audio-preview")
            .with_audio_output(AudioOutputConfig::new("alloy").with_format(AudioFormat::Wav));
        let req = build_request(&[Message::user("Say hello")], &config, false).unwrap();
        assert_eq!(req["modalities"], json!(["text", "audio"]));
        assert_eq!(req["audio"]["voice"], "alloy");
        assert_eq!(req["audio"]["format"], "wav");
    }

    #[test]
    fn test_audio_output_pcm16() {
        let config = ProviderConfig::new("gpt-4o-audio-preview")
            .with_audio_output(AudioOutputConfig::new("nova").with_format(AudioFormat::Pcm16));
        let req = build_request(&[Message::user("Say hi")], &config, false).unwrap();
        assert_eq!(req["audio"]["format"], "pcm16");
    }

    #[test]
    fn test_audio_output_no_format() {
        // When no format specified, audio object has only voice — no format key
        let config = ProviderConfig::new("gpt-4o-audio-preview")
            .with_audio_output(AudioOutputConfig::new("shimmer"));
        let req = build_request(&[Message::user("Hello")], &config, false).unwrap();
        assert_eq!(req["modalities"], json!(["text", "audio"]));
        assert_eq!(req["audio"]["voice"], "shimmer");
        assert!(req["audio"]["format"].is_null());
    }

    #[test]
    fn test_structured_output() {
        let mut config = ProviderConfig::new("gpt-4.1").with_max_tokens(256);
        config.extra.insert(
            "output_schema".to_string(),
            json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}),
        );
        let messages = vec![Message::user("Give me a name")];
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["response_format"]["type"], "json_schema");
    }

    #[test]
    fn test_parse_response() {
        let json = json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hello there!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 8, "completion_tokens": 4, "total_tokens": 12}
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello there!"));
        assert_eq!(resp.usage.input_tokens, 8);
    }

    #[test]
    fn test_parse_tool_call() {
        let json = json!({
            "id": "chatcmpl-456",
            "object": "chat.completion",
            "model": "gpt-4.1",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"location\":\"NYC\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 10, "total_tokens": 30}
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather"));
        assert!(matches!(resp.stop_reason, StopReason::ToolUse));
    }

    #[tokio::test]
    async fn test_integration_complete() {
        let api_key = match std::env::var("OPENAI_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: OPENAI_API_KEY not set");
                return;
            }
        };
        let provider = OpenAIChatProvider::new(api_key);
        let config = ProviderConfig::new("gpt-4o-mini").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hello' in one word.")];
        let resp = provider.complete(messages, config).await.unwrap();
        assert!(!resp.content.is_empty());
    }

    #[tokio::test]
    async fn test_integration_stream() {
        let api_key = match std::env::var("OPENAI_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: OPENAI_API_KEY not set");
                return;
            }
        };
        use crate::provider::collect_stream;
        let provider = OpenAIChatProvider::new(api_key);
        let config = ProviderConfig::new("gpt-4o-mini").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hi'.")];
        let stream = provider.stream(messages, config);
        let resp = collect_stream(stream).await.unwrap();
        assert!(!resp.content.is_empty());
    }
}
