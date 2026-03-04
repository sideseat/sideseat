use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{Provider, ProviderStream},
    providers::sse::{check_response, sse_data_stream},
    types::{
        ContentBlock, ContentBlockStart, ContentDelta, DocumentContent, EmbeddingRequest,
        EmbeddingResponse, ImageContent, MediaSource, Message, ModelInfo, ProviderConfig,
        ResponseFormat, Role, StopReason, StreamEvent, ThinkingBlock, TokenCount, ToolChoice,
        ToolUseBlock, Usage,
    },
};

const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const BEDROCK_ANTHROPIC_VERSION: &str = "bedrock-2023-05-31";
const VERTEX_ANTHROPIC_VERSION: &str = "vertex-2023-10-16";

// ---------------------------------------------------------------------------
// Backend enum
// ---------------------------------------------------------------------------

/// Selects which backend the `AnthropicProvider` uses to send requests.
#[derive(Clone)]
pub enum AnthropicBackend {
    /// Direct Anthropic API — uses `X-Api-Key` header.
    Direct { api_key: String, base_url: String },
    /// AWS Bedrock — Anthropic Messages API format via `invoke_model_with_response_stream`.
    Bedrock {
        client: Arc<BedrockClient>,
        region: String,
    },
    /// Google Vertex AI — Anthropic Messages API format via SSE to aiplatform.googleapis.com.
    Vertex {
        project_id: String,
        location: String,
        access_token: String,
    },
}

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// Anthropic Claude provider.
///
/// Supports all three deployment targets:
/// - Direct Anthropic API (`AnthropicBackend::Direct`)
/// - AWS Bedrock (`AnthropicBackend::Bedrock`)
/// - Google Vertex AI (`AnthropicBackend::Vertex`)
///
/// All backends accept the same `ProviderConfig` and `Message` types.
///
/// # Beta headers
///
/// Pass `betas` to enable experimental Anthropic features:
/// ```no_run
/// use sideseat::providers::AnthropicProvider;
/// let provider = AnthropicProvider::new("key")
///     .with_betas(vec!["files-api-2025-04-14".into()]);
/// ```
pub struct AnthropicProvider {
    backend: AnthropicBackend,
    client: Arc<reqwest::Client>,
    /// Beta feature names sent as `anthropic-beta: b1,b2,...`
    betas: Vec<String>,
}

