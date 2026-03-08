use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{ChatProvider, EmbeddingProvider, ImageProvider, AudioProvider, ModerationProvider, Provider, ProviderStream, StatefulProvider},
    providers::{
        openai_common::OpenAIInnerClient,
        sse::{check_response, sse_data_stream},
    },
    types::{
        ContentBlock, ContentBlockStart, ContentDelta, EmbeddingRequest, EmbeddingResponse,
        ImageEditRequest, ImageGenerationRequest, ImageGenerationResponse, MediaSource, Message,
        ModelInfo, ModerationRequest, ModerationResponse, ProviderConfig, ResponseFormat, Role,
        SpeechRequest, SpeechResponse, StopReason, StreamEvent, TokenCount, ToolUseBlock,
        TranscriptionRequest, TranscriptionResponse, Usage,
    },
};

const OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const OPENAI_RESPONSES_API_BASE: &str = "https://api.openai.com/v1";

/// OpenAI Responses API provider.
///
/// Supports server-side multi-turn (`previous_response_id`), built-in tools
/// (web search, file search, computer use), structured outputs, and typed SSE events.
pub struct OpenAIResponsesProvider {
    shared: OpenAIInnerClient,
    base_url: String,
    /// Optional ID of a previous response for server-side multi-turn
    pub previous_response_id: Option<String>,
}

impl OpenAIResponsesProvider {
    /// Create a provider from the `OPENAI_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ProviderError> {
        Ok(Self::new(crate::env::require(crate::env::keys::OPENAI_API_KEY)?))
    }

    pub fn new(api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        let client = Arc::new(reqwest::Client::new());
        Self {
            shared: OpenAIInnerClient::new(api_key, client, OPENAI_RESPONSES_API_BASE),
            base_url: OPENAI_RESPONSES_URL.to_string(),
            previous_response_id: None,
        }
    }

    /// Replace the HTTP client. Useful for custom TLS, proxies, or testing.
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.shared.client = Arc::new(client);
        self
    }

    /// Override the responses endpoint URL.  If the URL contains `/responses`
    /// the api_base is derived by stripping that suffix; otherwise the URL
    /// itself is used as the api_base.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let url = base_url.into();
        if let Some(pos) = url.find("/responses") {
            self.shared.api_base = url[..pos].to_string();
        } else {
            self.shared.api_base = url.clone();
        }
        self.base_url = url;
        self
    }

    /// Set the API base URL for use with any OpenAI-compatible proxy.
    /// Derives all endpoints:
    /// - Responses:  `{base}/responses`
    /// - Models:     `{base}/models`
    /// - Embeddings: `{base}/embeddings`
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        let base = api_base.into();
        self.base_url = format!("{}/responses", base);
        self.shared.api_base = base;
        self
    }

    pub fn with_previous_response_id(mut self, id: impl Into<String>) -> Self {
        self.previous_response_id = Some(id.into());
        self
    }

    /// [Amazon Bedrock OpenAI-compatible API](https://docs.aws.amazon.com/bedrock/latest/userguide/bedrock-mantle.html) —
    /// OpenAI-compatible Responses API endpoints backed by Amazon Bedrock.
    ///
    /// `region`: AWS region, e.g. `"us-east-1"`.
    /// `api_key`: a Bedrock API key (bearer token).
    ///
    /// Use `openai.` prefixed model names, e.g. `"openai.gpt-oss-120b"`.
    pub fn for_bedrock_openai(region: impl Into<String>, api_key: impl Into<String>) -> Self {
        let region = region.into();
        Self::new(api_key)
            .with_api_base(format!("https://bedrock-mantle.{region}.api.aws/v1"))
    }

    /// Create an Amazon Bedrock OpenAI-compatible API provider from environment variables.
    ///
    /// Reads `BEDROCK_API_KEY` (or `AWS_BEARER_TOKEN_BEDROCK`) for the API key
    /// and `BEDROCK_REGION` / `AWS_REGION` / `AWS_DEFAULT_REGION` for the region
    /// (defaulting to `"us-east-1"`).
    pub fn for_bedrock_openai_from_env() -> Result<Self, ProviderError> {
        let api_key = crate::env::require(crate::env::keys::BEDROCK_API_KEY)
            .or_else(|_| crate::env::require("AWS_BEARER_TOKEN_BEDROCK"))?;
        let region = crate::env::optional("BEDROCK_REGION")
            .or_else(|| crate::env::optional("AWS_REGION"))
            .or_else(|| crate::env::optional("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|| "us-east-1".to_string());
        Ok(Self::for_bedrock_openai(region, api_key))
    }

}

#[async_trait]
impl Provider for OpenAIResponsesProvider {
    fn provider_name(&self) -> &'static str {
        "openai"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.shared.list_models().await
    }
}

