use std::collections::HashMap;
use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{Provider, ProviderStream},
    providers::sse::{check_response, sse_data_stream},
    types::{
        ContentBlock, ContentBlockStart, ContentDelta, EmbeddingRequest, EmbeddingResponse,
        EmbeddingTaskType, ImageContent, MediaSource, Message, ModelInfo, ProviderConfig, Role,
        StopReason, StreamEvent, TokenCount, Tool, ToolChoice, ToolUseBlock, Usage,
    },
};

const COHERE_CHAT_URL: &str = "https://api.cohere.com/v2/chat";
const COHERE_API_BASE: &str = "https://api.cohere.com/v2";

/// Cohere Chat API v2 provider.
///
/// Supports Command R and Command R+ models with streaming, tool calling,
/// and text embeddings via the Cohere v2 API.
pub struct CohereProvider {
    api_key: String,
    client: Arc<reqwest::Client>,
    base_url: String,
    api_base: String,
}

impl CohereProvider {
    /// Create a provider from the `COHERE_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ProviderError> {
        let key = std::env::var("COHERE_API_KEY")
            .map_err(|_| ProviderError::Config("COHERE_API_KEY not set".into()))?;
        Ok(Self::new(key))
    }

    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Arc::new(reqwest::Client::new()),
            base_url: COHERE_CHAT_URL.to_string(),
            api_base: COHERE_API_BASE.to_string(),
        }
    }

    /// Override the API base URL for use with proxies or custom endpoints.
    /// All endpoints are derived from this base:
    /// - Chat:       `{base}/chat`
    /// - Models:     `{base}/models`
    /// - Embeddings: `{base}/embed`
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        let base = api_base.into();
        self.base_url = format!("{}/chat", base);
        self.api_base = base;
        self
    }
}

