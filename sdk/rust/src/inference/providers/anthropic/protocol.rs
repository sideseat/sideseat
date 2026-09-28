use super::*;

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/// Build a request body for the direct Anthropic Messages API.
pub(super) fn build_messages_request(
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
    if let Some(tier) = &config.service_tier {
        req["service_tier"] = json!(tier.as_str());
    }
    if let Some(id) = &config.container_id {
        req["container"] = json!(id);
    }
    if let Some(geo) = &config.inference_geo {
        req["inference_geo"] = json!(geo);
    }
    Ok(req)
}

/// Build a request body for Bedrock (no `model` or `stream` fields).
pub(super) fn build_bedrock_request(
    messages: &[Message],
    config: &ProviderConfig,
) -> Result<Value, ProviderError> {
    let mut req = json!({
        "anthropic_version": BEDROCK_ANTHROPIC_VERSION,
        "max_tokens": config.max_tokens.unwrap_or(1024),
    });
    apply_common_fields(&mut req, messages, config)?;
    // Bedrock: temperature and top_p cannot both be specified — drop top_p when temperature is set
    if req.get("temperature").is_some() {
        req.as_object_mut().map(|o| o.remove("top_p"));
    }
    // Bedrock invoke_model does not accept disable_parallel_tool_use (anywhere)
    if let Some(tc) = req.get_mut("tool_choice").and_then(|v| v.as_object_mut()) {
        tc.remove("disable_parallel_tool_use");
    }
    Ok(req)
}