#[async_trait]
impl ChatProvider for OpenAIResponsesProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let api_key = self.shared.api_key.clone();
        let client = Arc::clone(&self.shared.client);
        let base_url = self.base_url.clone();
        let previous_response_id = self.previous_response_id.clone();

        Box::pin(stream! {
            let body = match build_request(&messages, &config, true, previous_response_id.as_deref()) {
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

            let mut item_to_block: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            let mut next_block: usize = 0;

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

                let event_type = parsed["type"].as_str().unwrap_or("");

                match event_type {
                    "response.output_item.added" => {
                        let item = &parsed["item"];
                        let item_id = item["id"].as_str().unwrap_or("").to_string();
                        let item_type = item["type"].as_str().unwrap_or("");

                        let block = match item_type {
                            "function_call" => {
                                let call_id = item["call_id"].as_str()
                                    .or_else(|| item["id"].as_str())
                                    .unwrap_or("").to_string();
                                let name = item["name"].as_str().unwrap_or("").to_string();
                                Some(ContentBlockStart::ToolUse { id: call_id, name })
                            }
                            "message" => Some(ContentBlockStart::Text),
                            _ => None,
                        };

                        if let Some(b) = block {
                            let idx = next_block;
                            item_to_block.insert(item_id, idx);
                            next_block += 1;
                            yield Ok(StreamEvent::ContentBlockStart { index: idx, block: b });
                        }
                    }
                    "response.content_part.added" => {
                        // Content part added to a message item
                        let output_idx = parsed["output_index"].as_u64().unwrap_or(0) as usize;
                        let part_type = parsed["part"]["type"].as_str().unwrap_or("");
                        if part_type == "output_text" {
                            // Ensure block exists for this output index
                            if !item_to_block.values().any(|&v| v == output_idx) {
                                item_to_block.insert(format!("text_{}", output_idx), output_idx);
                                yield Ok(StreamEvent::ContentBlockStart {
                                    index: output_idx,
                                    block: ContentBlockStart::Text,
                                });
                            }
                        }
                    }
                    "response.output_text.delta" => {
                        let output_idx = parsed["output_index"].as_u64().unwrap_or(0) as usize;
                        let delta = parsed["delta"].as_str().unwrap_or("");
                        if !delta.is_empty() {
                            yield Ok(StreamEvent::ContentBlockDelta {
                                index: output_idx,
                                delta: ContentDelta::Text { text: delta.to_string() },
                            });
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        let output_idx = parsed["output_index"].as_u64().unwrap_or(0) as usize;
                        let delta = parsed["delta"].as_str().unwrap_or("");
                        if !delta.is_empty() {
                            yield Ok(StreamEvent::ContentBlockDelta {
                                index: output_idx,
                                delta: ContentDelta::ToolInput { partial_json: delta.to_string() },
                            });
                        }
                    }
                    "response.output_item.done" => {
                        let item = &parsed["item"];
                        let item_id = item["id"].as_str().unwrap_or("").to_string();
                        if let Some(&idx) = item_to_block.get(&item_id) {
                            yield Ok(StreamEvent::ContentBlockStop { index: idx });
                        }
                    }
                    "response.refusal.delta" => {
                        // Model refused the request — stream refusal text as a text delta
                        let output_idx = parsed["output_index"].as_u64().unwrap_or(0) as usize;
                        let delta = parsed["delta"].as_str().unwrap_or("");
                        if !delta.is_empty() {
                            yield Ok(StreamEvent::ContentBlockDelta {
                                index: output_idx,
                                delta: ContentDelta::Text { text: delta.to_string() },
                            });
                        }
                    }
                    "response.refusal.done" | "response.output_text.done" => {
                        // ContentBlockStop is handled by response.output_item.done to avoid
                        // double-stopping text blocks.
                    }
                    "response.completed" => {
                        let response = &parsed["response"];
                        let status = response["status"].as_str().unwrap_or("completed");
                        let stop_reason = if status == "incomplete" {
                            StopReason::MaxTokens
                        } else {
                            StopReason::EndTurn
                        };
                        let usage = parse_usage(&response["usage"]);
                        let model = response["model"].as_str().map(|s| s.to_string());
                        let id = response["id"].as_str().map(|s| s.to_string());
                        yield Ok(StreamEvent::MessageStop { stop_reason });
                        yield Ok(StreamEvent::Metadata { usage, model, id });
                        return;
                    }
                    "response.failed" => {
                        let msg = parsed["response"]["error"]["message"]
                            .as_str().unwrap_or("Response failed").to_string();
                        yield Err(ProviderError::Api { status: 0, message: msg });
                        return;
                    }
                    _ => {}
                }
            }
        })
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<crate::types::Response, ProviderError> {
        let body = build_request(
            &messages,
            &config,
            false,
            self.previous_response_id.as_deref(),
        )?;

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
        parse_response(&json)
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        let url = format!("{}/responses/input_tokens", self.shared.api_base);
        let body = build_request(&messages, &config, false, self.previous_response_id.as_deref())?;

        let mut req_builder = self.shared.client.post(&url).bearer_auth(&self.shared.api_key).json(&body);
        if let Some(ms) = config.timeout_ms {
            req_builder = req_builder.timeout(std::time::Duration::from_millis(ms));
        }
        let resp = req_builder.send().await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let input_tokens = json["input_tokens"].as_u64().unwrap_or(0);
        Ok(TokenCount { input_tokens })
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIResponsesProvider {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        self.shared.embed(request).await
    }
}

#[async_trait]
impl ImageProvider for OpenAIResponsesProvider {
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
impl AudioProvider for OpenAIResponsesProvider {
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
impl ModerationProvider for OpenAIResponsesProvider {
    async fn moderate(
        &self,
        request: ModerationRequest,
    ) -> Result<ModerationResponse, ProviderError> {
        self.shared.moderate(request).await
    }
}

#[async_trait]
impl StatefulProvider for OpenAIResponsesProvider {
    async fn retrieve_response(&self, id: &str) -> Result<crate::types::Response, ProviderError> {
        let url = format!("{}/responses/{}", self.shared.api_base, id);
        let resp = self
            .shared
            .client
            .get(&url)
            .bearer_auth(&self.shared.api_key)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;
        parse_response(&json)
    }

    async fn cancel_response(&self, id: &str) -> Result<crate::types::Response, ProviderError> {
        let url = format!("{}/responses/{}/cancel", self.shared.api_base, id);
        let resp = self
            .shared
            .client
            .post(&url)
            .bearer_auth(&self.shared.api_key)
            .header("content-length", "0")
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;
        parse_response(&json)
    }
}

// ---------------------------------------------------------------------------
// Request building (free function)
// ---------------------------------------------------------------------------

fn build_request(
    messages: &[Message],
    config: &ProviderConfig,
    stream: bool,
    previous_response_id: Option<&str>,
) -> Result<Value, ProviderError> {
    let input = format_input(messages)?;

    let mut req = json!({
        "model": config.model,
        "input": input,
        "stream": stream,
    });

    if let Some(sys) = &config.system {
        req["instructions"] = json!(sys);
    }
    if let Some(max_tokens) = config.max_tokens {
        req["max_output_tokens"] = json!(max_tokens);
    }
    if let Some(temp) = config.temperature {
        req["temperature"] = json!(temp);
    }
    if let Some(top_p) = config.top_p {
        req["top_p"] = json!(top_p);
    }

    let mut tools: Vec<Value> = config
        .tools
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
                "name": t.name,
                "description": t.description,
                "parameters": schema,
                "strict": t.strict,
            })
        })
        .collect();

    // Typed built-in tools (file_search, code_interpreter, mcp, etc.)
    tools.extend(config.built_in_tools.iter().map(|t| t.0.clone()));

    // Legacy: extra["builtin_tools"] raw JSON array (kept for backwards compatibility)
    if let Some(bt) = config.extra.get("builtin_tools")
        && let Some(arr) = bt.as_array()
    {
        tools.extend(arr.iter().cloned());
    }

    if !tools.is_empty() {
        req["tools"] = json!(tools);
    }

    if let Some(tc) = &config.tool_choice {
        req["tool_choice"] = format_tool_choice(tc);
    }

    if let Some(effort) = &config.reasoning_effort {
        req["reasoning"] = json!({"effort": effort.as_str()});
    }
    if let Some(seed) = config.seed {
        req["seed"] = json!(seed);
    }
    if let Some(tier) = &config.service_tier {
        req["service_tier"] = json!(tier.as_str());
    }

    // Web search
    if let Some(ws) = &config.web_search {
        let tools = req["tools"].as_array_mut().cloned().unwrap_or_default();
        let mut all_tools = tools;
        let mut ws_tool = json!({"type": "web_search_preview"});
        if let Some(allowed) = &ws.allowed_domains {
            ws_tool["allowed_domains"] = json!(allowed);
        }
        if let Some(blocked) = &ws.blocked_domains {
            ws_tool["blocked_domains"] = json!(blocked);
        }
        if let Some(ctx_size) = &ws.search_context_size {
            ws_tool["search_context_size"] = json!(ctx_size);
        }
        if let Some(loc) = &ws.user_location {
            ws_tool["user_location"] =
                serde_json::to_value(loc).unwrap_or(serde_json::Value::Null);
        }
        all_tools.push(ws_tool);
        req["tools"] = json!(all_tools);
    }

    // Response / text format
    if let Some(fmt) = &config.response_format {
        req["text"] = json!({"format": format_text_format(fmt)});
    } else if let Some(schema_val) = config.extra.get("output_schema") {
        // Legacy extra["output_schema"]: accepts raw schema or { name, schema } envelope.
        let name = schema_val["name"].as_str().unwrap_or("structured_output");
        let inner_schema = schema_val.get("schema").unwrap_or(schema_val);
        req["text"] = json!({
            "format": {
                "type": "json_schema",
                "json_schema": {
                    "name": name,
                    "schema": inner_schema,
                    "strict": true,
                }
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
    // `n` is not supported by the Responses API (removed from spec)

    if let Some(background) = config.background {
        req["background"] = json!(background);
    }
    if let Some(cm) = &config.context_management {
        req["context_management"] =
            serde_json::to_value(cm).unwrap_or_else(|_| json!({}));
    }
    if let Some(truncation) = &config.truncation {
        req["truncation"] = json!(truncation);
    }
    if let Some(retention) = &config.prompt_cache_retention {
        req["prompt_cache_retention"] = json!(retention);
    }
    if let Some(cache_key) = &config.prompt_cache_key {
        req["prompt_cache_key"] = json!(cache_key);
    }

    for (k, v) in &config.extra {
        if k != "output_schema" && k != "builtin_tools" {
            req[k] = v.clone();
        }
    }

    if let Some(prev_id) = previous_response_id {
        req["previous_response_id"] = json!(prev_id);
    }

    Ok(req)
}

// ---------------------------------------------------------------------------
// Message formatting
// ---------------------------------------------------------------------------

fn format_input(messages: &[Message]) -> Result<Value, ProviderError> {
    let mut items: Vec<Value> = Vec::new();

    for msg in messages {
        let role = match &msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Tool => "tool",
            Role::Assistant => "assistant",
            Role::Other(s) => s.as_str(),
        };

        // Tool results → function_call_output items
        let has_tool_results = msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult(_)));
        if has_tool_results {
            for block in &msg.content {
                if let ContentBlock::ToolResult(tr) = block {
                    let content: String = tr
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
                    items.push(json!({
                        "type": "function_call_output",
                        "call_id": tr.tool_use_id,
                        "output": content,
                    }));
                }
            }
            continue;
        }

        let content: Vec<Value> = msg
            .content
            .iter()
            .filter_map(|b| format_content_part(b).ok())
            .filter(|v| !v.is_null())
            .collect();

        let content_val = if content.len() == 1 {
            if let Some(text) = content[0]["text"].as_str() {
                json!(text)
            } else {
                json!(content)
            }
        } else {
            json!(content)
        };

        items.push(json!({"role": role, "content": content_val, "type": "message"}));
    }

    Ok(json!(items))
}

