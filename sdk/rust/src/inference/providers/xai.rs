use std::collections::HashMap;
use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{ChatProvider, EmbeddingProvider, Provider, ProviderStream},
    providers::{
        openai_common::{OpenAIInnerClient, parse_usage},
        sse::{check_response, sse_data_stream},
    },
    types::{
        ContentBlock, ContentBlockStart, ContentDelta, EmbeddingRequest, EmbeddingResponse,
        ImageContent, MediaSource, Message, ModelInfo, ProviderConfig, ResponseFormat, Role,
        StreamEvent, ThinkingBlock, TokenCount, Tool, ToolChoice, ToolUseBlock,
    },
};

const XAI_CHAT_URL: &str = "https://api.x.ai/v1/chat/completions";
const XAI_API_BASE: &str = "https://api.x.ai/v1";

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// xAI Grok provider.
///
/// Supports all Grok models via the xAI API, including vision-capable models
/// (grok-4, grok-2-vision) and reasoning models (grok-3-mini with
/// `reasoning_effort`). Built-in Live Search (web + X/Twitter) is available via
/// [`WebSearchConfig`](crate::types::WebSearchConfig) and `config.extra`.
///
/// ```no_run
/// use sideseat::providers::XAIProvider;
/// let provider = XAIProvider::new("my-api-key");
/// ```
pub struct XAIProvider {
    api_key: String,
    client: Arc<reqwest::Client>,
    chat_url: String,
    api_base: String,
}

impl XAIProvider {
    /// Create a provider from the `XAI_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ProviderError> {
        Ok(Self::new(crate::env::require(crate::env::keys::XAI_API_KEY)?))
    }

    /// Create a provider with an xAI API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Arc::new(reqwest::Client::new()),
            chat_url: XAI_CHAT_URL.to_string(),
            api_base: XAI_API_BASE.to_string(),
        }
    }

    /// Replace the HTTP client. Useful for custom TLS, proxies, or testing.
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = Arc::new(client);
        self
    }

    /// Override the API base URL (for proxies or local testing).
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        let base = base.into();
        self.chat_url = format!("{}/chat/completions", base);
        self.api_base = base;
        self
    }
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for XAIProvider {
    fn provider_name(&self) -> &'static str {
        "xai"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let inner = OpenAIInnerClient::new(&self.api_key, Arc::clone(&self.client), &self.api_base);
        inner.list_models().await
    }
}