#[async_trait]
impl Provider for CohereProvider {
    fn provider_name(&self) -> &'static str {
        "cohere"
    }

    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let api_key = self.api_key.clone();
        let client = Arc::clone(&self.client);
        let base_url = self.base_url.clone();

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
            let mut text_started = false;
            // tool_call stream index → (id, name, block_idx)
            let mut tool_calls: HashMap<usize, (String, String, usize)> = HashMap::new();
            let mut tool_arg_bufs: HashMap<usize, String> = HashMap::new();
            let mut next_tool_block_idx: usize = 1; // 0 reserved for text

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

                let event_type = match parsed["type"].as_str() {
                    Some(t) => t,
                    None => continue,
                };

                match event_type {
                    "content-delta" => {
                        if let Some(text) = parsed["delta"]["message"]["content"]["text"].as_str() {
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
                    }

                    "tool-call-start" => {
                        let stream_idx = parsed["index"].as_u64().unwrap_or(0) as usize;
                        let id = parsed["delta"]["message"]["tool_calls"]["id"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        let name = parsed["delta"]["message"]["tool_calls"]["function"]["name"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        let block_idx = next_tool_block_idx;
                        next_tool_block_idx += 1;

                        tool_calls.insert(stream_idx, (id.clone(), name.clone(), block_idx));
                        tool_arg_bufs.insert(stream_idx, String::new());

                        yield Ok(StreamEvent::ContentBlockStart {
                            index: block_idx,
                            block: ContentBlockStart::ToolUse { id, name },
                        });
                    }

                    "tool-call-delta" => {
                        let stream_idx = parsed["index"].as_u64().unwrap_or(0) as usize;
                        if let Some(args) = parsed["delta"]["message"]["tool_calls"]["function"]["arguments"].as_str() {
                            if let Some(buf) = tool_arg_bufs.get_mut(&stream_idx) {
                                buf.push_str(args);
                            }
                            if !args.is_empty() {
                                let block_idx = tool_calls.get(&stream_idx).map(|(_, _, i)| *i)
                                    .unwrap_or(0);
                                yield Ok(StreamEvent::ContentBlockDelta {
                                    index: block_idx,
                                    delta: ContentDelta::ToolInput { partial_json: args.to_string() },
                                });
                            }
                        }
                    }

                    "content-end" => {
                        if text_started {
                            let idx = parsed["index"].as_u64().unwrap_or(0) as usize;
                            // Only close the text block if this index matches — Cohere
                            // uses index 0 for the text block
                            if idx == 0 {
                                yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                                text_started = false;
                            }
                        }
                    }

                    "message-end" => {
                        let finish_reason = parsed["delta"]["finish_reason"].as_str().unwrap_or("COMPLETE");
                        let usage = parse_cohere_usage(&parsed["delta"]["usage"]);

                        if text_started {
                            yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                        }
                        for (_, _, block_idx) in tool_calls.values() {
                            yield Ok(StreamEvent::ContentBlockStop { index: *block_idx });
                        }

                        yield Ok(StreamEvent::MessageStop {
                            stop_reason: parse_finish_reason(finish_reason),
                        });
                        yield Ok(StreamEvent::Metadata { usage, model: None, id: None });
                        return;
                    }

                    _ => {}
                }
            }

            // Stream ended without message-end
            if text_started {
                yield Ok(StreamEvent::ContentBlockStop { index: text_index });
            }
            for (_, _, block_idx) in tool_calls.values() {
                yield Ok(StreamEvent::ContentBlockStop { index: *block_idx });
            }
            yield Ok(StreamEvent::MessageStop { stop_reason: StopReason::EndTurn });
        })
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<crate::types::Response, ProviderError> {
        let body = build_request(&messages, &config, false)?;

        let mut req_builder = self
            .client
            .post(&self.base_url)
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

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = format!("{}/models", self.api_base);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let mut models = Vec::new();
        if let Some(arr) = json["models"].as_array() {
            for item in arr {
                models.push(ModelInfo {
                    id: item["name"].as_str().unwrap_or("").to_string(),
                    display_name: item["display_name"].as_str().map(|s| s.to_string()),
                    description: item["description"].as_str().map(|s| s.to_string()),
                    created_at: None,
                });
            }
        }
        Ok(models)
    }

    async fn embed(
        &self,
        request: EmbeddingRequest,
        model: &str,
    ) -> Result<EmbeddingResponse, ProviderError> {
        let url = format!("{}/embed", self.api_base);

        let input_type = request
            .task_type
            .as_ref()
            .map(|t| match t {
                EmbeddingTaskType::RetrievalQuery => "search_query",
                EmbeddingTaskType::RetrievalDocument => "search_document",
                EmbeddingTaskType::SemanticSimilarity => "semantic_similarity",
                EmbeddingTaskType::Classification => "classification",
                EmbeddingTaskType::Clustering => "clustering",
                EmbeddingTaskType::QuestionAnswering => "search_query",
                EmbeddingTaskType::FactVerification => "fact_verification",
                EmbeddingTaskType::CodeRetrievalQuery => "code",
            })
            .unwrap_or("search_document");

        let mut body = json!({
            "model": model,
            "texts": request.inputs,
            "input_type": input_type,
            "embedding_types": ["float"],
            "truncate": "END",
        });
        if let Some(dims) = request.dimensions {
            body["output_dimension"] = json!(dims);
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let mut embeddings: Vec<Vec<f32>> = Vec::new();
        if let Some(float_arr) = json["embeddings"]["float"].as_array() {
            for vec_val in float_arr {
                if let Some(vec_arr) = vec_val.as_array() {
                    let vec: Vec<f32> = vec_arr
                        .iter()
                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                        .collect();
                    embeddings.push(vec);
                }
            }
        }

        let input_tokens = json["meta"]["billed_units"]["input_tokens"]
            .as_u64()
            .unwrap_or(0);
        let usage = Usage {
            input_tokens,
            ..Default::default()
        };

        Ok(EmbeddingResponse {
            embeddings,
            model: Some(model.to_string()),
            usage,
        })
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        let url = format!("{}/tokenize", self.api_base);

        // Flatten all message text for tokenization
        let text: String = messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| {
                if let ContentBlock::Text(t) = b {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        let body = serde_json::json!({
            "text": text,
            "model": config.model,
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: serde_json::Value = resp.json().await?;

        let input_tokens = json["tokens"]
            .as_array()
            .map(|a| a.len() as u64)
            .unwrap_or(0);
        Ok(TokenCount { input_tokens })
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
    let cohere_messages = format_messages(messages, config.system.as_deref())?;

    let mut req = json!({
        "model": config.model,
        "messages": cohere_messages,
        "stream": stream,
    });

    if let Some(max_tokens) = config.max_tokens {
        req["max_tokens"] = json!(max_tokens);
    }
    if let Some(temp) = config.temperature {
        req["temperature"] = json!(temp);
    }
    if let Some(top_p) = config.top_p {
        req["p"] = json!(top_p);
    }
    if let Some(top_k) = config.top_k {
        req["k"] = json!(top_k);
    }
    if let Some(seed) = config.seed {
        req["seed"] = json!(seed);
    }
    if !config.stop_sequences.is_empty() {
        req["stop_sequences"] = json!(config.stop_sequences);
    }
    if let Some(penalty) = config.presence_penalty {
        req["presence_penalty"] = json!(penalty);
    }
    if let Some(penalty) = config.frequency_penalty {
        req["frequency_penalty"] = json!(penalty);
    }

    if !config.tools.is_empty() {
        req["tools"] = format_tools(&config.tools);
    }
    if let Some(tc) = &config.tool_choice {
        req["tool_choice"] = format_tool_choice(tc);
    }

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
        let role = match msg.role {
            Role::System => {
                result.push(
                    json!({"role": "system", "content": format_content_blocks(&msg.content)}),
                );
                continue;
            }
            Role::User => "user",
            Role::Assistant => "assistant",
        };

        // Tool results in user messages
        let has_tool_results = msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult(_)));
        if role == "user" && has_tool_results {
            for block in &msg.content {
                if let ContentBlock::ToolResult(tr) = block {
                    let content_str: String = tr
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text(t) = b {
                                Some(t.as_str())
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
            }
            // Any non-tool-result content becomes a separate user message
            let other: Vec<&ContentBlock> = msg
                .content
                .iter()
                .filter(|b| !matches!(b, ContentBlock::ToolResult(_)))
                .collect();
            if !other.is_empty() {
                let text = other
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Text(t) = b {
                            Some(t.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    result.push(json!({"role": "user", "content": text}));
                }
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
                            "function": {
                                "name": tu.name,
                                "arguments": tu.input.to_string(),
                            }
                        })
                    })
                    .collect();
                let text: String = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Text(t) = b {
                            Some(t.as_str())
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

        let content = format_content_blocks(&msg.content);
        result.push(json!({"role": role, "content": content}));
    }

    Ok(json!(result))
}

/// Format content blocks for Cohere v2 chat.
/// Returns a JSON string when text-only, or a JSON array when images are present.
fn format_content_blocks(blocks: &[ContentBlock]) -> serde_json::Value {
    let has_images = blocks.iter().any(|b| matches!(b, ContentBlock::Image(_)));
    if !has_images {
        // Text-only: return plain string (Cohere default)
        let text: String = blocks
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Text(t) = b {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");
        return json!(text);
    }
    // Mixed content: return array format
    let parts: Vec<serde_json::Value> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) if !t.is_empty() => Some(json!({"type": "text", "text": t})),
            ContentBlock::Image(img) => format_cohere_image(img),
            _ => None,
        })
        .collect();
    json!(parts)
}

fn format_cohere_image(img: &ImageContent) -> Option<serde_json::Value> {
    match &img.source {
        MediaSource::Url(url) => Some(json!({
            "type": "image_url",
            "image_url": {"url": url}
        })),
        MediaSource::Base64(b64) => {
            let data_url = format!("data:{};base64,{}", b64.media_type, b64.data);
            Some(json!({
                "type": "image_url",
                "image_url": {"url": data_url}
            }))
        }
        _ => None,
    }
}

fn format_tools(tools: &[Tool]) -> Value {
    json!(
        tools
            .iter()
            .map(|t| json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            }))
            .collect::<Vec<_>>()
    )
}

