use super::*;

// ---------------------------------------------------------------------------
// Request building (free function)
// ---------------------------------------------------------------------------

pub(super) fn build_request(
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

pub(super) fn parse_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
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
