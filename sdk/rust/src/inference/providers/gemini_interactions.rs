use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{ChatProvider, Provider, ProviderStream},
    providers::sse::{check_response, sse_data_stream},
    types::{
        ContentBlock, ContentBlockStart, ContentDelta, Message, ModelInfo, ProviderConfig,
        ResponseFormat, Role, StopReason, StreamEvent, TokenCount, ToolUseBlock, Usage,
    },
};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const INTERACTIONS_PATH: &str = "/v1beta/interactions";
const MODELS_PATH: &str = "/v1beta/models";

/// Google Gemini Interactions API provider.
///
/// The Interactions API is the next-generation Gemini interface that provides:
/// - Server-side conversation history via `previous_interaction_id`
/// - Simpler request format (model in body, not URL path)
/// - Unified interface for models and agents
///
/// Compared to the legacy `generateContent` API, this API:
/// - Uses `POST /v1beta/interactions` (model-agnostic endpoint)
/// - Returns an interaction `id` for continuing conversations
/// - Streams via `?alt=sse` with typed events (`content.delta`, `interaction.complete`, etc.)
///
/// # Example
///
/// ```no_run
/// use sideseat::providers::GeminiInteractionsProvider;
/// use sideseat::{Provider, ProviderConfig, Message};
///
/// let provider = GeminiInteractionsProvider::new("your-api-key");
/// let config = ProviderConfig::new("gemini-2.5-flash").with_max_tokens(1024);
/// ```
pub struct GeminiInteractionsProvider {
    api_key: String,
    client: Arc<reqwest::Client>,
    /// Interaction ID from a previous call — enables server-side conversation history.
    pub previous_interaction_id: Option<String>,
    base_url: String,
}

impl GeminiInteractionsProvider {
    /// Create a provider from the `GEMINI_API_KEY` or `GOOGLE_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ProviderError> {
        let key = crate::env::require_any(&[
            crate::env::keys::GEMINI_API_KEY,
            crate::env::keys::GOOGLE_API_KEY,
        ])?;
        Ok(Self::new(key))
    }

    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: Arc::new(reqwest::Client::new()),
            previous_interaction_id: None,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Override the API base URL (for testing with a mock server).
    pub fn with_api_base(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    /// Replace the HTTP client. Useful for custom TLS, proxies, or testing.
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = Arc::new(client);
        self
    }

    /// Continue a previous interaction. Pass the `id` returned in the last response.
    pub fn with_previous_interaction_id(mut self, id: impl Into<String>) -> Self {
        self.previous_interaction_id = Some(id.into());
        self
    }

    fn build_url(&self, stream: bool) -> String {
        if stream {
            format!("{}{}?key={}&alt=sse", self.base_url, INTERACTIONS_PATH, self.api_key)
        } else {
            format!("{}{}?key={}", self.base_url, INTERACTIONS_PATH, self.api_key)
        }
    }

    fn build_models_url(&self) -> String {
        format!("{}{}?key={}", self.base_url, MODELS_PATH, self.api_key)
    }

    fn build_request(
        &self,
        messages: &[Message],
        config: &ProviderConfig,
        stream: bool,
    ) -> Result<Value, ProviderError> {
        let input = format_input(messages)?;

        let mut req = json!({
            "model": format!("models/{}", config.model),
            "input": input,
            "stream": stream,
        });

        if let Some(sys) = &config.system {
            req["system_instruction"] = json!(sys);
        }

        if let Some(prev_id) = &self.previous_interaction_id {
            req["previous_interaction_id"] = json!(prev_id);
        }

        // Generation config
        let mut gen_config = json!({});
        if let Some(max_tokens) = config.max_tokens {
            gen_config["max_output_tokens"] = json!(max_tokens);
        }
        if let Some(temp) = config.temperature {
            gen_config["temperature"] = json!(temp);
        }
        if let Some(top_p) = config.top_p {
            gen_config["top_p"] = json!(top_p);
        }
        if let Some(budget) = config.thinking_budget {
            gen_config["thinking_config"] = json!({"thinking_budget": budget});
        }
        // Response format
        match &config.response_format {
            Some(ResponseFormat::Json) => {
                gen_config["response_mime_type"] = json!("application/json");
            }
            Some(ResponseFormat::JsonSchema { schema, .. }) => {
                gen_config["response_mime_type"] = json!("application/json");
                gen_config["response_schema"] = schema.clone();
            }
            _ => {}
        }
        if gen_config
            .as_object()
            .map(|o| !o.is_empty())
            .unwrap_or(false)
        {
            req["generation_config"] = gen_config;
        }

        // Tools — Interactions API: [{type: "function", name, description, parameters}] (flat, not nested)
        if !config.tools.is_empty() {
            let tools: Vec<Value> = config
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    })
                })
                .collect();
            req["tools"] = json!(tools);
        }

        for (k, v) in &config.extra {
            req[k] = v.clone();
        }

        Ok(req)
    }
}

