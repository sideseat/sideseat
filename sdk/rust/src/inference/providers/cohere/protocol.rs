use super::*;

pub(super) fn build_request(
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

    match &config.response_format {
        Some(ResponseFormat::Json) => {
            req["response_format"] = json!({"type": "json_object"});
        }
        Some(ResponseFormat::JsonSchema { name, schema, .. }) => {
            req["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": name, "schema": schema }
            });
        }
        Some(ResponseFormat::Text) | None => {}
    }

    if let Some(budget) = config.thinking_budget {
        req["thinking"] = json!({"type": "enabled", "token_budget": budget});
    } else if config.include_thinking {
        req["thinking"] = json!({"type": "enabled"});
    }

    if config.logprobs == Some(true) || config.top_logprobs.is_some() {
        req["logprobs"] = json!(true);
    }

    for (k, v) in &config.extra {
        req[k] = v.clone();
    }

    Ok(req)
}

// ---------------------------------------------------------------------------
// Bedrock (Cohere v1 native) request building
// ---------------------------------------------------------------------------

pub(super) fn build_bedrock_request(
    messages: &[Message],
    config: &ProviderConfig,
) -> Result<Value, ProviderError> {
    let (current_message, chat_history, tool_results) = format_bedrock_messages(messages);

    let mut req = json!({ "message": current_message });

    if !chat_history.is_empty() {
        req["chat_history"] = json!(chat_history);
    }

    // Collect preamble: config.system + any Role::System messages
    let mut preamble_parts = Vec::new();
    if let Some(s) = &config.system {
        preamble_parts.push(s.as_str());
    }
    for msg in messages {
        if msg.role == Role::System {
            for block in &msg.content {
                if let ContentBlock::Text(t) = block {
                    preamble_parts.push(t.text.as_str());
                }
            }
        }
    }
    if !preamble_parts.is_empty() {
        req["preamble"] = json!(preamble_parts.join("\n\n"));
    }

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

    if !config.tools.is_empty() {
        req["tools"] = format_bedrock_tools(&config.tools);
    }

    if !tool_results.is_empty() {
        req["tool_results"] = json!(tool_results);
    }

    Ok(req)
}

/// Convert `Vec<Message>` to Cohere v1 Bedrock format.
///
/// Returns `(current_message, chat_history, tool_results)`.
fn format_bedrock_messages(messages: &[Message]) -> (String, Vec<Value>, Vec<Value>) {
    // Build a lookup from tool_use_id → (name, parameters) for populating tool_results.call
    let mut tool_use_map: HashMap<String, (String, Value)> = HashMap::new();
    for msg in messages {
        if msg.role == Role::Assistant {
            for block in &msg.content {
                if let ContentBlock::ToolUse(tu) = block {
                    tool_use_map.insert(tu.id.clone(), (tu.name.clone(), tu.input.clone()));
                }
            }
        }
    }

    let mut chat_history: Vec<Value> = Vec::new();
    let mut current_message = String::new();
    let mut tool_results: Vec<Value> = Vec::new();

    for (i, msg) in messages.iter().enumerate() {
        let is_last = i == messages.len() - 1;

        match &msg.role {
            Role::System => {
                // Handled separately as preamble; skip in chat_history
            }
            Role::User | Role::Tool | Role::Other(_) => {
                let has_tool_results = msg
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult(_)));

                if has_tool_results {
                    let results: Vec<Value> = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::ToolResult(tr) = b {
                                let output_text: String = tr
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
                                let (name, params) = tool_use_map
                                    .get(&tr.tool_use_id)
                                    .map(|(n, p)| (n.as_str(), p.clone()))
                                    .unwrap_or(("", Value::Object(Default::default())));
                                Some(json!({
                                    "call": { "name": name, "parameters": params },
                                    "outputs": [{ "text": output_text }],
                                }))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if is_last {
                        tool_results = results;
                        // Empty message signals a tool-continuation turn
                        current_message = String::new();
                    }
                    // Mid-conversation tool results are implicit via the preceding CHATBOT entry
                } else {
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

                    if is_last {
                        current_message = text;
                    } else {
                        chat_history.push(json!({ "role": "USER", "message": text }));
                    }
                }
            }
            Role::Assistant => {
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

                let mut entry = json!({ "role": "CHATBOT", "message": text });
                if !tool_uses.is_empty() {
                    let tc: Vec<Value> = tool_uses
                        .iter()
                        .map(|tu| {
                            json!({
                                "id": tu.id,
                                "name": tu.name,
                                "parameters": tu.input,
                            })
                        })
                        .collect();
                    entry["tool_calls"] = json!(tc);
                }
                chat_history.push(entry);
            }
        }
    }

    (current_message, chat_history, tool_results)
}

