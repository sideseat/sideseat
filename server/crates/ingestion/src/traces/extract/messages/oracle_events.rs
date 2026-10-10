use super::*;

// ============================================================================
// RETIRED EXTRACTION ORACLES
// ============================================================================

#[cfg(test)]
pub(crate) fn try_gen_ai_indexed(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let prompt_indices = extract_indices(attrs, "gen_ai.prompt");
    let completion_indices = extract_indices(attrs, "gen_ai.completion");

    if prompt_indices.is_empty() && completion_indices.is_empty() {
        return false;
    }

    for idx in prompt_indices {
        if let Some(msg) = extract_indexed_message(attrs, "gen_ai.prompt", idx, timestamp) {
            messages.push(msg);
        }
    }

    for idx in completion_indices {
        if let Some(msg) = extract_indexed_message(attrs, "gen_ai.completion", idx, timestamp) {
            messages.push(msg);
        }
    }

    !messages.is_empty()
}

/// OTEL standard GenAI messages (gen_ai.input/output.messages).
#[cfg(test)]
pub(crate) fn try_otel_genai_messages(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    span_name: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // gen_ai.system_instructions - system prompt as parts array (PydanticAI v2+)
    if let Some(instructions_json) = attrs.get(keys::GEN_AI_SYSTEM_INSTRUCTIONS) {
        if let Ok(parts) = serde_json::from_str::<JsonValue>(instructions_json) {
            let msg = json!({
                "role": "system",
                "parts": parts
            });
            messages.push(RawMessage::from_attr(
                keys::GEN_AI_SYSTEM_INSTRUCTIONS,
                timestamp,
                msg,
            ));
            found = true;
        }
    }

    // gen_ai.input.messages - store as-is, expand at query time in SideML pipeline
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::GEN_AI_INPUT_MESSAGES) {
        messages.push(RawMessage::from_attr(
            keys::GEN_AI_INPUT_MESSAGES,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // gen_ai.output.messages - store as-is, expand at query time in SideML pipeline
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::GEN_AI_OUTPUT_MESSAGES) {
        messages.push(RawMessage::from_attr(
            keys::GEN_AI_OUTPUT_MESSAGES,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // pydantic_ai.all_messages - full conversation history on agent run spans.
    // Keep the literal array: query-time carrier semantics expand it into turns
    // and deduplicate those snapshots against the model-call spans.
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::PYDANTIC_AI_ALL_MESSAGES) {
        messages.push(RawMessage::from_attr(
            keys::PYDANTIC_AI_ALL_MESSAGES,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // The tool's name and call id, which the two attributes below need and which live beside them on the
    // same span. Emitting the arguments without them produced a nameless, id-less tool call that the
    // pipeline then discarded - so a producer following the current conventions had its tool calls extracted
    // and *then* dropped, which is worse than not reading them, because every layer looked fine.
    // The span name is the fallback for the tool's name, because the conventions put it there too:
    // `execute_tool {name}` is the prescribed span name, and a producer that omits the attribute still
    // names the tool in the span. An empty name is what makes a call unusable downstream.
    let tool_name = attrs.get(keys::GEN_AI_TOOL_NAME).cloned().or_else(|| {
        span_name
            .strip_prefix("execute_tool ")
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
    });
    let tool_call_id = attrs.get(keys::GEN_AI_TOOL_CALL_ID);

    // gen_ai.tool.call.arguments - the call
    if let Some(args_json) = attrs.get(keys::GEN_AI_TOOL_CALL_ARGUMENTS) {
        let args = serde_json::from_str::<JsonValue>(args_json).unwrap_or(json!(args_json));
        // A `tool_use` content block, which is what a call *is* - not a bare object under a role. The
        // block shape is what carries the name and the id through normalisation, and the id is what lets
        // the result be paired with this call rather than guessed at by content.
        let mut block = serde_json::Map::new();
        block.insert("type".to_string(), json!("tool_use"));
        block.insert(
            "name".to_string(),
            json!(tool_name.clone().unwrap_or_default()),
        );
        if let Some(id) = tool_call_id {
            block.insert("id".to_string(), json!(id));
        }
        block.insert("input".to_string(), args);
        messages.push(RawMessage::from_attr(
            keys::GEN_AI_TOOL_CALL_ARGUMENTS,
            timestamp,
            json!({"role": "assistant", "content": [JsonValue::Object(block)]}),
        ));
        found = true;
    }

    // gen_ai.tool.call.result - the answer to it
    if let Some(result_json) = attrs.get(keys::GEN_AI_TOOL_CALL_RESULT) {
        let result = serde_json::from_str::<JsonValue>(result_json).unwrap_or(json!(result_json));
        let mut block = serde_json::Map::new();
        block.insert("type".to_string(), json!("tool_result"));
        if let Some(name) = &tool_name {
            block.insert("name".to_string(), json!(name));
        }
        if let Some(id) = tool_call_id {
            block.insert("tool_use_id".to_string(), json!(id));
        }
        block.insert("content".to_string(), result);
        messages.push(RawMessage::from_attr(
            keys::GEN_AI_TOOL_CALL_RESULT,
            timestamp,
            json!({"role": "tool", "content": [JsonValue::Object(block)]}),
        ));
        found = true;
    }

    // Tool definitions are now extracted by extract_tool_definitions() which runs on all spans
    // including tool execution spans. This avoids duplication and ensures tool defs are always captured.

    found
}

#[cfg(test)]
pub(crate) fn try_openinference(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // LLM input/output messages
    let input_indices = extract_indices(attrs, "llm.input_messages");
    let output_indices = extract_indices(attrs, "llm.output_messages");

    for idx in input_indices {
        if let Some(msg) =
            extract_openinference_message(attrs, "llm.input_messages", idx, timestamp)
        {
            messages.push(msg);
            found = true;
        }
    }

    for idx in output_indices {
        if let Some(msg) =
            extract_openinference_message(attrs, "llm.output_messages", idx, timestamp)
        {
            messages.push(msg);
            found = true;
        }
    }

    // llm.tools is extracted by extract_tool_definitions() which runs on all spans

    // retrieval.documents.N.* - Retrieved documents for RAG spans
    let retrieval_indices = extract_indices(attrs, "retrieval.documents");
    if !retrieval_indices.is_empty() {
        if let Some(msg) = extract_openinference_documents(
            attrs,
            "retrieval.documents",
            &retrieval_indices,
            timestamp,
        ) {
            messages.push(msg);
            found = true;
        }
    }

    // reranker.input_documents.N.* and reranker.output_documents.N.*
    let reranker_input_indices = extract_indices(attrs, "reranker.input_documents");
    let reranker_output_indices = extract_indices(attrs, "reranker.output_documents");

    if !reranker_input_indices.is_empty() {
        if let Some(msg) = extract_openinference_documents(
            attrs,
            "reranker.input_documents",
            &reranker_input_indices,
            timestamp,
        ) {
            messages.push(msg);
            found = true;
        }
    }

    if !reranker_output_indices.is_empty() {
        if let Some(msg) = extract_openinference_documents(
            attrs,
            "reranker.output_documents",
            &reranker_output_indices,
            timestamp,
        ) {
            messages.push(msg);
            found = true;
        }
    }

    // reranker.query - Reranker query string
    if let Some(query) = attrs.get(keys::RERANKER_QUERY) {
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("user"));
        msg.insert("content".to_string(), json!(query));
        msg.insert("_source".to_string(), json!("reranker.query"));
        messages.push(RawMessage::from_attr(
            keys::RERANKER_QUERY,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    // embedding.text - Input text for embedding spans
    if let Some(text) = attrs.get(keys::EMBEDDING_TEXT) {
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("user"));
        msg.insert("content".to_string(), json!(text));
        msg.insert("_source".to_string(), json!("embedding.text"));
        messages.push(RawMessage::from_attr(
            keys::EMBEDDING_TEXT,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    if found {
        enrich_oi_multimodal_from_input_value(messages, attrs);
    }

    found
}

/// Enrich OpenInference multimodal messages with richer content from `input.value`.
///
/// OI dotted attributes lose file blocks and use `__REDACTED__` URLs.
/// `input.value` contains the complete LangChain-serialized content with all blocks.
/// For user messages with multimodal `contents.*` dotted keys, replace the dotted-key
/// content with the richer content array from `input.value`.
#[cfg(test)]
pub(super) fn enrich_oi_multimodal_from_input_value(
    messages: &mut [RawMessage],
    attrs: &HashMap<String, String>,
) {
    // Fast path: check if any extracted message has multimodal contents.* dotted keys
    let has_multimodal = messages.iter().any(|m| {
        if let Some(obj) = m.content.as_object() {
            obj.keys().any(|k| k.starts_with("contents."))
        } else {
            false
        }
    });
    if !has_multimodal {
        return;
    }

    // Parse input.value
    let input_json = match attrs.get(keys::INPUT_VALUE) {
        Some(v) => v,
        None => return,
    };
    let parsed = match serde_json::from_str::<JsonValue>(input_json) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Navigate to message array: {"messages": [[m1,m2]]} or {"messages": [m1,m2]} or [m1,m2]
    let msg_array = find_input_value_messages(&parsed);
    let msg_array = match msg_array {
        Some(arr) if !arr.is_empty() => arr,
        _ => return,
    };

    // LangChain format guard: verify messages have LangChain structure
    let is_langchain = msg_array.iter().any(|m| {
        m.get("id").and_then(|v| v.as_array()).is_some()
            || m.get("lc").is_some()
            || m.get("kwargs").is_some()
    });
    if !is_langchain {
        return;
    }

    // Build index map: OI message index → input.value content
    // OI source keys look like "llm.input_messages.N.message"
    for msg in messages.iter_mut() {
        let oi_index = match extract_oi_message_index(msg) {
            Some(idx) => idx,
            None => continue,
        };

        // Check this message has multimodal contents.* keys
        let has_contents = msg
            .content
            .as_object()
            .is_some_and(|obj| obj.keys().any(|k| k.starts_with("contents.")));
        if !has_contents {
            continue;
        }

        // Get corresponding input.value message
        let iv_msg = match msg_array.get(oi_index) {
            Some(m) => m,
            None => continue,
        };

        // Extract content via LangChain format
        let iv_content = match extract_langchain_content(iv_msg) {
            Some(c) => c,
            None => continue,
        };

        // Only replace if input.value has array content (multimodal).
        // input.value is strictly higher quality than OI dotted keys:
        // real URLs instead of __REDACTED__, all block types preserved.
        if !iv_content.is_array() {
            continue;
        }

        // Replace: remove all contents.* keys, set content = array
        if let Some(obj) = msg.content.as_object_mut() {
            let keys_to_remove: Vec<String> = obj
                .keys()
                .filter(|k| k.starts_with("contents."))
                .cloned()
                .collect();
            for key in keys_to_remove {
                obj.remove(&key);
            }
            obj.insert("content".to_string(), iv_content);
        }
    }
}

/// Find message array in input.value JSON.
/// Handles: {"messages": [[m1,m2]]}, {"messages": [m1,m2]}, [m1,m2]
#[cfg(test)]
pub(super) fn find_input_value_messages(parsed: &JsonValue) -> Option<&Vec<JsonValue>> {
    // {"messages": ...}
    if let Some(msgs) = parsed.get("messages") {
        if let Some(arr) = msgs.as_array() {
            // Nested: {"messages": [[m1,m2]]}
            if arr.len() == 1 {
                if let Some(inner) = arr[0].as_array() {
                    return Some(inner);
                }
            }
            // Direct: {"messages": [m1,m2]}
            return Some(arr);
        }
    }
    // Direct array: [m1,m2]
    parsed.as_array()
}

/// Extract the OI message index from a RawMessage source key.
/// Source keys look like "llm.input_messages.N.message" → returns N.
#[cfg(test)]
pub(super) fn extract_oi_message_index(msg: &RawMessage) -> Option<usize> {
    let key = match &msg.source {
        MessageSource::Attribute { key, .. } => key,
        _ => return None,
    };
    // Pattern: "llm.input_messages.N.message" or "llm.input_messages.N"
    if !key.starts_with("llm.input_messages.") {
        return None;
    }
    let rest = key.strip_prefix("llm.input_messages.")?;
    let idx_str = rest.split('.').next()?;
    idx_str.parse().ok()
}

#[cfg(test)]
pub(crate) fn try_logfire_events(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // Try "events" attribute
    if let Some(events_json) = attrs.get(keys::EVENTS) {
        if let Ok(parsed) = serde_json::from_str::<Vec<JsonValue>>(events_json) {
            found |= extract_logfire_event_array(messages, parsed, keys::EVENTS, timestamp);
        }
    }

    // Also check "prompt" attribute for input
    if let Some(prompt_json) = attrs.get(keys::PROMPT) {
        if let Ok(parsed) = serde_json::from_str::<Vec<JsonValue>>(prompt_json) {
            for msg in parsed {
                if let JsonValue::Object(raw) = msg {
                    messages.push(RawMessage::from_attr(
                        keys::PROMPT,
                        timestamp,
                        JsonValue::Object(raw),
                    ));
                    found = true;
                }
            }
        }
    }

    // Also check "all_messages_events" attribute for output
    if let Some(all_msgs_json) = attrs.get(keys::ALL_MESSAGES_EVENTS) {
        if let Ok(parsed) = serde_json::from_str::<Vec<JsonValue>>(all_msgs_json) {
            for msg in parsed {
                if let JsonValue::Object(raw) = msg {
                    messages.push(RawMessage::from_attr(
                        keys::ALL_MESSAGES_EVENTS,
                        timestamp,
                        JsonValue::Object(raw),
                    ));
                    found = true;
                }
            }
        }
    }

    // Logfire Chat Completions / Anthropic Messages: request_data/response_data.
    // Stored as-is — structural extraction happens at query time
    // via MESSAGE_ARRAY_SOURCES expansion in normalize.rs.
    //
    // `request_data` keeps its precedence; `response_data` does not, and the asymmetry is the point.
    //
    // The gate used to cover both, so it asked about a *different* carrier than the one it guarded: a span
    // whose `events` hold only the question and whose `response_data` holds the answer returned the question
    // alone, because `events` had already set `found`. `_synthetic/logfire_partial_events` is that shape.
    //
    // `request_data` duplicates the *input* side that `events` also carries, and no captured Logfire fixture
    // exists to show that the two spellings hash alike - so if they differ slightly, reading both would put
    // the same question on screen twice, and downstream dedup could not tell. `response_data` carries the
    // output side, which is what was being dropped, so it is read whatever the events said. Narrow on
    // purpose: it closes a loss without risking a duplicate that nothing in the corpus can rule out.
    if !found {
        if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::REQUEST_DATA) {
            if parsed
                .get("messages")
                .and_then(|m| m.as_array())
                .is_some_and(|a| !a.is_empty())
            {
                messages.push(RawMessage::from_attr(keys::REQUEST_DATA, timestamp, parsed));
                found = true;
            }
        }
    }
    {
        if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::RESPONSE_DATA) {
            // Non-streaming: {message: {role, ...}, usage: {...}}
            let has_message = parsed.get("message").is_some_and(|m| m.is_object());
            // Streaming: {combined_chunk_content: "...", chunk_count: N}
            let has_streaming = parsed
                .get("combined_chunk_content")
                .and_then(|c| c.as_str())
                .is_some_and(|s| !s.is_empty());
            if has_message || has_streaming {
                messages.push(RawMessage::from_attr(
                    keys::RESPONSE_DATA,
                    timestamp,
                    parsed,
                ));
                found = true;
            }
        }
    }

    found
}

/// Extract messages from Logfire event array.
///
/// Logfire embeds OTEL-style events in an attribute as a JSON array.
/// Each event has "event.name" that can be used for query-time role derivation.
/// We preserve the raw content including event.name for query-time processing.
#[cfg(test)]
pub(super) fn extract_logfire_event_array(
    messages: &mut Vec<RawMessage>,
    events: Vec<JsonValue>,
    _source_key: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // First pass: extract recognized message events
    for event in &events {
        let Some(raw) = event.as_object() else {
            continue;
        };

        let event_name = raw.get("event.name").and_then(|e| e.as_str()).unwrap_or("");

        if !is_message_event(sideseat_domain::rules::ruleset(), event_name) {
            continue;
        }

        messages.push(RawMessage::from_event(event_name, timestamp, event.clone()));
        found = true;
    }

    // Second pass: collect unrecognized events with structured data.
    // Logfire emits multimodal content blocks (input_text, input_image,
    // input_file, etc.) as gen_ai.unknown events. The actual content is in
    // the `data` object, while `content` is a human-readable summary.
    // Group consecutive same-role blocks into a single synthetic message.
    let mut pending_blocks: Vec<JsonValue> = Vec::new();
    let mut pending_role: Option<&str> = None;

    for event in &events {
        let Some(raw) = event.as_object() else {
            continue;
        };
        let event_name = raw.get("event.name").and_then(|e| e.as_str()).unwrap_or("");
        if is_message_event(sideseat_domain::rules::ruleset(), event_name) {
            continue;
        }

        let Some(data) = raw.get("data").filter(|d| d.is_object()) else {
            continue;
        };
        let Some(data_type) = data.get("type").and_then(|t| t.as_str()) else {
            continue;
        };

        let role = if data_type.starts_with("input_") {
            "user"
        } else if data_type.starts_with("output_") {
            "assistant"
        } else {
            continue;
        };

        // Flush if role changed
        if let Some(pr) = pending_role {
            if pr != role {
                let event_name = if pr == "user" {
                    keys::EVENT_USER_MESSAGE
                } else {
                    keys::EVENT_ASSISTANT_MESSAGE
                };
                let blocks = std::mem::take(&mut pending_blocks);
                let msg = json!({"role": pr, "content": JsonValue::Array(blocks)});
                messages.push(RawMessage::from_event(event_name, timestamp, msg));
            }
        }
        pending_role = Some(role);
        pending_blocks.push(data.clone());
        found = true;
    }

    // Final flush
    if let Some(role) = pending_role {
        if !pending_blocks.is_empty() {
            let event_name = if role == "user" {
                keys::EVENT_USER_MESSAGE
            } else {
                keys::EVENT_ASSISTANT_MESSAGE
            };
            let blocks = std::mem::take(&mut pending_blocks);
            let msg = json!({"role": role, "content": JsonValue::Array(blocks)});
            messages.push(RawMessage::from_event(event_name, timestamp, msg));
        }
    }

    found
}

#[cfg(test)]
pub(crate) fn try_vercel_ai(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    span_name: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // Input messages - try ai.prompt.messages first, then ai.prompt
    let prompt_json = attrs
        .get(keys::AI_PROMPT_MESSAGES)
        .or_else(|| attrs.get(keys::AI_PROMPT));

    if let Some(prompt_json) = prompt_json {
        match serde_json::from_str::<Vec<JsonValue>>(prompt_json) {
            Ok(prompt_msgs) => {
                tracing::debug!(
                    prompt_count = prompt_msgs.len(),
                    "try_vercel_ai parsed prompt messages"
                );
                for msg_val in prompt_msgs {
                    let raw = match msg_val {
                        JsonValue::Object(map) => map,
                        _ => continue,
                    };
                    messages.push(RawMessage::from_attr(
                        keys::AI_PROMPT_MESSAGES,
                        timestamp,
                        JsonValue::Object(raw),
                    ));
                    found = true;
                }
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    prompt_json_preview = %prompt_json.chars().take(100).collect::<String>(),
                    "try_vercel_ai failed to parse prompt messages"
                );
            }
        }
    }

    // Tool definitions are extracted by extract_tool_definitions() which runs on all spans

    // Tool call input
    if let Some(tool_args) = attrs.get(keys::AI_TOOLCALL_ARGS) {
        let tool_name = attrs.get(keys::AI_TOOLCALL_NAME).map(|s| s.as_str());
        let tool_id = attrs.get(keys::AI_TOOLCALL_ID).map(|s| s.as_str());
        let args_val = serde_json::from_str::<JsonValue>(tool_args).unwrap_or(json!(tool_args));
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("tool_call"));
        if let Some(name) = tool_name {
            msg.insert("name".to_string(), json!(name));
        }
        if let Some(id) = tool_id {
            msg.insert("tool_call_id".to_string(), json!(id));
        }
        msg.insert("content".to_string(), args_val);
        messages.push(RawMessage::from_attr(
            keys::AI_TOOLCALL_ARGS,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    // Response - collect ai.response.* attributes with fallback to ai.result.*, then output.value
    let mut raw = serde_json::Map::new();

    // Check if this looks like a Vercel span before using output.value fallback
    // Only fall back to output.value if we have other Vercel-specific indicators
    // Including span name pattern (ai.* spans like ai.generateText)
    let has_vercel_indicators = attrs.contains_key(keys::AI_PROMPT_MESSAGES)
        || attrs.contains_key(keys::AI_PROMPT)
        || attrs.contains_key("ai.response.text")
        || attrs.contains_key(keys::AI_RESULT_TEXT)
        || attrs.contains_key("ai.response.toolCalls")
        || attrs.contains_key(keys::AI_RESULT_TOOL_CALLS)
        || attrs.contains_key(keys::AI_TOOLCALL_NAME)
        || span_name.starts_with("ai.");

    // Text content - try new attribute first, then legacy
    // Only fall back to output.value if we have evidence this is a Vercel span
    let text = attrs
        .get("ai.response.text")
        .or_else(|| attrs.get(keys::AI_RESULT_TEXT))
        .or_else(|| {
            if has_vercel_indicators {
                attrs.get(keys::OUTPUT_VALUE)
            } else {
                None
            }
        });
    if let Some(t) = text {
        raw.insert("content".to_string(), json!(t));
    }

    // Tool calls - try new attribute first, then legacy
    let tool_calls = attrs
        .get("ai.response.toolCalls")
        .or_else(|| attrs.get(keys::AI_RESULT_TOOL_CALLS));
    if let Some(tc) = tool_calls {
        let parsed = serde_json::from_str::<JsonValue>(tc).unwrap_or(json!(tc));
        raw.insert("tool_calls".to_string(), parsed); // Use snake_case for SideML
    }

    // Structured object - try new attribute first, then legacy
    let object = attrs
        .get("ai.response.object")
        .or_else(|| attrs.get(keys::AI_RESULT_OBJECT));
    if let Some(obj) = object {
        let parsed = parse_json_with_fallback(obj, "ai.result.object");
        raw.insert("object".to_string(), parsed);
    }

    // Collect any other ai.response.* attributes (but not text/toolCalls/object)
    #[expect(
        clippy::iter_over_hash_type,
        reason = "a test oracle compared as a JSON map, whose equality does not depend on key order"
    )]
    for (key, value) in attrs {
        if let Some(suffix) = key.strip_prefix("ai.response.") {
            if suffix != "text" && suffix != "toolCalls" && suffix != "object" {
                let json_val = if value.starts_with('{') || value.starts_with('[') {
                    parse_json_with_fallback(value, &format!("ai.response.{}", suffix))
                } else {
                    json!(value)
                };
                raw.insert(suffix.to_string(), json_val);
            }
        }
    }

    if !raw.is_empty() {
        raw.insert("role".to_string(), json!("assistant"));
        messages.push(RawMessage::from_attr(
            "ai.response",
            timestamp,
            JsonValue::Object(raw),
        ));
        found = true;
    }

    // Tool call result
    if let Some(tool_result) = attrs.get(keys::AI_TOOLCALL_RESULT) {
        let tool_name = attrs.get(keys::AI_TOOLCALL_NAME).map(|s| s.as_str());
        let tool_id = attrs.get(keys::AI_TOOLCALL_ID).map(|s| s.as_str());
        let result_val =
            serde_json::from_str::<JsonValue>(tool_result).unwrap_or(json!(tool_result));
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("tool"));
        if let Some(name) = tool_name {
            msg.insert("name".to_string(), json!(name));
        }
        if let Some(id) = tool_id {
            msg.insert("tool_call_id".to_string(), json!(id));
        }
        msg.insert("content".to_string(), result_val);
        messages.push(RawMessage::from_attr(
            keys::AI_TOOLCALL_RESULT,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    found
}