// ---------------------------------------------------------------------------
// ChatProvider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl ChatProvider for XAIProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let api_key = self.api_key.clone();
        let client = Arc::clone(&self.client);
        let chat_url = self.chat_url.clone();

        Box::pin(stream! {
            let body = match build_request(&messages, &config, true) {
                Ok(b) => b,
                Err(e) => { yield Err(e); return; }
            };

            let mut req_builder = client
                .post(&chat_url)
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
            let reasoning_index: usize = 1;
            let mut text_started = false;
            let mut reasoning_started = false;
            // Tool calls start at index 2 (after text=0, reasoning=1)
            let tool_base_index: usize = 2;
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

                // Usage-only chunk (final chunk with stream_options include_usage)
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
                let finish_reason = choice["finish_reason"].as_str();

                // Reasoning content (grok-3-mini and other reasoning models)
                if let Some(thinking) = delta["reasoning_content"].as_str()
                    && !thinking.is_empty()
                {
                    if !reasoning_started {
                        yield Ok(StreamEvent::ContentBlockStart {
                            index: reasoning_index,
                            block: ContentBlockStart::Thinking,
                        });
                        reasoning_started = true;
                    }
                    yield Ok(StreamEvent::ContentBlockDelta {
                        index: reasoning_index,
                        delta: ContentDelta::Thinking { thinking: thinking.to_string() },
                    });
                }

                // Text content delta
                if let Some(text) = delta["content"].as_str() {
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

                // Tool call deltas
                if let Some(tc_arr) = delta["tool_calls"].as_array() {
                    if reasoning_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: reasoning_index });
                        reasoning_started = false;
                    }
                    if text_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                        text_started = false;
                    }

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
                                let idx = tool_calls.get(&stream_idx).map(|(_, _, i)| *i)
                                    .unwrap_or(block_idx);
                                yield Ok(StreamEvent::ContentBlockDelta {
                                    index: idx,
                                    delta: ContentDelta::ToolInput { partial_json: args.to_string() },
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
                    for (_, _, block_idx) in tool_calls.values() {
                        yield Ok(StreamEvent::ContentBlockStop { index: *block_idx });
                    }
                    yield Ok(StreamEvent::MessageStop {
                        stop_reason: parse_xai_finish_reason(reason),
                    });
                }
            }
        })
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<crate::types::Response, ProviderError> {
        let body = build_request(&messages, &config, false)?;

        let mut req_builder = self.client
            .post(&self.chat_url)
            .bearer_auth(&self.api_key)
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
        let mut count_config = config.clone();
        count_config.max_tokens = Some(1);
        let body = build_request(&messages, &count_config, false)?;

        let mut req_builder = self.client
            .post(&self.chat_url)
            .bearer_auth(&self.api_key)
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

// ---------------------------------------------------------------------------
// EmbeddingProvider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl EmbeddingProvider for XAIProvider {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        let inner = OpenAIInnerClient::new(&self.api_key, Arc::clone(&self.client), &self.api_base);
        inner.embed(request).await
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
    let formatted = format_messages(messages, config.system.as_deref(), config.inject_system_as_user_message)?;

    let mut req = json!({
        "model": config.model,
        "messages": formatted,
        "stream": stream,
    });

    if stream {
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
    if let Some(seed) = config.seed {
        req["seed"] = json!(seed);
    }
    if !config.stop_sequences.is_empty() {
        req["stop"] = json!(config.stop_sequences);
    }
    // xAI supports reasoning_effort for grok-3-mini and other reasoning models
    if let Some(effort) = &config.reasoning_effort {
        req["reasoning_effort"] = json!(effort.as_str());
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
    if let Some(user) = &config.user {
        req["user"] = json!(user);
    }
    if let Some(store) = config.store {
        req["store"] = json!(store);
    }
    if let Some(tier) = &config.service_tier {
        req["service_tier"] = json!(tier.as_str());
    }
    if let Some(logprobs) = config.logprobs {
        req["logprobs"] = json!(logprobs);
    }
    if let Some(top_n) = config.top_logprobs {
        req["top_logprobs"] = json!(top_n);
        if config.logprobs.is_none() {
            req["logprobs"] = json!(true);
        }
    }
    if let Some(parallel) = config.parallel_tool_calls
        && (!config.tools.is_empty() || config.web_search.is_some())
    {
        req["parallel_tool_calls"] = json!(parallel);
    }

    // Tools
    let mut all_tools: Vec<Value> = format_tools(&config.tools);

    // Built-in web search: xAI uses {"type": "web_search"}, not "web_search_preview"
    if let Some(ws) = &config.web_search {
        all_tools.push(format_web_search_tool(ws));
    }
    if !all_tools.is_empty() {
        req["tools"] = json!(all_tools);
    }
    if let Some(tc) = &config.tool_choice {
        req["tool_choice"] = format_tool_choice(tc);
    }

    // Response format
    if let Some(fmt) = &config.response_format {
        req["response_format"] = format_response_format(fmt);
    }

    // xAI supports an `include` array for inline citations and encrypted reasoning
    // Users can pass this via config.extra["include"] = ["inline_citations"]
    for (k, v) in &config.extra {
        req[k] = v.clone();
    }

    Ok(req)
}

fn format_web_search_tool(ws: &crate::types::WebSearchConfig) -> Value {
    // xAI uses {"type": "web_search", ...} directly — no function wrapper
    let mut tool = json!({"type": "web_search"});
    if let Some(allowed) = &ws.allowed_domains {
        tool["allowed_domains"] = json!(allowed);
    }
    if let Some(blocked) = &ws.blocked_domains {
        tool["excluded_domains"] = json!(blocked);
    }
    tool
}

fn format_response_format(fmt: &ResponseFormat) -> Value {
    match fmt {
        ResponseFormat::Text => json!({"type": "text"}),
        ResponseFormat::Json => json!({"type": "json_object"}),
        ResponseFormat::JsonSchema { name, schema, strict } => {
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

// ---------------------------------------------------------------------------
// Message formatting
// ---------------------------------------------------------------------------

fn format_messages(
    messages: &[Message],
    system: Option<&str>,
    inject_system_as_user: bool,
) -> Result<Value, ProviderError> {
    let mut result: Vec<Value> = Vec::new();

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
                let content = format_content(&msg.content)?;
                if inject_system_as_user {
                    let text = content.as_str().unwrap_or("");
                    result.push(json!({"role": "user", "content": format!("<system>{}</system>", text)}));
                } else {
                    result.push(json!({"role": "system", "content": content}));
                }
                continue;
            }
            Role::User => "user",
            Role::Tool => "tool",
            Role::Assistant => "assistant",
            Role::Other(s) => s.as_str(),
        };

        // Tool results → role=tool messages
        let has_tool_results = msg.content.iter().any(|b| matches!(b, ContentBlock::ToolResult(_)));
        if (role == "user" || role == "tool") && has_tool_results {
            let mut other: Vec<&ContentBlock> = Vec::new();
            for block in &msg.content {
                match block {
                    ContentBlock::ToolResult(tr) => {
                        let content_str: String = tr.content.iter()
                            .filter_map(|b| if let ContentBlock::Text(t) = b { Some(t.text.as_str()) } else { None })
                            .collect::<Vec<_>>()
                            .join("\n");
                        result.push(json!({
                            "role": "tool",
                            "tool_call_id": tr.tool_use_id,
                            "content": content_str,
                        }));
                    }
                    o => other.push(o),
                }
            }
            if !other.is_empty() {
                let owned: Vec<ContentBlock> = other.into_iter().cloned().collect();
                result.push(json!({"role": "user", "content": format_content(&owned)?}));
            }
            continue;
        }

        // Tool calls in assistant messages
        if role == "assistant" {
            let tool_uses: Vec<_> = msg.content.iter()
                .filter_map(|b| if let ContentBlock::ToolUse(tu) = b { Some(tu) } else { None })
                .collect();
            if !tool_uses.is_empty() {
                let tc_vals: Vec<Value> = tool_uses.iter().map(|tu| json!({
                    "id": tu.id,
                    "type": "function",
                    "function": {"name": tu.name, "arguments": tu.input.to_string()},
                })).collect();
                let text: String = msg.content.iter()
                    .filter_map(|b| if let ContentBlock::Text(t) = b { Some(t.text.as_str()) } else { None })
                    .collect::<Vec<_>>().join("");
                let mut am = json!({"role": "assistant", "tool_calls": tc_vals});
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
            "Content type not supported in xAI messages".into(),
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
            "xAI images require URL or base64 source".into(),
        )),
    }
}

fn format_tools(tools: &[Tool]) -> Vec<Value> {
    tools.iter().map(|t| {
        let mut schema = t.input_schema.clone();
        if t.strict && let Some(obj) = schema.as_object_mut() {
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
    }).collect()
}

fn format_tool_choice(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Any => json!("required"),
        ToolChoice::None => json!("none"),
        ToolChoice::Tool { name } => json!({"type": "function", "function": {"name": name}}),
        // xAI doesn't support allowed_tools subset; "auto" is the closest valid option
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

    // Reasoning content (grok-3-mini, other reasoning models)
    if let Some(thinking) = message["reasoning_content"].as_str()
        && !thinking.is_empty()
    {
        content.push(ContentBlock::Thinking(ThinkingBlock {
            thinking: thinking.to_string(),
            signature: None,
        }));
    }

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
    let stop_reason = parse_xai_finish_reason(finish_reason);
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
        request_body: None,
    })
}

fn parse_logprobs(val: &Value) -> Option<Vec<crate::types::TokenLogprob>> {
    let content = val["content"].as_array()?;
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
                                b.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect()
                            }),
                        })
                        .collect()
                })
                .unwrap_or_default();
            crate::types::TokenLogprob {
                token: item["token"].as_str().unwrap_or("").to_string(),
                logprob: item["logprob"].as_f64().unwrap_or(0.0),
                bytes: item["bytes"].as_array().map(|b| {
                    b.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect()
                }),
                top_logprobs,
            }
        })
        .collect();
    if tokens.is_empty() { None } else { Some(tokens) }
}