fn format_content_part(block: &ContentBlock) -> Result<Value, ProviderError> {
    match block {
        ContentBlock::Text(t) => Ok(json!({"type": "input_text", "text": t.text})),
        ContentBlock::Image(img) => {
            use crate::types::ImageDetail;
            let detail = match img.detail.as_ref().unwrap_or(&ImageDetail::Auto) {
                ImageDetail::Auto => "auto",
                ImageDetail::Low => "low",
                ImageDetail::High => "high",
            };
            match &img.source {
            MediaSource::Url(url) => Ok(json!({
                "type": "input_image",
                "image_url": url,
                "detail": detail,
            })),
            MediaSource::Base64(b64) => Ok(json!({
                "type": "input_image",
                "image_url": format!("data:{};base64,{}", b64.media_type, b64.data),
                "detail": detail,
            })),
            _ => Err(ProviderError::Unsupported(
                "Responses API images require URL or base64 source".into(),
            )),
            }
        },
        ContentBlock::Document(doc) => match &doc.source {
            MediaSource::Base64(b64) => Ok(json!({
                "type": "input_file",
                "filename": doc.name.as_deref().unwrap_or("document"),
                "file_data": format!("data:{};base64,{}", b64.media_type, b64.data),
            })),
            _ => Err(ProviderError::Unsupported(
                "Responses API documents require base64 source".into(),
            )),
        },
        _ => Ok(json!(null)),
    }
}