/// Build a request body for Vertex AI (no `model` field).
pub(super) fn build_vertex_request(
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
pub(super) fn compute_auto_betas(config: &ProviderConfig, _messages: &[Message]) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();

    // Web search requires its own beta
    if config.web_search.is_some() {
        result.push(betas::WEB_SEARCH.to_string());
    }

    // Note: basic ephemeral (5-min) cache is now GA (no beta needed).
    // Extended TTL (1h) needs betas::EXTENDED_CACHE_TTL; add it manually via with_beta().

    result
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
                        system_blocks.push(json!({"type": "text", "text": t.text}));
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
                // Anthropic has no subset restriction; fall back to auto
                ToolChoice::AllowedTools { .. } => json!({"type": "auto"}),
            };
        }
    }

    if !tools_arr.is_empty() {
        req["tools"] = json!(tools_arr);
    }

    if let Some(user) = &config.user {
        req["metadata"] = json!({"user_id": user});
    }

    // Parallel tool calls: `disable_parallel_tool_use` lives inside `tool_choice`, not at
    // the top level. If no tool_choice was set, default to {"type":"auto"}.
    if let Some(parallel) = config.parallel_tool_calls
        && !parallel
        && req.get("tools").is_some()
    {
        if let Some(obj) = req.get_mut("tool_choice").and_then(|v| v.as_object_mut()) {
            obj.insert("disable_parallel_tool_use".to_string(), json!(true));
        } else {
            req["tool_choice"] = json!({"type": "auto", "disable_parallel_tool_use": true});
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Message formatting
// ---------------------------------------------------------------------------

fn format_messages(messages: &[Message]) -> Result<Value, ProviderError> {
    let mut result = Vec::new();
    for msg in messages {
        let role = match &msg.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::System => continue,
            Role::Other(s) => s.as_str(),
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
        .filter(|b| !matches!(b, ContentBlock::Text(t) if t.text.is_empty()))
        .collect();
    let parts: Result<Vec<Value>, _> = filtered.iter().map(|b| format_content_block(b)).collect();
    Ok(json!(parts?))
}

fn format_content_block(block: &ContentBlock) -> Result<Value, ProviderError> {
    match block {
        ContentBlock::Text(text) => Ok(json!({"type": "text", "text": text.text})),
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
                "thinking": th.text,
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

pub(super) fn format_image_block(img: &ImageContent) -> Result<Value, ProviderError> {
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
        MediaSource::Url(url) => Ok(json!({
            "type": "document",
            "source": {"type": "url", "url": url}
        })),
        MediaSource::Text(text) => Ok(json!({
            "type": "document",
            "source": {"type": "text", "media_type": "text/plain", "data": text}
        })),
        _ => Err(ProviderError::Unsupported(
            "Anthropic documents require base64, URL, or text source".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// SSE event parsing (shared by Direct and Vertex backends)
// ---------------------------------------------------------------------------

pub(super) fn parse_sse_events(data: &str) -> Vec<Result<StreamEvent, ProviderError>> {
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
            // Emit id from message_start so collect_stream can capture it
            if let Some(id) = parsed["message"]["id"].as_str() {
                let input_tokens = parsed["message"]["usage"]["input_tokens"]
                    .as_u64()
                    .unwrap_or(0);
                events.push(Ok(StreamEvent::Metadata {
                    usage: crate::types::Usage {
                        input_tokens,
                        ..Default::default()
                    },
                    model: parsed["message"]["model"].as_str().map(|s| s.to_string()),
                    id: Some(id.to_string()),
                }));
            }
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
                    ContentDelta::Thinking { text: thinking }
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
            let stop_reason = if stop_reason_str == "stop_sequence" {
                StopReason::StopSequence(
                    parsed["delta"]["stop_sequence"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                )
            } else {
                parse_stop_reason(stop_reason_str)
            };
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

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

pub(super) fn parse_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let content_arr = json["content"].as_array().ok_or_else(|| {
        ProviderError::Serialization("Missing 'content' in Anthropic response".into())
    })?;

    let content: Vec<ContentBlock> = content_arr.iter().filter_map(parse_content_block).collect();

    let stop_reason = {
        let reason = json["stop_reason"].as_str().unwrap_or("end_turn");
        if reason == "stop_sequence" {
            StopReason::StopSequence(json["stop_sequence"].as_str().unwrap_or("").to_string())
        } else {
            parse_stop_reason(reason)
        }
    };

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
    let id = json["id"].as_str().map(|s| s.to_string());
    let container = json
        .get("container")
        .and_then(|c| serde_json::from_value::<ContainerInfo>(c.clone()).ok());

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model,
        id,
        container,
        logprobs: None,
        grounding_metadata: None,
        warnings: vec![],
    })
}

fn parse_content_block(block: &Value) -> Option<ContentBlock> {
    match block["type"].as_str()? {
        "text" => {
            let text = block["text"].as_str()?.to_string();
            let citations = block
                .get("citations")
                .and_then(|c| serde_json::from_value::<Vec<Citation>>(c.clone()).ok())
                .unwrap_or_default();
            Some(ContentBlock::Text(TextBlock { text, citations }))
        }
        "tool_use" => Some(ContentBlock::ToolUse(ToolUseBlock {
            id: block["id"].as_str()?.to_string(),
            name: block["name"].as_str()?.to_string(),
            input: block["input"].clone(),
        })),
        "thinking" => Some(ContentBlock::Thinking(ThinkingBlock {
            text: block["thinking"].as_str()?.to_string(),
            signature: block["signature"].as_str().map(|s| s.to_string()),
        })),
        "image" => {
            let source = &block["source"];
            let src = match source["type"].as_str()? {
                "base64" => MediaSource::Base64(Base64Data {
                    media_type: source["media_type"].as_str()?.to_string(),
                    data: source["data"].as_str()?.to_string(),
                }),
                "url" => MediaSource::Url(source["url"].as_str()?.to_string()),
                _ => return None,
            };
            Some(ContentBlock::Image(ImageContent {
                source: src,
                format: None,
                detail: None,
            }))
        }
        _ => None,
    }
}

pub(super) fn parse_stop_reason(s: &str) -> StopReason {
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

pub(super) async fn list_models_direct(
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

pub(super) async fn count_tokens_direct(
    client: &reqwest::Client,
    api_key: &str,
    base_url: &str,
    betas: &[String],
    messages: Vec<Message>,
    config: ProviderConfig,
) -> Result<TokenCount, ProviderError> {
    let mut body = build_messages_request(&messages, &config, false)?;
    // count_tokens only accepts: model, messages, system, tools, tool_choice, thinking.
    // Use an allowlist so any future fields added to build_messages_request don't leak in.
    if let Some(obj) = body.as_object_mut() {
        obj.retain(|k, _| {
            matches!(
                k.as_str(),
                "model" | "messages" | "system" | "tools" | "tool_choice" | "thinking"
            )
        });
    }

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