fn format_tool_choice(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Any => json!("required"),
        ToolChoice::None => json!("none"),
        // Cohere v2 does not support forcing a specific named tool via tool_choice;
        // use "required" as the closest equivalent
        ToolChoice::Tool { .. } => json!("required"),
    }
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

fn parse_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let message = &json["message"];
    let finish_reason = json["finish_reason"].as_str().unwrap_or("COMPLETE");

    let mut content: Vec<ContentBlock> = Vec::new();

    // Text content blocks
    if let Some(content_arr) = message["content"].as_array() {
        for block in content_arr {
            if block["type"].as_str() == Some("text")
                && let Some(text) = block["text"].as_str()
                && !text.is_empty()
            {
                content.push(ContentBlock::Text(text.to_string()));
            }
        }
    }

    // Tool calls
    if let Some(tool_calls) = message["tool_calls"].as_array() {
        for tc in tool_calls {
            let id = tc["id"].as_str().unwrap_or("").to_string();
            let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let input = serde_json::from_str(args_str).unwrap_or(Value::Null);
            content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
        }
    }

    let usage = parse_cohere_usage(&json["usage"]);
    let stop_reason = parse_finish_reason(finish_reason);
    let model = json["model"].as_str().map(|s| s.to_string());

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model,
        id: None,
        logprobs: None,
        grounding_metadata: None,
        warnings: vec![],
        request_body: None,
    })
}

fn parse_cohere_usage(usage: &Value) -> Usage {
    // Prefer "tokens" over "billed_units" as it has the full counts
    let tokens = &usage["tokens"];
    let billed = &usage["billed_units"];

    let input = tokens["input_tokens"]
        .as_u64()
        .or_else(|| billed["input_tokens"].as_u64())
        .unwrap_or(0);
    let output = tokens["output_tokens"]
        .as_u64()
        .or_else(|| billed["output_tokens"].as_u64())
        .unwrap_or(0);

    Usage {
        input_tokens: input,
        output_tokens: output,
        ..Default::default()
    }
}