fn format_text_format(fmt: &ResponseFormat) -> Value {
    match fmt {
        ResponseFormat::Text => json!({"type": "text"}),
        ResponseFormat::Json => json!({"type": "json_object"}),
        ResponseFormat::JsonSchema {
            name,
            schema,
            strict,
        } => {
            let mut s = schema.clone();
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

fn format_tool_choice(tc: &crate::types::ToolChoice) -> Value {
    match tc {
        crate::types::ToolChoice::Auto => json!("auto"),
        crate::types::ToolChoice::Any => json!("required"),
        crate::types::ToolChoice::None => json!("none"),
        crate::types::ToolChoice::Tool { name } => json!({"type": "function", "name": name}),
        crate::types::ToolChoice::AllowedTools { tools } => json!({
            "type": "allowed_tools",
            "mode": "auto",
            "tools": tools.iter().map(|n| json!({"type": "function", "name": n})).collect::<Vec<_>>(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

fn parse_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let output = json["output"].as_array().ok_or_else(|| {
        ProviderError::Serialization("Missing 'output' in Responses API response".into())
    })?;

    let mut content: Vec<ContentBlock> = Vec::new();

    for item in output {
        match item["type"].as_str().unwrap_or("") {
            "message" => {
                if let Some(parts) = item["content"].as_array() {
                    for part in parts {
                        if part["type"].as_str() == Some("output_text")
                            && let Some(text) = part["text"].as_str()
                        {
                            content.push(ContentBlock::text(text));
                        }
                    }
                }
            }
            "function_call" => {
                let call_id = item["call_id"]
                    .as_str()
                    .or_else(|| item["id"].as_str())
                    .unwrap_or("")
                    .to_string();
                let name = item["name"].as_str().unwrap_or("").to_string();
                let args_str = item["arguments"].as_str().unwrap_or("{}");
                let input = serde_json::from_str(args_str).unwrap_or(Value::Null);
                content.push(ContentBlock::ToolUse(ToolUseBlock {
                    id: call_id,
                    name,
                    input,
                }));
            }
            _ => {}
        }
    }

    let status = json["status"].as_str().unwrap_or("completed");
    let has_tool_use = content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    let stop_reason = if has_tool_use {
        StopReason::ToolUse
    } else {
        match status {
            "completed" => StopReason::EndTurn,
            "incomplete" => StopReason::MaxTokens,
            "failed" => StopReason::Other("failed".to_string()),
            other => StopReason::Other(other.to_string()),
        }
    };

    let usage = parse_usage(&json["usage"]);
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

fn parse_usage(usage: &Value) -> Usage {
    Usage {
        input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: usage["input_tokens_details"]["cached_tokens"]
            .as_u64()
            .unwrap_or(0),
        reasoning_tokens: usage["output_tokens_details"]["reasoning_tokens"]
            .as_u64()
            .unwrap_or(0),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BuiltinTool, McpToolConfig, WebSearchConfig, WebSearchUserLocation};
    use serde_json::json;

    #[test]
    fn test_builtin_tool_file_search() {
        let config = ProviderConfig::new("gpt-4.1")
            .with_built_in_tool(BuiltinTool::file_search());
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"][0]["type"], "file_search");
    }

    #[test]
    fn test_builtin_tool_file_search_with_ids() {
        let config = ProviderConfig::new("gpt-4.1")
            .with_built_in_tool(BuiltinTool::file_search_with_ids(["vs_abc", "vs_def"]));
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"][0]["type"], "file_search");
        assert_eq!(req["tools"][0]["vector_store_ids"][0], "vs_abc");
    }

    #[test]
    fn test_builtin_tool_code_interpreter() {
        let config = ProviderConfig::new("gpt-4.1")
            .with_built_in_tool(BuiltinTool::code_interpreter());
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"][0]["type"], "code_interpreter");
        assert_eq!(req["tools"][0]["container"]["type"], "auto");
    }

    #[test]
    fn test_builtin_tool_code_interpreter_with_files() {
        let config = ProviderConfig::new("gpt-4.1")
            .with_built_in_tool(BuiltinTool::code_interpreter_with_files(["file-123"]));
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"][0]["container"]["file_ids"][0], "file-123");
    }

    #[test]
    fn test_builtin_tool_image_generation() {
        let config = ProviderConfig::new("gpt-4.1")
            .with_built_in_tool(BuiltinTool::image_generation());
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"][0]["type"], "image_generation");
    }

    #[test]
    fn test_builtin_tool_computer_use() {
        let config = ProviderConfig::new("computer-use-preview")
            .with_built_in_tool(BuiltinTool::computer_use(1024, 768, "browser"));
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"][0]["type"], "computer_use_preview");
        assert_eq!(req["tools"][0]["display_width"], 1024);
        assert_eq!(req["tools"][0]["display_height"], 768);
        assert_eq!(req["tools"][0]["environment"], "browser");
    }

    #[test]
    fn test_builtin_tool_mcp() {
        let config = ProviderConfig::new("gpt-4.1").with_built_in_tool(BuiltinTool::mcp(
            McpToolConfig::new("my-server", "https://mcp.example.com/sse")
                .with_require_approval("never")
                .with_allowed_tools(vec!["search".to_string()]),
        ));
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"][0]["type"], "mcp");
        assert_eq!(req["tools"][0]["server_label"], "my-server");
        assert_eq!(req["tools"][0]["server_url"], "https://mcp.example.com/sse");
        assert_eq!(req["tools"][0]["require_approval"], "never");
        assert_eq!(req["tools"][0]["allowed_tools"][0], "search");
    }

    #[test]
    fn test_builtin_tool_local_shell() {
        let config = ProviderConfig::new("codex-mini-latest")
            .with_built_in_tool(BuiltinTool::local_shell());
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"][0]["type"], "local_shell");
    }

    #[test]
    fn test_builtin_tool_apply_patch() {
        let config = ProviderConfig::new("gpt-5.1")
            .with_built_in_tool(BuiltinTool::apply_patch());
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"][0]["type"], "apply_patch");
    }

    #[test]
    fn test_builtin_tools_combined_with_functions() {
        // Function tools and built-in tools should appear together in tools array
        let config = ProviderConfig::new("gpt-4.1")
            .with_tools(vec![crate::types::Tool::new("search", "Search", json!({"type":"object"}))])
            .with_built_in_tool(BuiltinTool::file_search());
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"].as_array().unwrap().len(), 2);
        assert_eq!(req["tools"][0]["type"], "function");
        assert_eq!(req["tools"][1]["type"], "file_search");
    }

    #[test]
    fn test_web_search_with_user_location() {
        let loc = WebSearchUserLocation::new()
            .with_country("GB")
            .with_city("London")
            .with_timezone("Europe/London");
        let config = ProviderConfig::new("gpt-4.1")
            .with_web_search(WebSearchConfig::new().with_user_location(loc));
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        let ws = &req["tools"][0];
        assert_eq!(ws["type"], "web_search_preview");
        assert_eq!(ws["user_location"]["country"], "GB");
        assert_eq!(ws["user_location"]["city"], "London");
        assert_eq!(ws["user_location"]["timezone"], "Europe/London");
    }

    #[test]
    fn test_web_search_context_size() {
        let config = ProviderConfig::new("gpt-4.1").with_web_search(
            WebSearchConfig::new().with_search_context_size("high"),
        );
        let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
        assert_eq!(req["tools"][0]["search_context_size"], "high");
    }

    #[test]
    fn test_build_request_basic() {
        let config = ProviderConfig::new("gpt-4.1")
            .with_system("Be helpful")
            .with_max_tokens(512);
        let messages = vec![Message::user("Hello")];
        let req = build_request(&messages, &config, false, None).unwrap();
        assert_eq!(req["model"], "gpt-4.1");
        assert_eq!(req["instructions"], "Be helpful");
        assert_eq!(req["max_output_tokens"], 512);
    }

    #[test]
    fn test_parse_response() {
        let json = json!({
            "id": "resp_123",
            "status": "completed",
            "model": "gpt-4.1",
            "output": [{
                "type": "message",
                "id": "msg_001",
                "status": "completed",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Hello!", "annotations": []}]
            }],
            "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello!"));
    }

    #[test]
    fn test_parse_tool_call_response() {
        let json = json!({
            "id": "resp_456",
            "status": "completed",
            "model": "gpt-4.1",
            "output": [{
                "type": "function_call",
                "id": "call_001",
                "call_id": "call_001",
                "name": "get_weather",
                "arguments": "{\"location\":\"NYC\"}",
                "status": "completed"
            }],
            "usage": {"input_tokens": 20, "output_tokens": 10, "total_tokens": 30}
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
        let provider = OpenAIResponsesProvider::new(api_key);
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
        let provider = OpenAIResponsesProvider::new(api_key);
        let config = ProviderConfig::new("gpt-4o-mini").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hi'.")];
        let stream = provider.stream(messages, config);
        let resp = collect_stream(stream).await.unwrap();
        assert!(!resp.content.is_empty());
    }
}