/// Format tools for Cohere v1 (Bedrock): `parameter_definitions` instead of JSON Schema.
fn format_bedrock_tools(tools: &[Tool]) -> Value {
    json!(
        tools
            .iter()
            .map(|t| {
                let schema = &t.input_schema;
                let required: Vec<&str> = schema["required"]
                    .as_array()
                    .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                let param_defs: serde_json::Map<String, Value> = schema["properties"]
                    .as_object()
                    .map(|props| {
                        props
                            .iter()
                            .map(|(name, prop)| {
                                let cohere_type = match prop["type"].as_str().unwrap_or("str") {
                                    "string" => "str",
                                    "integer" => "int",
                                    "number" => "float",
                                    "boolean" => "bool",
                                    "array" => "List[str]",
                                    "object" => "Dict",
                                    other => other,
                                };
                                let desc = prop["description"].as_str().unwrap_or("").to_string();
                                let is_required = required.contains(&name.as_str());
                                (
                                    name.clone(),
                                    json!({
                                        "description": desc,
                                        "type": cohere_type,
                                        "required": is_required,
                                    }),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                json!({
                    "name": t.name,
                    "description": t.description,
                    "parameter_definitions": param_defs,
                })
            })
            .collect::<Vec<_>>()
    )
}

// ---------------------------------------------------------------------------
// Message formatting (Direct v2)
// ---------------------------------------------------------------------------

fn format_messages(messages: &[Message], system: Option<&str>) -> Result<Value, ProviderError> {
    let mut result = Vec::new();

    if let Some(sys) = system {
        result.push(json!({"role": "system", "content": sys}));
    }

    for msg in messages {
        let role = match &msg.role {
            Role::System => {
                result.push(
                    json!({"role": "system", "content": format_content_blocks(&msg.content)}),
                );
                continue;
            }
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::Other(s) => s.as_str(),
        };

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
            }
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
                            Some(t.text.as_str())
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

        let content = format_content_blocks(&msg.content);
        result.push(json!({"role": role, "content": content}));
    }

    Ok(json!(result))
}

/// Format content blocks for Cohere v2 chat.
/// Returns a plain string when text-only, or an array when images are present.
fn format_content_blocks(blocks: &[ContentBlock]) -> serde_json::Value {
    let has_images = blocks.iter().any(|b| matches!(b, ContentBlock::Image(_)));
    if !has_images {
        let text: String = blocks
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
        return json!(text);
    }
    let parts: Vec<serde_json::Value> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) if !t.text.is_empty() => {
                Some(json!({"type": "text", "text": t.text}))
            }
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
        ToolChoice::Tool { .. } => json!("required"),
        // Cohere has no subset-restriction mode; "auto" preserves optional-tool semantics.
        ToolChoice::AllowedTools { .. } => json!("auto"),
    }
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

pub(super) fn parse_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let message = &json["message"];
    let finish_reason = json["finish_reason"].as_str().unwrap_or("COMPLETE");

    let mut content: Vec<ContentBlock> = Vec::new();

    if let Some(content_arr) = message["content"].as_array() {
        for block in content_arr {
            if block["type"].as_str() == Some("text")
                && let Some(text) = block["text"].as_str()
                && !text.is_empty()
            {
                content.push(ContentBlock::text(text));
            }
        }
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

    let usage = parse_cohere_usage(&json["usage"]);
    let stop_reason = parse_finish_reason(finish_reason);
    let model = json["model"].as_str().map(|s| s.to_string());
    let id = json["id"].as_str().map(|s| s.to_string());
    let logprobs = parse_logprobs(&json["logprobs"]);

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

/// Parse a Cohere v1 (Bedrock) response.
///
/// Differences from v2: `text` (not `message.content`), `parameters` (not `function.arguments`),
/// usage under `meta.billed_units` (not `usage.billed_units`).
pub(super) fn parse_bedrock_response(
    json: &Value,
) -> Result<crate::types::Response, ProviderError> {
    let finish_reason = json["finish_reason"].as_str().unwrap_or("COMPLETE");
    let mut content: Vec<ContentBlock> = Vec::new();

    if let Some(text) = json["text"].as_str()
        && !text.is_empty()
    {
        content.push(ContentBlock::text(text));
    }

    if let Some(tool_calls) = json["tool_calls"].as_array() {
        for tc in tool_calls {
            let id = tc["id"].as_str().unwrap_or("").to_string();
            let name = tc["name"].as_str().unwrap_or("").to_string();
            // v1: parameters is a plain object, not a JSON string
            let input = tc["parameters"].clone();
            content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
        }
    }

    // Cohere v1 puts usage under meta.billed_units (v2 uses usage.billed_units)
    let usage = parse_cohere_v1_usage(json);
    let stop_reason = parse_finish_reason(finish_reason);

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model: None,
        id: json["id"].as_str().map(|s| s.to_string()),
        container: None,
        logprobs: None,
        grounding_metadata: None,
        warnings: vec![],
    })
}

pub(super) fn parse_cohere_embed_response(
    json: &Value,
    model: &str,
) -> Result<EmbeddingResponse, ProviderError> {
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
        .or_else(|| json["usage"]["billed_units"]["input_tokens"].as_u64())
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

/// Parse usage from a Cohere Bedrock response. Checks all known field locations:
/// - `meta.billed_units` (v1 streaming stream-end response)
/// - `usage.billed_units` / `usage.tokens` (v2-style invoke_model response)
/// - `token_count` (v1 non-streaming fallback)
pub(super) fn parse_cohere_v1_usage(json: &Value) -> Usage {
    let input = json["meta"]["billed_units"]["input_tokens"]
        .as_u64()
        .or_else(|| json["usage"]["billed_units"]["input_tokens"].as_u64())
        .or_else(|| json["usage"]["tokens"]["input_tokens"].as_u64())
        .or_else(|| json["token_count"]["prompt_tokens"].as_u64())
        .unwrap_or(0);
    let output = json["meta"]["billed_units"]["output_tokens"]
        .as_u64()
        .or_else(|| json["usage"]["billed_units"]["output_tokens"].as_u64())
        .or_else(|| json["usage"]["tokens"]["output_tokens"].as_u64())
        .or_else(|| json["token_count"]["response_tokens"].as_u64())
        .unwrap_or(0);
    Usage {
        input_tokens: input,
        output_tokens: output,
        ..Default::default()
    }
}

pub(super) fn parse_cohere_usage(usage: &Value) -> Usage {
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

pub(super) fn parse_finish_reason(reason: &str) -> StopReason {
    match reason {
        "COMPLETE" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "TOOL_CALL" => StopReason::ToolUse,
        "STOP_SEQUENCE" => StopReason::StopSequence(String::new()),
        "ERROR" => StopReason::Other("error".to_string()),
        "TIMEOUT" => StopReason::Other("timeout".to_string()),
        other => {
            tracing::debug!(
                "Cohere: unknown finish_reason {:?}, mapping to Other",
                other
            );
            StopReason::Other(other.to_string())
        }
    }
}

/// Parse Cohere v2 logprobs into our `TokenLogprob` format.
///
/// Cohere returns `[{text, token_ids, logprobs}, ...]` — each item is a text chunk
/// with one logprob per token ID. We expand them into one `TokenLogprob` per token.
fn parse_logprobs(val: &Value) -> Option<Vec<TokenLogprob>> {
    let items = val.as_array()?;
    if items.is_empty() {
        return None;
    }
    let mut result = Vec::new();
    for item in items {
        let text = item["text"].as_str().unwrap_or("");
        let lps = item["logprobs"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or(&[]);
        let ids = item["token_ids"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(1)
            .max(1);
        for (i, lp) in lps.iter().enumerate().take(ids) {
            result.push(TokenLogprob {
                token: if i == 0 {
                    text.to_string()
                } else {
                    String::new()
                },
                logprob: lp.as_f64().unwrap_or(0.0),
                bytes: None,
                top_logprobs: vec![],
            });
        }
        if lps.is_empty() {
            result.push(TokenLogprob {
                token: text.to_string(),
                logprob: 0.0,
                bytes: None,
                top_logprobs: vec![],
            });
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

// ---------------------------------------------------------------------------
// Bedrock error classification
// ---------------------------------------------------------------------------

pub(super) fn classify_bedrock_sdk_error(msg: String) -> ProviderError {
    if msg.contains("ThrottlingException") || msg.to_lowercase().contains("throttl") {
        ProviderError::TooManyRequests {
            message: msg,
            retry_after_secs: None,
        }
    } else if msg.contains("ModelTimeoutException") {
        ProviderError::Timeout { ms: None }
    } else {
        ProviderError::Api {
            status: 0,
            message: msg,
        }
    }
}

#[cfg(test)]
#[path = "../cohere_tests.rs"]
mod tests;