#[async_trait]
impl Provider for GeminiInteractionsProvider {
    fn provider_name(&self) -> &'static str {
        "google"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = self.build_models_url();
        let resp = self.client.get(&url).send().await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let mut models = Vec::new();
        if let Some(arr) = json["models"].as_array() {
            for item in arr {
                let id = item["name"]
                    .as_str()
                    .unwrap_or("")
                    .trim_start_matches("models/")
                    .to_string();
                models.push(ModelInfo {
                    id,
                    display_name: item["displayName"].as_str().map(|s| s.to_string()),
                    description: item["description"].as_str().map(|s| s.to_string()),
                    created_at: None,
                });
            }
        }
        Ok(models)
    }
}

#[async_trait]
impl ChatProvider for GeminiInteractionsProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let client = Arc::clone(&self.client);
        let url = self.build_url(true);
        let body = match self.build_request(&messages, &config, true) {
            Ok(b) => b,
            Err(e) => return Box::pin(stream! { yield Err(e); }),
        };

        Box::pin(stream! {
            let resp = match client
                .post(&url)
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

            yield Ok(StreamEvent::MessageStart { role: Role::Assistant });

            let mut text_started = false;
            let mut text_index: usize = 0;
            let mut next_block_index: usize = 1;
            // tool call index tracking: stream_index -> (block_idx, id, name)
            let mut tool_calls: std::collections::HashMap<usize, (usize, String, String)> = std::collections::HashMap::new();
            let mut interaction_id: Option<String> = None;

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

                // Interactions API uses "event_type" (not "type") for SSE events.
                let event_type = parsed["event_type"].as_str()
                    .or_else(|| parsed["type"].as_str())
                    .unwrap_or("");

                match event_type {
                    "interaction.start" => {
                        // ID is nested: {"interaction": {"id": "..."}, "event_type": "..."}
                        interaction_id = parsed["interaction"]["id"].as_str().map(|s| s.to_string());
                    }

                    "content.start" => {
                        let index = parsed["index"].as_u64().unwrap_or(0) as usize;
                        // content type is nested: {"content": {"type": "text"}, ...}
                        let content_type = parsed["content"]["type"].as_str().unwrap_or("text");
                        match content_type {
                            "text" => {
                                text_index = index;
                                yield Ok(StreamEvent::ContentBlockStart {
                                    index,
                                    block: ContentBlockStart::Text,
                                });
                                text_started = true;
                            }
                            "thought" => {
                                yield Ok(StreamEvent::ContentBlockStart {
                                    index,
                                    block: ContentBlockStart::Thinking,
                                });
                            }
                            _ => {}
                        }
                        if index >= next_block_index { next_block_index = index + 1; }
                    }

                    "content.delta" => {
                        let index = parsed["index"].as_u64().unwrap_or(0) as usize;
                        let delta = &parsed["delta"];
                        let delta_type = delta["type"].as_str().unwrap_or("");

                        match delta_type {
                            "text" => {
                                if let Some(text) = delta["text"].as_str() && !text.is_empty() {
                                    yield Ok(StreamEvent::ContentBlockDelta {
                                        index,
                                        delta: ContentDelta::Text { text: text.to_string() },
                                    });
                                }
                            }
                            "thought" => {
                                if let Some(thinking) = delta["thought"].as_str() && !thinking.is_empty() {
                                    yield Ok(StreamEvent::ContentBlockDelta {
                                        index,
                                        delta: ContentDelta::Thinking { thinking: thinking.to_string() },
                                    });
                                }
                            }
                            // Tool calls arrive as complete objects in a single delta
                            "function_call" => {
                                let id = delta["id"].as_str().unwrap_or("").to_string();
                                let name = delta["name"].as_str().unwrap_or("").to_string();
                                let args = delta["arguments"].clone();
                                let block_idx = next_block_index;
                                next_block_index += 1;
                                tool_calls.insert(index, (block_idx, id.clone(), name.clone()));

                                yield Ok(StreamEvent::ContentBlockStart {
                                    index: block_idx,
                                    block: ContentBlockStart::ToolUse { id, name },
                                });
                                let args_str = args.to_string();
                                if !args_str.is_empty() && args_str != "null" {
                                    yield Ok(StreamEvent::ContentBlockDelta {
                                        index: block_idx,
                                        delta: ContentDelta::ToolInput { partial_json: args_str },
                                    });
                                }
                                yield Ok(StreamEvent::ContentBlockStop { index: block_idx });
                            }
                            _ => {}
                        }
                    }

                    "content.stop" => {
                        let index = parsed["index"].as_u64().unwrap_or(0) as usize;
                        // Don't close tool calls here — already closed in content.delta
                        if !tool_calls.values().any(|(bi, _, _)| *bi == index) {
                            yield Ok(StreamEvent::ContentBlockStop { index });
                            if index == text_index { text_started = false; }
                        }
                    }

                    "interaction.complete" => {
                        if text_started {
                            yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                        }
                        // All data nested: {"interaction": {"usage": ..., "status": ..., "id": ...}}
                        let interaction = &parsed["interaction"];
                        let usage = parse_usage(&interaction["usage"]);
                        let stop_reason = parse_status(interaction["status"].as_str().unwrap_or("completed"));
                        let id = interaction["id"].as_str()
                            .map(|s| s.to_string())
                            .or(interaction_id.take());
                        yield Ok(StreamEvent::MessageStop { stop_reason });
                        yield Ok(StreamEvent::Metadata { usage, model: None, id });
                        return;
                    }

                    "error" => {
                        let code = parsed["error"]["code"].as_str().unwrap_or("");
                        let msg = parsed["error"]["message"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_string();
                        let err = if code == "too_many_requests" {
                            ProviderError::TooManyRequests { message: msg, retry_after_secs: None }
                        } else {
                            ProviderError::Api { status: 0, message: msg }
                        };
                        yield Err(err);
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
        let url = self.build_url(false);
        let body = self.build_request(&messages, &config, false)?;

        let resp = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;
        parse_response(&json)
    }

    async fn count_tokens(
        &self,
        _messages: Vec<Message>,
        _config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        Err(ProviderError::Unsupported(
            "count_tokens is not available for the Interactions API; use GeminiProvider".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Input formatting
// ---------------------------------------------------------------------------

fn format_input(messages: &[Message]) -> Result<Value, ProviderError> {
    // The Interactions API manages conversation history server-side via previous_interaction_id.
    // Client-side history is not supported; extract only the last user message.
    let last_user = messages
        .iter()
        .filter(|m| m.role == Role::User)
        .next_back()
        .ok_or_else(|| ProviderError::InvalidRequest("No user message in input".into()))?;

    let all_text = last_user.content.iter().all(|b| matches!(b, ContentBlock::Text(_)));
    if all_text {
        let text = last_user
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
        return Ok(json!(text));
    }

    // Multi-modal: format as Content object
    let parts: Result<Vec<Value>, _> = last_user.content.iter().map(format_part).collect();
    Ok(json!({"role": "user", "parts": parts?}))
}

fn format_part(block: &ContentBlock) -> Result<Value, ProviderError> {
    match block {
        ContentBlock::Text(t) => Ok(json!({"text": t.text})),
        ContentBlock::ToolUse(tu) => Ok(json!({
            "function_call": {"name": tu.name, "args": tu.input}
        })),
        ContentBlock::ToolResult(tr) => {
            let output: String = tr
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
            Ok(
                json!({"function_response": {"name": tr.tool_use_id, "response": {"output": output}}}),
            )
        }
        _ => Err(ProviderError::Unsupported(
            "Only text, tool use, and tool results are supported in Interactions API".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

fn parse_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let id = json["id"].as_str().map(|s| s.to_string());
    let status = json["status"].as_str().unwrap_or("completed");
    let stop_reason = parse_status(status);

    let mut content: Vec<ContentBlock> = Vec::new();

    if let Some(outputs) = json["outputs"].as_array() {
        for output in outputs {
            let output_type = output["type"].as_str().unwrap_or("text");
            match output_type {
                "text" => {
                    if let Some(text) = output["text"].as_str()
                        && !text.is_empty()
                    {
                        content.push(ContentBlock::text(text));
                    }
                }
                "thought" => {
                    if let Some(thinking) = output["thought"].as_str() {
                        use crate::types::ThinkingBlock;
                        content.push(ContentBlock::Thinking(ThinkingBlock {
                            thinking: thinking.to_string(),
                            signature: None,
                        }));
                    }
                }
                "function_call" => {
                    let id = output["id"].as_str().unwrap_or("").to_string();
                    let name = output["name"].as_str().unwrap_or("").to_string();
                    let input = output["arguments"].clone();
                    content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
                }
                _ => {}
            }
        }
    }

    let usage = parse_usage(&json["usage"]);
    let model = json["model"]
        .as_str()
        .map(|s| s.trim_start_matches("models/").to_string());

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
        request_body: None,
    })
}

fn parse_usage(usage: &Value) -> Usage {
    Usage {
        input_tokens: usage["total_input_tokens"]
            .as_u64()
            .or_else(|| usage["prompt_tokens"].as_u64())
            .or_else(|| usage["input_tokens"].as_u64())
            .unwrap_or(0),
        output_tokens: usage["total_output_tokens"]
            .as_u64()
            .or_else(|| usage["candidates_tokens"].as_u64())
            .or_else(|| usage["output_tokens"].as_u64())
            .unwrap_or(0),
        ..Default::default()
    }
}

fn parse_status(status: &str) -> StopReason {
    match status {
        "completed" => StopReason::EndTurn,
        "requires_action" => StopReason::ToolUse,
        "failed" => StopReason::Other("failed".to_string()),
        _ => StopReason::EndTurn,
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
    fn test_single_user_message_as_string() {
        let provider = GeminiInteractionsProvider::new("key");
        let messages = vec![Message::user("Hello!")];
        let config = ProviderConfig::new("gemini-2.5-flash");
        let req = provider.build_request(&messages, &config, false).unwrap();
        assert_eq!(req["input"], "Hello!");
        assert_eq!(req["model"], "models/gemini-2.5-flash");
    }

    #[test]
    fn test_multi_turn_uses_last_user_message() {
        // The Interactions API manages history server-side; only the last user message is sent.
        let provider = GeminiInteractionsProvider::new("key");
        let messages = vec![
            Message::user("Hi"),
            Message::assistant("Hello there"),
            Message::user("How are you?"),
        ];
        let config = ProviderConfig::new("gemini-2.5-flash");
        let req = provider.build_request(&messages, &config, false).unwrap();
        assert_eq!(req["input"], "How are you?");
    }

    #[test]
    fn test_previous_interaction_id() {
        let provider =
            GeminiInteractionsProvider::new("key").with_previous_interaction_id("prev-123");
        let messages = vec![Message::user("Continue")];
        let config = ProviderConfig::new("gemini-2.5-flash");
        let req = provider.build_request(&messages, &config, false).unwrap();
        assert_eq!(req["previous_interaction_id"], "prev-123");
    }

    #[test]
    fn test_system_instruction() {
        let provider = GeminiInteractionsProvider::new("key");
        let messages = vec![Message::user("Hi")];
        let config = ProviderConfig::new("gemini-2.5-flash").with_system("Be concise");
        let req = provider.build_request(&messages, &config, false).unwrap();
        assert_eq!(req["system_instruction"], "Be concise");
    }

    #[test]
    fn test_parse_response() {
        // Test with real Interactions API v2 usage field names.
        let json = json!({
            "id": "interaction-abc123",
            "status": "completed",
            "outputs": [{"type": "text", "text": "Hello world"}],
            "usage": {"total_input_tokens": 7, "total_output_tokens": 2, "total_tokens": 9},
            "model": "models/gemini-2.5-flash"
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello world"));
        assert_eq!(resp.id.as_deref(), Some("interaction-abc123"));
        assert_eq!(resp.model.as_deref(), Some("gemini-2.5-flash"));
        assert_eq!(resp.usage.input_tokens, 7);
        assert_eq!(resp.usage.output_tokens, 2);
    }

    #[test]
    fn test_parse_response_legacy_usage_fields() {
        // Legacy fallback: older field names still parsed correctly.
        let json = json!({
            "id": "interaction-legacy",
            "status": "completed",
            "outputs": [{"type": "text", "text": "Hi"}],
            "usage": {"prompt_tokens": 10, "candidates_tokens": 5},
            "model": "models/gemini-2.5-flash"
        });
        let resp = parse_response(&json).unwrap();
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn test_parse_tool_call_response() {
        let json = json!({
            "id": "interaction-tool",
            "status": "requires_action",
            "outputs": [{
                "type": "function_call",
                "id": "call_1",
                "name": "get_weather",
                "arguments": {"city": "NYC"}
            }],
            "usage": {"prompt_tokens": 20, "candidates_tokens": 8}
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather"));
        assert!(matches!(resp.stop_reason, StopReason::ToolUse));
    }
}