impl AnthropicProvider {
    /// Create a direct Anthropic API provider.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            backend: AnthropicBackend::Direct {
                api_key: api_key.into(),
                base_url: ANTHROPIC_API_BASE.to_string(),
            },
            client: Arc::new(reqwest::Client::new()),
            betas: Vec::new(),
        }
    }

    /// Override the API base URL.  Only applies to the `Direct` backend.
    ///
    /// For use with LiteLLM or other Anthropic-compatible proxies:
    /// ```no_run
    /// use sideseat::providers::AnthropicProvider;
    /// // LiteLLM proxy running locally
    /// let p = AnthropicProvider::new("sk-xxx").with_base_url("http://0.0.0.0:4000/v1");
    /// ```
    /// The `/messages` path is appended automatically to the base URL for all requests.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        if let AnthropicBackend::Direct {
            base_url: ref mut u,
            ..
        } = self.backend
        {
            *u = base_url.into();
        }
        self
    }

    /// Add beta feature names. These are sent as `anthropic-beta: b1,b2,...`.
    pub fn with_betas(mut self, betas: Vec<String>) -> Self {
        self.betas = betas;
        self
    }

    /// Create a provider backed by AWS Bedrock (Anthropic Messages API format).
    pub fn from_bedrock(client: Arc<BedrockClient>, region: impl Into<String>) -> Self {
        Self {
            backend: AnthropicBackend::Bedrock {
                client,
                region: region.into(),
            },
            client: Arc::new(reqwest::Client::new()),
            betas: Vec::new(),
        }
    }

    /// Create a provider backed by Google Vertex AI (Anthropic Messages API format).
    pub fn from_vertex(
        project_id: impl Into<String>,
        location: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Self {
        Self {
            backend: AnthropicBackend::Vertex {
                project_id: project_id.into(),
                location: location.into(),
                access_token: access_token.into(),
            },
            client: Arc::new(reqwest::Client::new()),
            betas: Vec::new(),
        }
    }

    // ---- helpers -----------------------------------------------------------

    fn vertex_url(project_id: &str, location: &str, model: &str, stream: bool) -> String {
        let method = if stream {
            "streamRawPredict"
        } else {
            "rawPredict"
        };
        format!(
            "https://{location}-aiplatform.googleapis.com/v1/projects/{project_id}/locations/{location}/publishers/anthropic/models/{model}:{method}"
        )
    }

    fn add_direct_headers(
        req: reqwest::RequestBuilder,
        api_key: &str,
        betas: &[String],
    ) -> reqwest::RequestBuilder {
        let mut r = req
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");
        if !betas.is_empty() {
            r = r.header("anthropic-beta", betas.join(","));
        }
        r
    }

    async fn direct_complete(
        client: &reqwest::Client,
        api_key: &str,
        base_url: &str,
        betas: &[String],
        body: Value,
    ) -> Result<Value, ProviderError> {
        let req =
            Self::add_direct_headers(client.post(format!("{base_url}/messages")), api_key, betas);
        let resp = req.json(&body).send().await?;
        let resp = check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn vertex_complete(
        client: &reqwest::Client,
        project_id: &str,
        location: &str,
        access_token: &str,
        model: &str,
        body: Value,
    ) -> Result<Value, ProviderError> {
        let url = Self::vertex_url(project_id, location, model, false);
        let resp = client
            .post(url)
            .bearer_auth(access_token)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        Ok(resp.json().await?)
    }
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for AnthropicProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let backend = self.backend.clone();
        let client = Arc::clone(&self.client);
        let mut betas = self.betas.clone();
        // Auto-add required betas based on features in use
        for b in compute_auto_betas(&config, &messages) {
            if !betas.contains(&b) {
                betas.push(b);
            }
        }

        Box::pin(stream! {
            match &backend {
                AnthropicBackend::Direct { api_key, base_url } => {
                    let body = match build_messages_request(&messages, &config, true) {
                        Ok(b) => b,
                        Err(e) => { yield Err(e); return; }
                    };

                    let req = AnthropicProvider::add_direct_headers(
                        client.post(format!("{base_url}/messages")),
                        api_key,
                        &betas,
                    );
                    let resp = match req.json(&body).send().await {
                        Ok(r) => r,
                        Err(e) => { yield Err(e.into()); return; }
                    };
                    let resp = match check_response(resp).await {
                        Ok(r) => r,
                        Err(e) => { yield Err(e); return; }
                    };

                    let mut data_stream = Box::pin(sse_data_stream(resp));
                    use futures::StreamExt;

                    while let Some(result) = data_stream.next().await {
                        let data = match result {
                            Ok(d) => d,
                            Err(e) => { yield Err(e); return; }
                        };
                        for event in parse_sse_events(&data) {
                            yield event;
                        }
                    }
                }

                AnthropicBackend::Bedrock { client: bedrock_client, .. } => {
                    // Build request body for Bedrock (no model/stream fields)
                    let body = match build_bedrock_request(&messages, &config) {
                        Ok(b) => b,
                        Err(e) => { yield Err(e); return; }
                    };
                    let body_bytes = match serde_json::to_vec(&body) {
                        Ok(b) => b,
                        Err(e) => { yield Err(ProviderError::Serialization(e.to_string())); return; }
                    };

                    let mut event_stream = match bedrock_client
                        .invoke_model_with_response_stream()
                        .model_id(&config.model)
                        .content_type("application/json")
                        .accept("application/json")
                        .body(aws_sdk_bedrockruntime::primitives::Blob::new(body_bytes))
                        .send()
                        .await
                    {
                        Ok(r) => r.body,
                        Err(e) => { yield Err(ProviderError::Api { status: 0, message: e.to_string() }); return; }
                    };

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
                                    for event in parse_sse_events(&serde_json::to_string(&parsed).unwrap_or_default()) {
                                        yield event;
                                    }
                                }
                            }
                            Ok(None) => break,
                            Ok(_) => continue,
                            Err(e) => {
                                yield Err(ProviderError::Api { status: 0, message: e.to_string() });
                                break;
                            }
                        }
                    }
                }

                AnthropicBackend::Vertex { project_id, location, access_token } => {
                    let body = match build_vertex_request(&messages, &config, true) {
                        Ok(b) => b,
                        Err(e) => { yield Err(e); return; }
                    };
                    let url = AnthropicProvider::vertex_url(project_id, location, &config.model, true);
                    let resp = match client
                        .post(url)
                        .bearer_auth(access_token)
                        .header("content-type", "application/json")
                        .json(&body)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => { yield Err(e.into()); return; }
                    };
                    let resp = match check_response(resp).await {
                        Ok(r) => r,
                        Err(e) => { yield Err(e); return; }
                    };

                    let mut data_stream = Box::pin(sse_data_stream(resp));
                    use futures::StreamExt;

                    while let Some(result) = data_stream.next().await {
                        let data = match result {
                            Ok(d) => d,
                            Err(e) => { yield Err(e); return; }
                        };
                        for event in parse_sse_events(&data) {
                            yield event;
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
        let mut betas = self.betas.clone();
        for b in compute_auto_betas(&config, &messages) {
            if !betas.contains(&b) {
                betas.push(b);
            }
        }
        match &self.backend {
            AnthropicBackend::Direct { api_key, base_url } => {
                let body = build_messages_request(&messages, &config, false)?;
                let json = AnthropicProvider::direct_complete(
                    &self.client,
                    api_key,
                    base_url,
                    &betas,
                    body,
                )
                .await?;
                parse_response(&json)
            }
            AnthropicBackend::Bedrock { client, .. } => {
                let body = build_bedrock_request(&messages, &config)?;
                let body_bytes = serde_json::to_vec(&body)
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                let resp = client
                    .invoke_model()
                    .model_id(&config.model)
                    .content_type("application/json")
                    .accept("application/json")
                    .body(aws_sdk_bedrockruntime::primitives::Blob::new(body_bytes))
                    .send()
                    .await
                    .map_err(|e| ProviderError::Api {
                        status: 0,
                        message: e.to_string(),
                    })?;
                let json: Value = serde_json::from_slice(resp.body.as_ref())
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                parse_response(&json)
            }
            AnthropicBackend::Vertex {
                project_id,
                location,
                access_token,
            } => {
                let body = build_vertex_request(&messages, &config, false)?;
                let json = AnthropicProvider::vertex_complete(
                    &self.client,
                    project_id,
                    location,
                    access_token,
                    &config.model,
                    body,
                )
                .await?;
                parse_response(&json)
            }
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        match &self.backend {
            AnthropicBackend::Direct { api_key, base_url } => {
                list_models_direct(&self.client, api_key, base_url, &self.betas).await
            }
            AnthropicBackend::Bedrock { .. } => Err(ProviderError::Unsupported(
                "list_models not available for Bedrock backend; use BedrockProvider::list_models()"
                    .into(),
            )),
            AnthropicBackend::Vertex { .. } => Err(ProviderError::Unsupported(
                "list_models not available for Vertex backend".into(),
            )),
        }
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        match &self.backend {
            AnthropicBackend::Direct { api_key, base_url } => {
                count_tokens_direct(
                    &self.client,
                    api_key,
                    base_url,
                    &self.betas,
                    messages,
                    config,
                )
                .await
            }
            _ => Err(ProviderError::Unsupported(
                "count_tokens is only available for the Direct Anthropic backend".into(),
            )),
        }
    }

    async fn embed(
        &self,
        _request: EmbeddingRequest,
        _model: &str,
    ) -> Result<EmbeddingResponse, ProviderError> {
        Err(ProviderError::Unsupported(
            "Anthropic does not offer an embeddings API".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/// Build a request body for the direct Anthropic Messages API.
fn build_messages_request(
    messages: &[Message],
    config: &ProviderConfig,
    stream: bool,
) -> Result<Value, ProviderError> {
    let mut req = json!({
        "model": config.model,
        "max_tokens": config.max_tokens.unwrap_or(1024),
        "stream": stream,
    });
    apply_common_fields(&mut req, messages, config)?;
    Ok(req)
}

/// Build a request body for Bedrock (no `model` or `stream` fields).
fn build_bedrock_request(
    messages: &[Message],
    config: &ProviderConfig,
) -> Result<Value, ProviderError> {
    let mut req = json!({
        "anthropic_version": BEDROCK_ANTHROPIC_VERSION,
        "max_tokens": config.max_tokens.unwrap_or(1024),
    });
    apply_common_fields(&mut req, messages, config)?;
    Ok(req)
}

/// Build a request body for Vertex AI (no `model` field).
fn build_vertex_request(
    messages: &[Message],
    config: &ProviderConfig,
    stream: bool,
) -> Result<Value, ProviderError> {
    let mut req = json!({
        "anthropic_version": VERTEX_ANTHROPIC_VERSION,
        "max_tokens": config.max_tokens.unwrap_or(1024),
        "stream": stream,
    });
    apply_common_fields(&mut req, messages, config)?;
    Ok(req)
}

/// Returns extra beta headers automatically required by the given config.
/// These are merged with any user-specified betas before sending the request.
pub(crate) fn compute_auto_betas(config: &ProviderConfig, messages: &[Message]) -> Vec<String> {
    let mut betas: Vec<String> = Vec::new();

    // Web search requires its own beta
    if config.web_search.is_some() {
        betas.push("web-search-2025-03-05".to_string());
    }

    // Check if any message or the system prompt uses cache control
    let has_cache = messages.iter().any(|m| m.cache_control.is_some());
    // Note: basic ephemeral (5-min) cache is now GA (no beta needed).
    // Extended TTL (1h) would need extended-cache-ttl-2025-04-30, but we only
    // expose CacheControl::Ephemeral (5-min) for now.
    let _ = has_cache; // reserved for future use

    betas
}

/// Apply fields common to all backends: system, messages, temperature, tools, thinking…
fn apply_common_fields(
    req: &mut Value,
    messages: &[Message],
    config: &ProviderConfig,
) -> Result<(), ProviderError> {
    // System: build as array of content blocks to support cache control on system prompt
    {
        let mut system_blocks: Vec<Value> = Vec::new();
        if let Some(s) = config.system.as_deref() {
            system_blocks.push(json!({"type": "text", "text": s}));
        }
        for msg in messages {
            if msg.role == Role::System {
                for block in &msg.content {
                    if let ContentBlock::Text(t) = block {
                        system_blocks.push(json!({"type": "text", "text": t}));
                    }
                }
            }
        }
        if !system_blocks.is_empty() {
            // If no blocks have cache control, merge all text into a single string
            let any_cache_control = system_blocks
                .iter()
                .any(|b| b.get("cache_control").is_some());
            if any_cache_control {
                req["system"] = json!(system_blocks);
            } else {
                let combined: String = system_blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                req["system"] = json!(combined);
            }
        }
    }

    req["messages"] = format_messages(messages)?;

    if let Some(temp) = config.temperature {
        req["temperature"] = json!(temp);
    }
    if let Some(top_p) = config.top_p {
        req["top_p"] = json!(top_p);
    }
    if let Some(top_k) = config.top_k {
        req["top_k"] = json!(top_k);
    }
    if !config.stop_sequences.is_empty() {
        req["stop_sequences"] = json!(config.stop_sequences);
    }

    // Reasoning effort (Opus 4.6, Sonnet 4.6, Opus 4.5)
    if let Some(effort) = &config.reasoning_effort {
        req["output_config"] = json!({"effort": effort.as_str()});
    }

    // Extended thinking (manual budget — older models / Sonnet 4.6 interleaved mode)
    if let Some(budget) = config.thinking_budget {
        req["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
        req["temperature"] = json!(1.0);
    }

    // Build tools array
    let mut tools_arr: Vec<Value> = Vec::new();

    // Built-in web search tool
    if let Some(ws) = &config.web_search {
        let mut ws_tool = json!({
            "type": "web_search_20250305",
            "name": "web_search",
        });
        if let Some(max) = ws.max_uses {
            ws_tool["max_uses"] = json!(max);
        }
        if let Some(allowed) = &ws.allowed_domains {
            ws_tool["allowed_domains"] = json!(allowed);
        }
        if let Some(blocked) = &ws.blocked_domains {
            ws_tool["blocked_domains"] = json!(blocked);
        }
        tools_arr.push(ws_tool);
    }

    // Response format (structured output via tool trick)
    if let Some(ResponseFormat::JsonSchema { name, schema, .. }) = &config.response_format {
        tools_arr.push(json!({
            "name": name,
            "description": "Return structured output conforming to the schema",
            "input_schema": schema,
        }));
        req["tool_choice"] = json!({"type": "tool", "name": name});
    } else if let Some(schema) = config.extra.get("output_schema") {
        // Legacy extra["output_schema"] support
        tools_arr.push(json!({
            "name": "structured_output",
            "description": "Return structured output conforming to the schema",
            "input_schema": schema,
        }));
        req["tool_choice"] = json!({"type": "tool", "name": "structured_output"});
    } else if !config.tools.is_empty() {
        for t in &config.tools {
            tools_arr.push(json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
            }));
        }
        if let Some(tc) = &config.tool_choice {
            req["tool_choice"] = match tc {
                ToolChoice::Auto => json!({"type": "auto"}),
                ToolChoice::Any => json!({"type": "any"}),
                ToolChoice::None => json!({"type": "none"}),
                ToolChoice::Tool { name } => json!({"type": "tool", "name": name}),
            };
        }
    }

    if !tools_arr.is_empty() {
        req["tools"] = json!(tools_arr);
    }

    if let Some(user) = &config.user {
        req["metadata"] = json!({"user_id": user});
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Message formatting
// ---------------------------------------------------------------------------

fn format_messages(messages: &[Message]) -> Result<Value, ProviderError> {
    let mut result = Vec::new();
    for msg in messages {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => continue,
        };
        let mut content = format_content_blocks(&msg.content)?;
        // Apply per-message cache control to the last content block
        if msg.cache_control.is_some()
            && let Some(arr) = content.as_array_mut()
            && let Some(last) = arr.last_mut()
            && let Some(obj) = last.as_object_mut()
        {
            obj.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
        }
        result.push(json!({"role": role, "content": content}));
    }
    Ok(json!(result))
}

fn format_content_blocks(blocks: &[ContentBlock]) -> Result<Value, ProviderError> {
    // Anthropic rejects empty text blocks with a 400 error — filter them out
    let filtered: Vec<&ContentBlock> = blocks
        .iter()
        .filter(|b| !matches!(b, ContentBlock::Text(t) if t.is_empty()))
        .collect();
    let parts: Result<Vec<Value>, _> = filtered.iter().map(|b| format_content_block(b)).collect();
    Ok(json!(parts?))
}

fn format_content_block(block: &ContentBlock) -> Result<Value, ProviderError> {
    match block {
        ContentBlock::Text(text) => Ok(json!({"type": "text", "text": text})),
        ContentBlock::Image(img) => format_image_block(img),
        ContentBlock::Document(doc) => format_document_block(doc),
        ContentBlock::ToolUse(tu) => Ok(json!({
            "type": "tool_use",
            "id": tu.id,
            "name": tu.name,
            "input": tu.input,
        })),
        ContentBlock::ToolResult(tr) => {
            let content = format_content_blocks(&tr.content)?;
            Ok(json!({
                "type": "tool_result",
                "tool_use_id": tr.tool_use_id,
                "content": content,
                "is_error": tr.is_error,
            }))
        }
        ContentBlock::Thinking(th) => {
            let mut v = json!({
                "type": "thinking",
                "thinking": th.thinking,
            });
            if let Some(sig) = &th.signature {
                v["signature"] = json!(sig);
            }
            Ok(v)
        }
        ContentBlock::Audio(_) => Err(ProviderError::Unsupported(
            "Anthropic does not support audio input".into(),
        )),
        ContentBlock::Video(_) => Err(ProviderError::Unsupported(
            "Anthropic does not support video input".into(),
        )),
    }
}

fn format_image_block(img: &ImageContent) -> Result<Value, ProviderError> {
    match &img.source {
        MediaSource::Base64(b64) => Ok(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": b64.media_type,
                "data": b64.data,
            }
        })),
        MediaSource::Url(url) => Ok(json!({
            "type": "image",
            "source": {"type": "url", "url": url}
        })),
        _ => Err(ProviderError::Unsupported(
            "Anthropic images require base64 or URL source".into(),
        )),
    }
}

fn format_document_block(doc: &DocumentContent) -> Result<Value, ProviderError> {
    match &doc.source {
        MediaSource::Base64(b64) => Ok(json!({
            "type": "document",
            "source": {
                "type": "base64",
                "media_type": b64.media_type,
                "data": b64.data,
            }
        })),
        _ => Err(ProviderError::Unsupported(
            "Anthropic documents require base64 source".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// SSE event parsing (shared by Direct and Vertex backends)
// ---------------------------------------------------------------------------

fn parse_sse_events(data: &str) -> Vec<Result<StreamEvent, ProviderError>> {
    let parsed: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let event_type = parsed["type"].as_str().unwrap_or("");
    let mut events = Vec::new();

    match event_type {
        "message_start" => {
            let role_str = parsed["message"]["role"].as_str().unwrap_or("assistant");
            let role = if role_str == "user" {
                Role::User
            } else {
                Role::Assistant
            };
            events.push(Ok(StreamEvent::MessageStart { role }));
        }
        "content_block_start" => {
            let index = parsed["index"].as_u64().unwrap_or(0) as usize;
            let block_type = parsed["content_block"]["type"].as_str().unwrap_or("text");
            let block = match block_type {
                "tool_use" => {
                    let id = parsed["content_block"]["id"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let name = parsed["content_block"]["name"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    ContentBlockStart::ToolUse { id, name }
                }
                "thinking" => ContentBlockStart::Thinking,
                _ => ContentBlockStart::Text,
            };
            events.push(Ok(StreamEvent::ContentBlockStart { index, block }));
        }
        "content_block_delta" => {
            let index = parsed["index"].as_u64().unwrap_or(0) as usize;
            let delta = &parsed["delta"];
            let delta_type = delta["type"].as_str().unwrap_or("");
            let cd = match delta_type {
                "text_delta" => {
                    let text = delta["text"].as_str().unwrap_or("").to_string();
                    ContentDelta::Text { text }
                }
                "input_json_delta" => {
                    let partial_json = delta["partial_json"].as_str().unwrap_or("").to_string();
                    ContentDelta::ToolInput { partial_json }
                }
                "thinking_delta" => {
                    let thinking = delta["thinking"].as_str().unwrap_or("").to_string();
                    ContentDelta::Thinking { thinking }
                }
                "signature_delta" => {
                    let signature = delta["signature"].as_str().unwrap_or("").to_string();
                    ContentDelta::Signature { signature }
                }
                _ => return events,
            };
            events.push(Ok(StreamEvent::ContentBlockDelta { index, delta: cd }));
        }
        "content_block_stop" => {
            let index = parsed["index"].as_u64().unwrap_or(0) as usize;
            events.push(Ok(StreamEvent::ContentBlockStop { index }));
        }
        "message_delta" => {
            let stop_reason_str = parsed["delta"]["stop_reason"]
                .as_str()
                .unwrap_or("end_turn");
            let stop_reason = parse_stop_reason(stop_reason_str);
            let output_tokens = parsed["usage"]["output_tokens"].as_u64().unwrap_or(0);
            events.push(Ok(StreamEvent::MessageStop { stop_reason }));
            events.push(Ok(StreamEvent::Metadata {
                usage: Usage {
                    output_tokens,
                    ..Default::default()
                },
                model: None,
                id: None,
            }));
        }
        "message_stop" => {}
        "error" => {
            let msg = parsed["error"]["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string();
            events.push(Err(ProviderError::Api {
                status: 0,
                message: msg,
            }));
        }
        _ => {}
    }

    events
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

fn parse_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let content_arr = json["content"].as_array().ok_or_else(|| {
        ProviderError::Serialization("Missing 'content' in Anthropic response".into())
    })?;

    let content: Vec<ContentBlock> = content_arr.iter().filter_map(parse_content_block).collect();

    let stop_reason = parse_stop_reason(json["stop_reason"].as_str().unwrap_or("end_turn"));

    let usage = Usage {
        input_tokens: json["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: json["usage"]["output_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: json["usage"]["cache_read_input_tokens"]
            .as_u64()
            .unwrap_or(0),
        cache_write_tokens: json["usage"]["cache_creation_input_tokens"]
            .as_u64()
            .unwrap_or(0),
        ..Default::default()
    };

    let model = json["model"].as_str().map(|s| s.to_string());

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model,
        id: None,
    })
}

fn parse_content_block(block: &Value) -> Option<ContentBlock> {
    match block["type"].as_str()? {
        "text" => Some(ContentBlock::Text(block["text"].as_str()?.to_string())),
        "tool_use" => Some(ContentBlock::ToolUse(ToolUseBlock {
            id: block["id"].as_str()?.to_string(),
            name: block["name"].as_str()?.to_string(),
            input: block["input"].clone(),
        })),
        "thinking" => Some(ContentBlock::Thinking(ThinkingBlock {
            thinking: block["thinking"].as_str()?.to_string(),
            signature: block["signature"].as_str().map(|s| s.to_string()),
        })),
        _ => None,
    }
}

fn parse_stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "tool_use" => StopReason::ToolUse,
        "stop_sequence" => StopReason::StopSequence(String::new()),
        other => StopReason::Other(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// List models
// ---------------------------------------------------------------------------

async fn list_models_direct(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    betas: &[String],
) -> Result<Vec<ModelInfo>, ProviderError> {
    let mut req = client
        .get(format!("{base_url}/models"))
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION);
    if !betas.is_empty() {
        req = req.header("anthropic-beta", betas.join(","));
    }
    let resp = req.send().await?;
    let resp = check_response(resp).await?;
    let json: Value = resp.json().await?;

    let mut models = Vec::new();
    if let Some(arr) = json["data"].as_array() {
        for item in arr {
            models.push(ModelInfo {
                id: item["id"].as_str().unwrap_or("").to_string(),
                display_name: item["display_name"].as_str().map(|s| s.to_string()),
                description: None,
                created_at: item["created_at"].as_u64(),
            });
        }
    }
    Ok(models)
}

// ---------------------------------------------------------------------------
// Count tokens
// ---------------------------------------------------------------------------

async fn count_tokens_direct(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    betas: &[String],
    messages: Vec<Message>,
    config: ProviderConfig,
) -> Result<TokenCount, ProviderError> {
    let mut body = build_messages_request(&messages, &config, false)?;
    // count_tokens doesn't use stream field
    body.as_object_mut().map(|m| m.remove("stream"));

    // Always include the token-counting beta
    let mut all_betas = betas.to_vec();
    if !all_betas.iter().any(|b| b.contains("token-counting")) {
        all_betas.push("token-counting-2024-11-01".to_string());
    }

    let resp = client
        .post(format!("{base_url}/messages/count_tokens"))
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", all_betas.join(","))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;
    let resp = check_response(resp).await?;
    let json: Value = resp.json().await?;

    Ok(TokenCount {
        input_tokens: json["input_tokens"].as_u64().unwrap_or(0),
    })
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Tool, ToolChoice};
    use serde_json::json;

    #[test]
    fn test_build_request_basic() {
        let config = ProviderConfig::new("claude-opus-4-6").with_max_tokens(512);
        let messages = vec![Message::user("Hello")];
        let req = build_messages_request(&messages, &config, false).unwrap();
        assert_eq!(req["model"], "claude-opus-4-6");
        assert_eq!(req["max_tokens"], 512);
        assert_eq!(req["stream"], false);
        assert_eq!(req["messages"][0]["role"], "user");
    }

    #[test]
    fn test_build_request_with_system() {
        let config = ProviderConfig::new("claude-opus-4-6")
            .with_system("You are a helpful assistant")
            .with_max_tokens(256);
        let messages = vec![Message::user("Hi")];
        let req = build_messages_request(&messages, &config, false).unwrap();
        assert_eq!(req["system"], "You are a helpful assistant");
    }

    #[test]
    fn test_build_request_with_tools() {
        let config = ProviderConfig::new("claude-opus-4-6")
            .with_max_tokens(256)
            .with_tools(vec![Tool::new(
                "get_weather",
                "Get the weather",
                json!({"type": "object", "properties": {"location": {"type": "string"}}, "required": ["location"]}),
            )])
            .with_tool_choice(ToolChoice::Auto);
        let messages = vec![Message::user("What is the weather in NYC?")];
        let req = build_messages_request(&messages, &config, false).unwrap();
        assert_eq!(req["tools"][0]["name"], "get_weather");
        assert_eq!(req["tool_choice"]["type"], "auto");
    }

    #[test]
    fn test_build_request_with_thinking() {
        let config = ProviderConfig::new("claude-sonnet-4-6")
            .with_max_tokens(16000)
            .with_thinking(10000);
        let messages = vec![Message::user("Complex math problem")];
        let req = build_messages_request(&messages, &config, false).unwrap();
        assert_eq!(req["thinking"]["type"], "enabled");
        assert_eq!(req["thinking"]["budget_tokens"], 10000);
        assert_eq!(req["temperature"], 1.0);
    }

    #[test]
    fn test_bedrock_request_no_model_or_stream() {
        let config = ProviderConfig::new("anthropic.claude-sonnet-4-6-v1:0").with_max_tokens(256);
        let messages = vec![Message::user("Hello")];
        let req = build_bedrock_request(&messages, &config).unwrap();
        assert!(req.get("model").is_none());
        assert!(req.get("stream").is_none());
        assert_eq!(req["anthropic_version"], BEDROCK_ANTHROPIC_VERSION);
    }

    #[test]
    fn test_vertex_request_no_model() {
        let config = ProviderConfig::new("claude-sonnet-4-6@20250929").with_max_tokens(256);
        let messages = vec![Message::user("Hello")];
        let req = build_vertex_request(&messages, &config, true).unwrap();
        assert!(req.get("model").is_none());
        assert_eq!(req["anthropic_version"], VERTEX_ANTHROPIC_VERSION);
        assert_eq!(req["stream"], true);
    }

    #[test]
    fn test_parse_stop_reason() {
        assert!(matches!(parse_stop_reason("end_turn"), StopReason::EndTurn));
        assert!(matches!(parse_stop_reason("tool_use"), StopReason::ToolUse));
        assert!(matches!(
            parse_stop_reason("max_tokens"),
            StopReason::MaxTokens
        ));
    }

    #[test]
    fn test_parse_response() {
        let json = json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "model": "claude-opus-4-6",
            "content": [{"type": "text", "text": "Hello, world!"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            }
        });
        let resp = parse_response(&json).unwrap();
        assert_eq!(resp.content.len(), 1);
        assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello, world!"));
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
        assert_eq!(resp.model.as_deref(), Some("claude-opus-4-6"));
    }

    #[test]
    fn test_format_image_base64() {
        let img = crate::types::ImageContent {
            source: MediaSource::base64("image/png", "iVBORw0KGgo="),
            format: Some(crate::types::ImageFormat::Png),
        };
        let v = format_image_block(&img).unwrap();
        assert_eq!(v["type"], "image");
        assert_eq!(v["source"]["type"], "base64");
        assert_eq!(v["source"]["media_type"], "image/png");
    }

    #[test]
    fn test_count_tokens_includes_beta() {
        // Ensure the beta header logic adds token-counting-2024-11-01 automatically
        let betas: Vec<String> = vec![];
        let mut all_betas = betas.clone();
        if !all_betas.iter().any(|b| b.contains("token-counting")) {
            all_betas.push("token-counting-2024-11-01".to_string());
        }
        assert!(all_betas.contains(&"token-counting-2024-11-01".to_string()));
    }

    #[test]
    fn test_system_messages_merged() {
        let config = ProviderConfig::new("claude-opus-4-6")
            .with_system("System from config")
            .with_max_tokens(256);
        let messages = vec![
            Message::with_content(Role::System, vec![ContentBlock::text("Extra system")]),
            Message::user("Hello"),
        ];
        let req = build_messages_request(&messages, &config, false).unwrap();
        let system = req["system"].as_str().unwrap();
        assert!(system.contains("System from config"));
        assert!(system.contains("Extra system"));
        // System messages should not appear in messages array
        let msgs = req["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[tokio::test]
    async fn test_integration_complete() {
        let api_key = match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: ANTHROPIC_API_KEY not set");
                return;
            }
        };
        let provider = AnthropicProvider::new(api_key);
        let config = ProviderConfig::new("claude-haiku-4-5-20251001").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hello' in one word.")];
        let resp = provider.complete(messages, config).await.unwrap();
        assert!(!resp.content.is_empty());
        assert!(resp.usage.output_tokens > 0);
    }

    #[tokio::test]
    async fn test_integration_stream() {
        let api_key = match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: ANTHROPIC_API_KEY not set");
                return;
            }
        };
        use crate::provider::collect_stream;
        let provider = AnthropicProvider::new(api_key);
        let config = ProviderConfig::new("claude-haiku-4-5-20251001").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hi'.")];
        let stream = provider.stream(messages, config);
        let resp = collect_stream(stream).await.unwrap();
        assert!(!resp.content.is_empty());
    }

    #[tokio::test]
    async fn test_integration_list_models() {
        let api_key = match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: ANTHROPIC_API_KEY not set");
                return;
            }
        };
        let provider = AnthropicProvider::new(api_key);
        let models = provider.list_models().await.unwrap();
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.id.contains("claude")));
    }

    #[tokio::test]
    async fn test_integration_count_tokens() {
        let api_key = match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: ANTHROPIC_API_KEY not set");
                return;
            }
        };
        let provider = AnthropicProvider::new(api_key);
        let config = ProviderConfig::new("claude-haiku-4-5-20251001").with_max_tokens(64);
        let messages = vec![Message::user("Hello, how are you?")];
        let count = provider.count_tokens(messages, config).await.unwrap();
        assert!(count.input_tokens > 0);
    }
}
