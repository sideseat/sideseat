use std::collections::HashMap;
use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::primitives::Blob;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{ChatProvider, EmbeddingProvider, Provider, ProviderStream},
    providers::{
        openai_common::{OpenAIInnerClient, parse_finish_reason, parse_usage},
        sse::{check_response, sse_data_stream},
    },
    types::{
        ContentBlock, ContentBlockStart, ContentDelta, EmbeddingRequest, EmbeddingResponse,
        ImageContent, MediaSource, Message, ModelInfo, ProviderConfig, ResponseFormat, Role,
        StreamEvent, TokenCount, Tool, ToolChoice, ToolUseBlock,
    },
};

const MISTRAL_CHAT_URL: &str = "https://api.mistral.ai/v1/chat/completions";
const MISTRAL_API_BASE: &str = "https://api.mistral.ai/v1";

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) enum MistralBackend {
    Direct {
        api_key: String,
        chat_url: String,
        api_base: String,
    },
    Bedrock {
        client: Arc<BedrockClient>,
    },
}

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// Mistral AI provider.
///
/// Supports Mistral and Mixtral models via the direct Mistral API or
/// AWS Bedrock (`invoke_model` with Mistral's native format).
///
/// # Direct API
/// ```no_run
/// use sideseat::providers::MistralProvider;
/// let provider = MistralProvider::new("my-api-key");
/// ```
///
/// # AWS Bedrock (IAM)
/// ```no_run
/// # async fn example() {
/// use sideseat::providers::MistralProvider;
/// let provider = MistralProvider::from_bedrock_from_env("us-east-1").await;
/// # }
/// ```
pub struct MistralProvider {
    backend: MistralBackend,
    client: Arc<reqwest::Client>,
}