// ---------------------------------------------------------------------------
// xAI-specific finish reason mapping
// ---------------------------------------------------------------------------

/// Parse xAI finish reasons. Mostly identical to OpenAI but adds "reasoning"
/// which indicates the model completed its reasoning chain (maps to EndTurn).
fn parse_xai_finish_reason(reason: &str) -> crate::types::StopReason {
    match reason {
        "stop" | "reasoning" => crate::types::StopReason::EndTurn,
        "length" => crate::types::StopReason::MaxTokens,
        "tool_calls" | "function_call" => crate::types::StopReason::ToolUse,
        "content_filter" => crate::types::StopReason::ContentFilter,
        other => crate::types::StopReason::Other(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_messages() -> Vec<Message> {
        vec![Message::user("Hello")]
    }

    #[test]
    fn test_build_request_basic() {
        let config = ProviderConfig::new("grok-3-mini").with_max_tokens(100);
        let req = build_request(&make_messages(), &config, false).unwrap();
        assert_eq!(req["model"], json!("grok-3-mini"));
        assert_eq!(req["stream"], json!(false));
        assert_eq!(req["max_tokens"], json!(100));
        assert!(req.get("stream_options").is_none());
    }

    #[test]
    fn test_build_request_streaming() {
        let config = ProviderConfig::new("grok-3").with_max_tokens(50);
        let req = build_request(&make_messages(), &config, true).unwrap();
        assert_eq!(req["stream"], json!(true));
        assert_eq!(req["stream_options"]["include_usage"], json!(true));
    }

    #[test]
    fn test_build_request_reasoning_effort() {
        use crate::types::ReasoningEffort;
        let mut config = ProviderConfig::new("grok-3-mini").with_max_tokens(100);
        config.reasoning_effort = Some(ReasoningEffort::High);
        let req = build_request(&make_messages(), &config, false).unwrap();
        assert_eq!(req["reasoning_effort"], json!("high"));
    }

    #[test]
    fn test_build_request_web_search() {
        use crate::types::WebSearchConfig;
        let mut config = ProviderConfig::new("grok-3").with_max_tokens(100);
        config.web_search = Some(WebSearchConfig {
            max_uses: None,
            allowed_domains: Some(vec!["example.com".to_string()]),
            blocked_domains: None,
            search_context_size: None,
            user_location: None,
        });
        let req = build_request(&make_messages(), &config, false).unwrap();
        let tools = req["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        // xAI uses {"type": "web_search", ...} directly — no function wrapper
        assert_eq!(tools[0]["type"], json!("web_search"));
        assert_eq!(tools[0]["allowed_domains"][0], json!("example.com"));
    }

    #[test]
    fn test_build_request_response_format_json() {
        let mut config = ProviderConfig::new("grok-3").with_max_tokens(100);
        config.response_format = Some(ResponseFormat::Json);
        let req = build_request(&make_messages(), &config, false).unwrap();
        assert_eq!(req["response_format"]["type"], json!("json_object"));
    }

    #[test]
    fn test_build_request_system_prompt() {
        let config = ProviderConfig {
            model: "grok-3".to_string(),
            max_tokens: Some(50),
            system: Some("You are a pirate.".to_string()),
            ..Default::default()
        };
        let req = build_request(&make_messages(), &config, false).unwrap();
        let msgs = req["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], json!("system"));
        assert_eq!(msgs[0]["content"], json!("You are a pirate."));
    }

    #[test]
    fn test_parse_finish_reason_reasoning() {
        assert_eq!(
            parse_xai_finish_reason("reasoning"),
            crate::types::StopReason::EndTurn
        );
        assert_eq!(
            parse_xai_finish_reason("stop"),
            crate::types::StopReason::EndTurn
        );
        assert_eq!(
            parse_xai_finish_reason("tool_calls"),
            crate::types::StopReason::ToolUse
        );
    }

    #[test]
    fn test_parse_response_with_reasoning() {
        let json = json!({
            "id": "resp-123",
            "model": "grok-3-mini",
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "reasoning_content": "Let me think about this...",
                    "content": "The answer is 42."
                }
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30,
                "completion_tokens_details": {"reasoning_tokens": 5}
            }
        });
        let resp = parse_response(&json).unwrap();
        assert_eq!(resp.content.len(), 2);
        assert!(matches!(resp.content[0], ContentBlock::Thinking(_)));
        assert!(matches!(resp.content[1], ContentBlock::Text(_)));
        assert_eq!(resp.usage.reasoning_tokens, 5);
        assert_eq!(resp.id.as_deref(), Some("resp-123"));
    }

    #[test]
    fn test_parse_response_tool_call() {
        let json = json!({
            "id": "resp-456",
            "model": "grok-3",
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"message\":\"hello\"}"
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 15, "completion_tokens": 10, "total_tokens": 25}
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(resp.stop_reason, crate::types::StopReason::ToolUse));
        let tool = resp.content.iter().find(|b| matches!(b, ContentBlock::ToolUse(_))).unwrap();
        if let ContentBlock::ToolUse(tu) = tool {
            assert_eq!(tu.name, "echo");
            assert_eq!(tu.input["message"], json!("hello"));
        }
    }

    #[test]
    fn test_format_messages_vision() {
        use crate::types::{ImageContent, MediaSource};
        let messages = vec![Message {
            role: Role::User,
            content: vec![
                ContentBlock::text("What is in this image?".to_string()),
                ContentBlock::Image(ImageContent {
                    source: MediaSource::Url("https://example.com/img.png".to_string()),
                    format: None,
                    detail: None,
                }),
            ],
            name: None,
            cache_control: None,
        }];
        let formatted = format_messages(&messages, None, false).unwrap();
        let arr = formatted.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let parts = arr[0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], json!("text"));
        assert_eq!(parts[1]["type"], json!("image_url"));
    }
}