fn parse_finish_reason(reason: &str) -> StopReason {
    match reason {
        "COMPLETE" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "TOOL_CALL" => StopReason::ToolUse,
        "ERROR" => StopReason::Other("error".to_string()),
        other => StopReason::Other(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_build_request_basic() {
        let config = ProviderConfig::new("command-r-plus").with_max_tokens(512);
        let messages = vec![Message::user("Hello")];
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["model"], "command-r-plus");
        assert_eq!(req["max_tokens"], 512);
        assert_eq!(req["stream"], false);
        assert_eq!(req["messages"][0]["role"], "user");
        assert_eq!(req["messages"][0]["content"], "Hello");
    }

    #[test]
    fn test_system_injection() {
        let config = ProviderConfig::new("command-r-plus")
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
        use crate::types::Tool;
        let config = ProviderConfig::new("command-r-plus")
            .with_tools(vec![Tool::new(
                "search",
                "Search the web",
                json!({"type": "object"}),
            )])
            .with_tool_choice(ToolChoice::Any);
        let messages = vec![Message::user("Search something")];
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["tool_choice"], "required");
        assert_eq!(req["tools"][0]["function"]["name"], "search");
    }

    #[test]
    fn test_parse_response_text() {
        let json = json!({
            "id": "chat-123",
            "finish_reason": "COMPLETE",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Hello there!"}]
            },
            "usage": {
                "billed_units": {"input_tokens": 10, "output_tokens": 5},
                "tokens": {"input_tokens": 10, "output_tokens": 5}
            }
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello there!"));
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
        assert!(matches!(resp.stop_reason, StopReason::EndTurn));
    }

    #[test]
    fn test_parse_response_tool_call() {
        let json = json!({
            "id": "chat-456",
            "finish_reason": "TOOL_CALL",
            "message": {
                "role": "assistant",
                "content": [],
                "tool_calls": [{
                    "id": "tc_abc",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"location\":\"Paris\"}"
                    }
                }]
            },
            "usage": {
                "billed_units": {"input_tokens": 20, "output_tokens": 10},
                "tokens": {"input_tokens": 20, "output_tokens": 10}
            }
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather"));
        assert!(matches!(resp.stop_reason, StopReason::ToolUse));
    }

    #[test]
    fn test_parse_finish_reasons() {
        assert!(matches!(
            parse_finish_reason("COMPLETE"),
            StopReason::EndTurn
        ));
        assert!(matches!(
            parse_finish_reason("MAX_TOKENS"),
            StopReason::MaxTokens
        ));
        assert!(matches!(
            parse_finish_reason("TOOL_CALL"),
            StopReason::ToolUse
        ));
        assert!(matches!(parse_finish_reason("ERROR"), StopReason::Other(s) if s == "error"));
    }

    #[test]
    fn test_with_api_base() {
        let provider = CohereProvider::new("key").with_api_base("https://proxy.example.com/v2");
        assert_eq!(provider.base_url, "https://proxy.example.com/v2/chat");
        assert_eq!(provider.api_base, "https://proxy.example.com/v2");
    }

    #[test]
    fn test_tool_result_formatting() {
        use crate::types::{ContentBlock, ToolResultBlock};
        let messages = vec![
            Message::user("Call the tool"),
            Message::with_content(
                Role::Assistant,
                vec![ContentBlock::ToolUse(ToolUseBlock {
                    id: "tc_1".into(),
                    name: "search".into(),
                    input: json!({"query": "rust"}),
                })],
            ),
            Message::with_content(
                Role::User,
                vec![ContentBlock::ToolResult(ToolResultBlock {
                    tool_use_id: "tc_1".into(),
                    content: vec![ContentBlock::Text("Results here".into())],
                    is_error: false,
                })],
            ),
        ];
        let formatted = format_messages(&messages, None).unwrap();
        let arr = formatted.as_array().unwrap();
        // Last message should be role=tool with the result content
        let tool_msg = arr.iter().find(|m| m["role"] == "tool").unwrap();
        assert_eq!(tool_msg["tool_call_id"], "tc_1");
        assert_eq!(tool_msg["content"], "Results here");
    }

    #[tokio::test]
    async fn test_integration_complete() {
        let api_key = match std::env::var("COHERE_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: COHERE_API_KEY not set");
                return;
            }
        };
        let provider = CohereProvider::new(api_key);
        let config = ProviderConfig::new("command-r").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hello' in one word.")];
        let resp = provider.complete(messages, config).await.unwrap();
        assert!(!resp.content.is_empty());
    }

    #[tokio::test]
    async fn test_integration_stream() {
        let api_key = match std::env::var("COHERE_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: COHERE_API_KEY not set");
                return;
            }
        };
        use crate::provider::collect_stream;
        let provider = CohereProvider::new(api_key);
        let config = ProviderConfig::new("command-r").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hi'.")];
        let stream = provider.stream(messages, config);
        let resp = collect_stream(stream).await.unwrap();
        assert!(!resp.content.is_empty());
    }
}