impl MistralProvider {
    /// Create a provider from the `MISTRAL_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ProviderError> {
        Ok(Self::new(crate::env::require(crate::env::keys::MISTRAL_API_KEY)?))
    }

    /// Create a provider with a direct Mistral API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            backend: MistralBackend::Direct {
                api_key: api_key.into(),
                chat_url: MISTRAL_CHAT_URL.to_string(),
                api_base: MISTRAL_API_BASE.to_string(),
            },
            client: Arc::new(reqwest::Client::new()),
        }
    }

    /// Replace the HTTP client. Useful for custom TLS, proxies, or testing.
    /// Only applies to the Direct backend (Bedrock uses the AWS SDK).
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = Arc::new(client);
        self
    }

    /// Override the API base URL (chat URL and all other endpoints).
    ///
    /// Sets both `{base}/chat/completions` and `{base}` for embeddings/models.
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        let base = base.into();
        if let MistralBackend::Direct { ref mut chat_url, ref mut api_base, .. } = self.backend {
            *chat_url = format!("{}/chat/completions", base);
            *api_base = base;
        }
        self
    }

    /// Create a Bedrock-backed provider using an existing `BedrockClient`.
    pub fn from_bedrock(client: Arc<BedrockClient>) -> Self {
        Self {
            backend: MistralBackend::Bedrock { client },
            client: Arc::new(reqwest::Client::new()),
        }
    }

    /// Create a Bedrock-backed provider using IAM credentials from the environment.
    pub async fn from_bedrock_from_env(region: impl Into<String>) -> Self {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.into()))
            .load()
            .await;
        Self::from_bedrock(Arc::new(BedrockClient::new(&config)))
    }

    /// Create a Bedrock-backed provider using a bearer-token API key.
    ///
    /// See: <https://docs.aws.amazon.com/bedrock/latest/userguide/api-keys.html>
    pub fn with_api_key_bedrock(api_key: impl Into<String>, region: impl Into<String>) -> Self {
        use aws_sdk_bedrockruntime::config::{BehaviorVersion, Region, Token};
        let conf = aws_sdk_bedrockruntime::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region.into()))
            .bearer_token(Token::new(api_key.into(), None))
            .build();
        Self::from_bedrock(Arc::new(BedrockClient::from_conf(conf)))
    }

    /// Create a Bedrock-backed provider using env vars for API key and region.
    ///
    /// Reads `BEDROCK_API_KEY` / `AWS_BEARER_TOKEN_BEDROCK` for the key and
    /// `BEDROCK_REGION` / `AWS_REGION` / `AWS_DEFAULT_REGION` for the region.
    pub fn with_api_key_bedrock_from_env() -> Result<Self, ProviderError> {
        let api_key = crate::env::require(crate::env::keys::BEDROCK_API_KEY)
            .or_else(|_| crate::env::require("AWS_BEARER_TOKEN_BEDROCK"))?;
        let region = crate::env::optional("BEDROCK_REGION")
            .or_else(|| crate::env::optional("AWS_REGION"))
            .or_else(|| crate::env::optional("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|| "us-east-1".to_string());
        Ok(Self::with_api_key_bedrock(api_key, region))
    }
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for MistralProvider {
    fn provider_name(&self) -> &'static str {
        "mistral"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        match &self.backend {
            MistralBackend::Direct { api_key, api_base, .. } => {
                let inner = OpenAIInnerClient::new(api_key.as_str(), Arc::clone(&self.client), api_base.as_str());
                inner.list_models().await
            }
            MistralBackend::Bedrock { .. } => Err(ProviderError::Unsupported(
                "list_models not available for Bedrock backend".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// ChatProvider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl ChatProvider for MistralProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let backend = self.backend.clone();
        let client = Arc::clone(&self.client);

        Box::pin(stream! {
            match &backend {
                MistralBackend::Direct { api_key, chat_url, .. } => {
                    let body = match build_request(&messages, &config, true) {
                        Ok(b) => b,
                        Err(e) => { yield Err(e); return; }
                    };

                    let mut req_builder = client
                        .post(chat_url)
                        .bearer_auth(api_key)
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
                    let mut text_started = false;
                    let mut tool_calls: HashMap<usize, (String, String, usize)> = HashMap::new();
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

                        // Final usage-only chunk
                        if let Some(usage_obj) = parsed.get("usage").filter(|u| !u.is_null()) {
                            let usage = parse_usage(usage_obj);
                            let model = parsed["model"].as_str().map(|s| s.to_string());
                            let id = parsed["id"].as_str().map(|s| s.to_string());
                            yield Ok(StreamEvent::Metadata { usage, model, id });
                            continue;
                        }

                        let choices = match parsed["choices"].as_array() {
                            Some(c) if !c.is_empty() => c,
                            _ => continue,
                        };
                        let choice = &choices[0];
                        let delta = &choice["delta"];
                        let finish_reason = choice["finish_reason"].as_str();

                        // Text content delta
                        if let Some(text) = delta["content"].as_str() {
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

                        // Tool call deltas
                        if let Some(tc_arr) = delta["tool_calls"].as_array() {
                            if text_started {
                                yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                                text_started = false;
                            }
                            let tool_base_index = 1; // text=0, tools start at 1

                            for tc_delta in tc_arr {
                                let stream_idx = tc_delta["index"].as_u64().unwrap_or(0) as usize;
                                let block_idx = tool_base_index + stream_idx;

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
                                        let idx = tool_calls.get(&stream_idx).map(|(_, _, i)| *i).unwrap_or(block_idx);
                                        yield Ok(StreamEvent::ContentBlockDelta {
                                            index: idx,
                                            delta: ContentDelta::ToolInput { partial_json: args.to_string() },
                                        });
                                    }
                                }
                            }
                        }

                        if let Some(reason) = finish_reason {
                            if text_started {
                                yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                            }
                            for (_, _, block_idx) in tool_calls.values() {
                                yield Ok(StreamEvent::ContentBlockStop { index: *block_idx });
                            }
                            yield Ok(StreamEvent::MessageStop {
                                stop_reason: parse_finish_reason(reason),
                            });
                        }
                    }
                }

                MistralBackend::Bedrock { client: bedrock_client } => {
                    let mut body = match build_request(&messages, &config, false) {
                        Ok(b) => b,
                        Err(e) => { yield Err(e); return; }
                    };
                    // Bedrock invoke_model specifies the model via .model_id(); the body
                    // must not include a "model" field (ValidationException otherwise).
                    body.as_object_mut().map(|o| o.remove("model"));
                    let body_bytes = match serde_json::to_vec(&body) {
                        Ok(b) => b,
                        Err(e) => { yield Err(ProviderError::Serialization(e.to_string())); return; }
                    };

                    let send_fut = bedrock_client
                        .invoke_model_with_response_stream()
                        .model_id(&config.model)
                        .content_type("application/json")
                        .accept("application/json")
                        .body(Blob::new(body_bytes))
                        .send();

                    let mut event_stream = if let Some(ms) = config.timeout_ms {
                        match tokio::time::timeout(std::time::Duration::from_millis(ms), send_fut).await {
                            Ok(Ok(r)) => r.body,
                            Ok(Err(e)) => { yield Err(classify_bedrock_error(format!("{e:?}"))); return; }
                            Err(_) => { yield Err(ProviderError::Timeout { ms: Some(ms) }); return; }
                        }
                    } else {
                        match send_fut.await {
                            Ok(r) => r.body,
                            Err(e) => { yield Err(classify_bedrock_error(format!("{e:?}"))); return; }
                        }
                    };

                    yield Ok(StreamEvent::MessageStart { role: Role::Assistant });

                    let text_index: usize = 0;
                    let mut text_started = false;
                    let mut tool_calls: HashMap<usize, (String, String, usize)> = HashMap::new();
                    let mut tool_arg_bufs: HashMap<usize, String> = HashMap::new();

                    use aws_sdk_bedrockruntime::types::ResponseStream;

                    loop {
                        match event_stream.recv().await {
                            Ok(Some(ResponseStream::Chunk(chunk))) => {
                                if let Some(blob) = chunk.bytes {
                                    let data = String::from_utf8_lossy(blob.as_ref()).to_string();
                                    let parsed: Value = match serde_json::from_str(&data) {
                                        Ok(v) => v,
                                        Err(_) => continue,
                                    };

                                    // Usage chunk (final)
                                    if let Some(usage_obj) = parsed.get("usage").filter(|u| !u.is_null())
                                        && parsed["choices"].as_array().map(|c| c.is_empty()).unwrap_or(true)
                                    {
                                        let usage = parse_usage(usage_obj);
                                        let model = parsed["model"].as_str().map(|s| s.to_string());
                                        let id = parsed["id"].as_str().map(|s| s.to_string());
                                        yield Ok(StreamEvent::Metadata { usage, model, id });
                                        continue;
                                    }

                                    let choices = match parsed["choices"].as_array() {
                                        Some(c) if !c.is_empty() => c,
                                        _ => continue,
                                    };
                                    let choice = &choices[0];
                                    let delta = &choice["delta"];
                                    // Bedrock wraps the full response in a single chunk (not deltas);
                                    // fall back to choice["message"]["content"] when delta is absent.
                                    let message_content = choice["message"]["content"].as_str();
                                    let finish_reason = choice["finish_reason"].as_str()
                                        .or_else(|| choice["stop_reason"].as_str());

                                    if let Some(text) = delta["content"].as_str().or(message_content) {
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

                                    if let Some(tc_arr) = delta["tool_calls"].as_array() {
                                        if text_started {
                                            yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                                            text_started = false;
                                        }
                                        let tool_base_index = 1;
                                        for tc_delta in tc_arr {
                                            let stream_idx = tc_delta["index"].as_u64().unwrap_or(0) as usize;
                                            let block_idx = tool_base_index + stream_idx;
                                            if let Some(id) = tc_delta["id"].as_str() {
                                                let name = tc_delta["function"]["name"].as_str().unwrap_or("").to_string();
                                                tool_calls.insert(stream_idx, (id.to_string(), name.clone(), block_idx));
                                                tool_arg_bufs.insert(stream_idx, String::new());
                                                yield Ok(StreamEvent::ContentBlockStart {
                                                    index: block_idx,
                                                    block: ContentBlockStart::ToolUse { id: id.to_string(), name },
                                                });
                                            }
                                            if let Some(args) = tc_delta["function"]["arguments"].as_str() {
                                                if let Some(buf) = tool_arg_bufs.get_mut(&stream_idx) {
                                                    buf.push_str(args);
                                                }
                                                if !args.is_empty() {
                                                    let idx = tool_calls.get(&stream_idx).map(|(_, _, i)| *i).unwrap_or(block_idx);
                                                    yield Ok(StreamEvent::ContentBlockDelta {
                                                        index: idx,
                                                        delta: ContentDelta::ToolInput { partial_json: args.to_string() },
                                                    });
                                                }
                                            }
                                        }
                                    }

                                    if let Some(reason) = finish_reason {
                                        if text_started {
                                            yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                                        }
                                        for (_, _, block_idx) in tool_calls.values() {
                                            yield Ok(StreamEvent::ContentBlockStop { index: *block_idx });
                                        }
                                        // Usage may be in the same chunk as finish_reason
                                        if let Some(usage_obj) = parsed.get("usage").filter(|u| !u.is_null()) {
                                            let usage = parse_usage(usage_obj);
                                            let model = parsed["model"].as_str().map(|s| s.to_string());
                                            let id = parsed["id"].as_str().map(|s| s.to_string());
                                            yield Ok(StreamEvent::MessageStop {
                                                stop_reason: parse_finish_reason(reason),
                                            });
                                            yield Ok(StreamEvent::Metadata { usage, model, id });
                                        } else {
                                            yield Ok(StreamEvent::MessageStop {
                                                stop_reason: parse_finish_reason(reason),
                                            });
                                        }
                                        return;
                                    }
                                }
                            }
                            Ok(Some(_)) => continue, // other event variants
                            Ok(None) => break,
                            Err(e) => {
                                yield Err(classify_bedrock_error(format!("{e:?}")));
                                return;
                            }
                        }
                    }
                }
            }
        })
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<crate::types::Response, ProviderError> {
        match &self.backend {
            MistralBackend::Direct { api_key, chat_url, .. } => {
                let body = build_request(&messages, &config, false)?;

                let mut req_builder = self
                    .client
                    .post(chat_url)
                    .bearer_auth(api_key)
                    .json(&body);
                if let Some(ms) = config.timeout_ms {
                    req_builder = req_builder.timeout(std::time::Duration::from_millis(ms));
                }
                let resp = req_builder.send().await?;
                let resp = check_response(resp).await?;
                let json: Value = resp.json().await?;
                parse_response(&json)
            }

            MistralBackend::Bedrock { client } => {
                let mut body = build_request(&messages, &config, false)?;
                // Bedrock invoke_model specifies the model via .model_id(); the body
                // must not include a "model" field (ValidationException otherwise).
                body.as_object_mut().map(|o| o.remove("model"));
                let body_bytes = serde_json::to_vec(&body)
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;

                let fut = client
                    .invoke_model()
                    .model_id(&config.model)
                    .content_type("application/json")
                    .accept("application/json")
                    .body(Blob::new(body_bytes))
                    .send();

                let resp = if let Some(ms) = config.timeout_ms {
                    tokio::time::timeout(std::time::Duration::from_millis(ms), fut)
                        .await
                        .map_err(|_| ProviderError::Timeout { ms: Some(ms) })?
                        .map_err(|e| classify_bedrock_error(format!("{e:?}")))?
                } else {
                    fut.await.map_err(|e| classify_bedrock_error(format!("{e:?}")))?
                };

                let json: Value = serde_json::from_slice(resp.body.as_ref())
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                parse_response(&json)
            }
        }
    }

    async fn count_tokens(
        &self,
        _messages: Vec<Message>,
        _config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        Err(ProviderError::Unsupported(
            "Mistral does not expose a token counting endpoint".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// EmbeddingProvider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl EmbeddingProvider for MistralProvider {
    async fn embed(
        &self,
        request: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ProviderError> {
        match &self.backend {
            MistralBackend::Direct { api_key, api_base, .. } => {
                let inner = OpenAIInnerClient::new(
                    api_key.as_str(),
                    Arc::clone(&self.client),
                    api_base.as_str(),
                );
                inner.embed(request).await
            }
            MistralBackend::Bedrock { .. } => Err(ProviderError::Unsupported(
                "Embeddings are not available via the Bedrock Mistral backend".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

fn build_request(
    messages: &[Message],
    config: &ProviderConfig,
    stream: bool,
) -> Result<Value, ProviderError> {
    let mistral_messages = format_messages(messages, config.system.as_deref())?;

    let mut req = json!({
        "model": config.model,
        "messages": mistral_messages,
        "stream": stream,
    });

    if stream {
        // Request usage in the final streaming chunk
        req["stream_options"] = json!({"include_usage": true});
    }

    if let Some(max_tokens) = config.max_tokens {
        req["max_tokens"] = json!(max_tokens);
    }
    if let Some(temp) = config.temperature {
        req["temperature"] = json!(temp);
    }
    if let Some(top_p) = config.top_p {
        req["top_p"] = json!(top_p);
    }
    // Mistral uses `random_seed` instead of `seed`
    if let Some(seed) = config.seed {
        req["random_seed"] = json!(seed);
    }
    if !config.stop_sequences.is_empty() {
        req["stop"] = json!(config.stop_sequences);
    }
    if let Some(penalty) = config.presence_penalty {
        req["presence_penalty"] = json!(penalty);
    }
    if let Some(penalty) = config.frequency_penalty {
        req["frequency_penalty"] = json!(penalty);
    }
    if let Some(n) = config.n {
        req["n"] = json!(n);
    }

    if !config.tools.is_empty() {
        req["tools"] = format_tools(&config.tools);
    }
    if let Some(tc) = &config.tool_choice {
        req["tool_choice"] = format_tool_choice(tc);
    }
    if let Some(parallel) = config.parallel_tool_calls
        && !config.tools.is_empty()
    {
        req["parallel_tool_calls"] = json!(parallel);
    }

    match &config.response_format {
        Some(ResponseFormat::Json) => {
            req["response_format"] = json!({"type": "json_object"});
        }
        Some(ResponseFormat::JsonSchema { name, schema, strict }) => {
            let mut s = schema.clone();
            if *strict && let Some(obj) = s.as_object_mut() {
                obj.entry("additionalProperties").or_insert(json!(false));
            }
            req["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": name, "schema": s, "strict": strict }
            });
        }
        Some(ResponseFormat::Text) | None => {}
    }

    // `safe_prompt` and other Mistral-specific params via extra passthrough
    for (k, v) in &config.extra {
        req[k] = v.clone();
    }

    Ok(req)
}

// ---------------------------------------------------------------------------
// Message formatting
// ---------------------------------------------------------------------------

fn format_messages(messages: &[Message], system: Option<&str>) -> Result<Value, ProviderError> {
    let mut result = Vec::new();

    if let Some(sys) = system {
        result.push(json!({"role": "system", "content": sys}));
    }

    for msg in messages {
        let role = match &msg.role {
            Role::System => {
                let content = text_content(&msg.content);
                result.push(json!({"role": "system", "content": content}));
                continue;
            }
            Role::User => "user",
            Role::Tool => "tool",
            Role::Assistant => "assistant",
            Role::Other(s) => s.as_str(),
        };

        // Tool results
        let has_tool_results = msg.content.iter().any(|b| matches!(b, ContentBlock::ToolResult(_)));
        if (role == "user" || role == "tool") && has_tool_results {
            for block in &msg.content {
                if let ContentBlock::ToolResult(tr) = block {
                    let content_str: String = tr
                        .content
                        .iter()
                        .filter_map(|b| if let ContentBlock::Text(t) = b { Some(t.text.as_str()) } else { None })
                        .collect::<Vec<_>>()
                        .join("\n");
                    result.push(json!({
                        "role": "tool",
                        "tool_call_id": tr.tool_use_id,
                        "content": content_str,
                    }));
                }
            }
            let other: Vec<&ContentBlock> = msg.content.iter()
                .filter(|b| !matches!(b, ContentBlock::ToolResult(_)))
                .collect();
            if !other.is_empty() {
                let owned: Vec<ContentBlock> = other.into_iter().cloned().collect();
                result.push(json!({"role": "user", "content": format_content_parts(&owned)?}));
            }
            continue;
        }

        // Assistant tool calls
        if role == "assistant" {
            let tool_uses: Vec<_> = msg.content.iter()
                .filter_map(|b| if let ContentBlock::ToolUse(tu) = b { Some(tu) } else { None })
                .collect();
            if !tool_uses.is_empty() {
                let tool_calls: Vec<Value> = tool_uses.iter().map(|tu| json!({
                    "id": tu.id,
                    "type": "function",
                    "function": { "name": tu.name, "arguments": tu.input.to_string() }
                })).collect();
                let text: String = msg.content.iter()
                    .filter_map(|b| if let ContentBlock::Text(t) = b { Some(t.text.as_str()) } else { None })
                    .collect::<Vec<_>>().join("");
                let mut am = json!({"role": "assistant", "tool_calls": tool_calls});
                if !text.is_empty() {
                    am["content"] = json!(text);
                }
                result.push(am);
                continue;
            }
        }

        result.push(json!({"role": role, "content": format_content_parts(&msg.content)?}));
    }

    Ok(json!(result))
}

fn format_content_parts(blocks: &[ContentBlock]) -> Result<Value, ProviderError> {
    if blocks.len() == 1 && let ContentBlock::Text(t) = &blocks[0] {
        return Ok(json!(t.text));
    }
    let parts: Result<Vec<Value>, _> = blocks.iter().map(format_content_part).collect();
    Ok(json!(parts?))
}

fn format_content_part(block: &ContentBlock) -> Result<Value, ProviderError> {
    match block {
        ContentBlock::Text(t) => Ok(json!({"type": "text", "text": t.text})),
        ContentBlock::Image(img) => format_image_part(img),
        ContentBlock::ToolResult(_) | ContentBlock::ToolUse(_) => Ok(json!(null)),
        _ => Err(ProviderError::Unsupported(
            "Content type not supported in Mistral messages".into(),
        )),
    }
}

fn format_image_part(img: &ImageContent) -> Result<Value, ProviderError> {
    let url = match &img.source {
        MediaSource::Url(u) => u.clone(),
        MediaSource::Base64(b64) => {
            let mime = match img.format {
                Some(crate::types::ImageFormat::Jpeg) => "image/jpeg",
                Some(crate::types::ImageFormat::Webp) => "image/webp",
                Some(crate::types::ImageFormat::Gif) => "image/gif",
                _ => "image/png",
            };
            format!("data:{};base64,{}", mime, b64.data)
        }
        _ => return Err(ProviderError::Unsupported(
            "Mistral only supports URL or base64 image sources".into(),
        )),
    };
    Ok(json!({"type": "image_url", "image_url": {"url": url}}))
}

fn text_content(blocks: &[ContentBlock]) -> String {
    blocks.iter()
        .filter_map(|b| if let ContentBlock::Text(t) = b { Some(t.text.as_str()) } else { None })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Tool formatting
// ---------------------------------------------------------------------------

fn format_tools(tools: &[Tool]) -> Value {
    let arr: Vec<Value> = tools.iter().map(|t| json!({
        "type": "function",
        "function": {
            "name": t.name,
            "description": t.description,
            "parameters": t.input_schema,
        }
    })).collect();
    json!(arr)
}

fn format_tool_choice(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Any => json!("any"),
        ToolChoice::Tool { name } => json!({"type": "function", "function": {"name": name}}),
        // Mistral doesn't support restricting to a subset; fall back to required
        ToolChoice::AllowedTools { .. } => json!("any"),
    }
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

fn parse_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let choices = json["choices"].as_array().ok_or_else(|| {
        ProviderError::Serialization("Mistral response missing 'choices'".into())
    })?;
    if choices.is_empty() {
        return Err(ProviderError::Serialization("Mistral response has empty 'choices'".into()));
    }

    let message = &choices[0]["message"];
    let finish_reason = choices[0]["finish_reason"]
        .as_str()
        .or_else(|| choices[0]["stop_reason"].as_str())
        .unwrap_or("stop");
    let mut content: Vec<ContentBlock> = Vec::new();

    if let Some(text) = message["content"].as_str()
        && !text.is_empty()
    {
        content.push(ContentBlock::text(text));
    }

    if let Some(tool_calls) = message["tool_calls"].as_array() {
        for tc in tool_calls {
            let id = tc["id"].as_str().unwrap_or("").to_string();
            let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let input = serde_json::from_str(args_str).unwrap_or(Value::Null);
            content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
        }
    }

    let usage = parse_usage(&json["usage"]);
    let stop_reason = parse_finish_reason(finish_reason);
    let model = json["model"].as_str().map(|s| s.to_string());
    let id = json["id"].as_str().map(|s| s.to_string());

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model,
        id,
        container: None,
        logprobs: None,
        grounding_metadata: None,
        warnings: vec![],
    })
}

// ---------------------------------------------------------------------------
// Bedrock error classification
// ---------------------------------------------------------------------------

fn classify_bedrock_error(msg: String) -> ProviderError {
    if msg.contains("ThrottlingException") || msg.to_lowercase().contains("throttl") {
        ProviderError::TooManyRequests { message: msg, retry_after_secs: None }
    } else if msg.contains("ModelTimeoutException") {
        ProviderError::Timeout { ms: None }
    } else if msg.contains("ValidationException") {
        ProviderError::Api { status: 400, message: msg }
    } else {
        ProviderError::Api { status: 0, message: msg }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StopReason;
    use serde_json::json;

    #[test]
    fn test_build_request_random_seed() {
        let messages = vec![Message::user("Hello")];
        let mut config = ProviderConfig::new("mistral-large-latest").with_max_tokens(100);
        config.seed = Some(42);
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["random_seed"], json!(42));
        assert!(req.get("seed").is_none(), "must use random_seed, not seed");
    }

    #[test]
    fn test_build_request_no_top_k() {
        let messages = vec![Message::user("Hello")];
        let mut config = ProviderConfig::new("mistral-large-latest");
        config.top_k = Some(50);
        let req = build_request(&messages, &config, false).unwrap();
        assert!(req.get("top_k").is_none(), "Mistral does not support top_k");
    }

    #[test]
    fn test_build_request_response_format_json() {
        let messages = vec![Message::user("Hello")];
        let mut config = ProviderConfig::new("mistral-large-latest");
        config.response_format = Some(ResponseFormat::Json);
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["response_format"]["type"], "json_object");
    }

    #[test]
    fn test_build_request_response_format_json_schema() {
        let messages = vec![Message::user("Hello")];
        let mut config = ProviderConfig::new("mistral-large-latest");
        config.response_format = Some(ResponseFormat::JsonSchema {
            name: "Answer".to_string(),
            schema: json!({"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"]}),
            strict: true,
        });
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["response_format"]["type"], "json_schema");
        assert_eq!(req["response_format"]["json_schema"]["name"], "Answer");
        assert_eq!(req["response_format"]["json_schema"]["strict"], true);
    }

    #[test]
    fn test_build_request_safe_prompt_via_extra() {
        let messages = vec![Message::user("Hello")];
        let mut config = ProviderConfig::new("mistral-large-latest");
        config.extra.insert("safe_prompt".to_string(), json!(true));
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["safe_prompt"], json!(true));
    }

    #[test]
    fn test_format_messages_system() {
        let messages = vec![Message::user("Hi")];
        let result = format_messages(&messages, Some("You are helpful.")).unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[0]["content"], "You are helpful.");
        assert_eq!(arr[1]["role"], "user");
    }

    #[test]
    fn test_format_messages_tool_result() {
        use crate::types::{ContentBlock, ToolResultBlock};
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: "call_1".to_string(),
                content: vec![ContentBlock::text("42 degrees")],
                is_error: false,
            })],
            name: None,
            cache_control: None,
        }];
        let result = format_messages(&messages, None).unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr[0]["role"], "tool");
        assert_eq!(arr[0]["tool_call_id"], "call_1");
        assert_eq!(arr[0]["content"], "42 degrees");
    }

    #[test]
    fn test_parse_response_text() {
        let json = json!({
            "id": "msg-123",
            "model": "mistral-large-latest",
            "choices": [{
                "message": {"role": "assistant", "content": "Hello!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t.text == "Hello!"));
        assert_eq!(resp.usage.input_tokens, 5);
        assert_eq!(resp.usage.output_tokens, 3);
        assert_eq!(resp.id.as_deref(), Some("msg-123"));
        assert!(matches!(resp.stop_reason, StopReason::EndTurn));
    }

    #[test]
    fn test_parse_response_tool_call() {
        let json = json!({
            "id": "msg-456",
            "model": "mistral-large-latest",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "D681PevKs",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 15, "completion_tokens": 8, "total_tokens": 23}
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather" && tu.id == "D681PevKs"));
        assert!(matches!(resp.stop_reason, StopReason::ToolUse));
    }

    #[test]
    fn test_format_tool_choice() {
        assert_eq!(format_tool_choice(&ToolChoice::Auto), json!("auto"));
        assert_eq!(format_tool_choice(&ToolChoice::None), json!("none"));
        assert_eq!(format_tool_choice(&ToolChoice::Any), json!("any"));
        assert_eq!(
            format_tool_choice(&ToolChoice::Tool { name: "my_fn".into() }),
            json!({"type": "function", "function": {"name": "my_fn"}})
        );
    }
}
