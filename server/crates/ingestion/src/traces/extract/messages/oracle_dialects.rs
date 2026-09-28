use super::*;

#[cfg(test)]
pub(crate) fn try_google_adk(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // LLM spans - detect by presence of request/response attributes (not span name)
    // Handle empty "{}" - fall back to tool_call_args when request is empty
    if let Some(request_json) = attrs.get(keys::GCP_VERTEX_LLM_REQUEST) {
        if let Ok(request) = serde_json::from_str::<JsonValue>(request_json) {
            // Check if request is meaningful (not empty object)
            let is_empty = matches!(&request, JsonValue::Object(m) if m.is_empty());
            if !is_empty {
                found |=
                    extract_adk_request_messages(messages, tool_definitions, &request, timestamp);
            }
        }
    }

    // Fall back to tool_call_args when llm_request is empty or missing
    if !found {
        if let Some(tool_args_json) = attrs.get(keys::GCP_VERTEX_TOOL_CALL_ARGS) {
            if let Ok(tool_args) = serde_json::from_str::<JsonValue>(tool_args_json) {
                let msg = json!({
                    "role": "tool_call",
                    "content": tool_args
                });
                messages.push(RawMessage::from_attr(
                    keys::GCP_VERTEX_TOOL_CALL_ARGS,
                    timestamp,
                    msg,
                ));
                found = true;
            }
        }
    }

    // Response - try llm_response first
    if let Some(response_json) = attrs.get(keys::GCP_VERTEX_LLM_RESPONSE) {
        if let Ok(response) = serde_json::from_str::<JsonValue>(response_json) {
            // Check if response is meaningful (not empty object)
            let is_empty = matches!(&response, JsonValue::Object(m) if m.is_empty());
            if !is_empty {
                found |= extract_adk_response_message(messages, &response, timestamp);
            }
        }
    }

    // Tool spans - detect by presence of tool response attribute (not span name)
    if let Some(response_json) = attrs.get(keys::GCP_VERTEX_TOOL_RESPONSE) {
        if let Ok(response) = serde_json::from_str::<JsonValue>(response_json) {
            let msg = json!({
                "role": "tool",
                "content": response
            });
            messages.push(RawMessage::from_attr(
                keys::GCP_VERTEX_TOOL_RESPONSE,
                timestamp,
                msg,
            ));
            found = true;
        }
    }

    // gcp.vertex.agent.data - data sent to agent (conversation history from trace_send_data)
    if let Some(data_json) = attrs.get(keys::GCP_VERTEX_DATA) {
        if let Ok(data) = serde_json::from_str::<JsonValue>(data_json) {
            // Check if data is meaningful (not empty)
            let is_empty = matches!(&data, JsonValue::Object(m) if m.is_empty())
                || matches!(&data, JsonValue::Array(a) if a.is_empty());
            if !is_empty {
                let msg = json!({
                    "role": "data",
                    "type": "conversation_history",
                    "content": data
                });
                messages.push(RawMessage::from_attr(keys::GCP_VERTEX_DATA, timestamp, msg));
                found = true;
            }
        }
    }

    found
}

/// Extract messages from ADK LLM request.
/// Format: {model, config: {system_instruction, tools}, contents: [{parts, role}, ...]}
/// Also handles Vertex AI native format: {systemInstruction: {parts: [...]}, contents: [...]}
#[cfg(test)]
pub(super) fn extract_adk_request_messages(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    request: &JsonValue,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // Extract system instruction - try multiple formats:
    // 1. Vertex AI native: systemInstruction.parts (camelCase, object with parts)
    // 2. Vertex AI native: systemInstruction as string
    // 3. ADK format: config.system_instruction (snake_case, string)
    let system_msg = request
        .get("systemInstruction")
        .and_then(|si| {
            // Try as object with parts first
            if let Some(parts) = si.get("parts") {
                Some(json!({
                    "role": "system",
                    "content": parts
                }))
            } else {
                // Fall back to string
                si.as_str()
                    .filter(|s| !s.is_empty())
                    .map(|text| json!({"role": "system", "content": text}))
            }
        })
        .or_else(|| {
            // ADK format: config.system_instruction as string
            request
                .get("config")
                .and_then(|c| c.get("system_instruction"))
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .map(|text| {
                    json!({
                        "role": "system",
                        "content": text
                    })
                })
        });

    if let Some(msg) = system_msg {
        messages.push(RawMessage::from_attr(
            keys::GCP_VERTEX_LLM_REQUEST,
            timestamp,
            msg,
        ));
        found = true;
    }

    // Extract tools - try multiple formats:
    // 1. Vertex AI native: top-level tools array
    // 2. ADK format: config.tools
    let tools = request
        .get("tools")
        .and_then(|t| t.as_array())
        .filter(|t| !t.is_empty())
        .or_else(|| {
            request
                .get("config")
                .and_then(|c| c.get("tools"))
                .and_then(|t| t.as_array())
                .filter(|t| !t.is_empty())
        });

    if let Some(tools) = tools {
        // Unwrap function_declarations (snake_case ADK) or functionDeclarations (camelCase Vertex)
        let mut flattened = Vec::new();
        for tool_group in tools {
            let decls = tool_group
                .get("function_declarations")
                .or_else(|| tool_group.get("functionDeclarations"))
                .and_then(|fd| fd.as_array());
            if let Some(decls) = decls {
                flattened.extend(decls.iter().cloned());
            } else {
                flattened.push(tool_group.clone());
            }
        }
        if !flattened.is_empty() {
            tool_definitions.push(RawToolDefinition::from_attr(
                keys::GCP_VERTEX_LLM_REQUEST,
                timestamp,
                JsonValue::Array(flattened),
            ));
        }
        found = true;
    }

    // Extract messages from contents array
    if let Some(contents) = request.get("contents").and_then(|c| c.as_array()) {
        for content in contents {
            // Convert Gemini format: {parts, role} -> {role, content: parts}
            if let (Some(role), Some(parts)) = (
                content.get("role").and_then(|r| r.as_str()),
                content.get("parts"),
            ) {
                let msg = json!({
                    "role": role,
                    "content": parts
                });
                messages.push(RawMessage::from_attr(
                    keys::GCP_VERTEX_LLM_REQUEST,
                    timestamp,
                    msg,
                ));
                found = true;
            }
        }
    }

    found
}

/// Extract message from ADK LLM response.
/// Format: {model_version, content: {parts, role}, finish_reason, usage_metadata}
#[cfg(test)]
pub(super) fn extract_adk_response_message(
    messages: &mut Vec<RawMessage>,
    response: &JsonValue,
    timestamp: DateTime<Utc>,
) -> bool {
    let Some(content) = response.get("content") else {
        return false;
    };

    let (Some(role), Some(parts)) = (
        content.get("role").and_then(|r| r.as_str()),
        content.get("parts"),
    ) else {
        return false;
    };

    let finish_reason = response
        .get("finish_reason")
        .and_then(|f| f.as_str())
        .map(|s| s.to_lowercase());

    let mut msg = json!({
        "role": if role == "model" { "assistant" } else { role },
        "content": parts
    });
    if let Some(fr) = finish_reason {
        msg["finish_reason"] = json!(fr);
    }
    messages.push(RawMessage::from_attr(
        keys::GCP_VERTEX_LLM_RESPONSE,
        timestamp,
        msg,
    ));
    true
}

/// LiveKit message extraction
#[cfg(test)]
pub(crate) fn try_livekit(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // lk.instructions - system instructions
    if let Some(instructions) = attrs.get(keys::LK_INSTRUCTIONS) {
        if !instructions.is_empty() {
            let msg = json!({
                "role": "system",
                "content": instructions
            });
            messages.push(RawMessage::from_attr(keys::LK_INSTRUCTIONS, timestamp, msg));
            found = true;
        }
    }

    // lk.function_tools - tool definitions
    if let Some(content) = extract_json::<JsonValue>(attrs, keys::LK_FUNCTION_TOOLS) {
        tool_definitions.push(RawToolDefinition::from_attr(
            keys::LK_FUNCTION_TOOLS,
            timestamp,
            content,
        ));
        found = true;
    }

    // lk.chat_ctx - chat context (full conversation history)
    if let Some(ctx) = extract_json::<JsonValue>(attrs, keys::LK_CHAT_CTX) {
        let msg = json!({
            "role": "context",
            "type": "chat_history",
            "content": ctx
        });
        messages.push(RawMessage::from_attr(keys::LK_CHAT_CTX, timestamp, msg));
        found = true;
    }

    // lk.user_input or lk.input_text - user input
    let input = attrs
        .get(keys::LK_USER_INPUT)
        .or_else(|| attrs.get(keys::LK_INPUT_TEXT));
    if let Some(text) = input {
        if !text.is_empty() {
            let msg = json!({
                "role": "user",
                "content": text
            });
            let key = if attrs.contains_key(keys::LK_USER_INPUT) {
                keys::LK_USER_INPUT
            } else {
                keys::LK_INPUT_TEXT
            };
            messages.push(RawMessage::from_attr(key, timestamp, msg));
            found = true;
        }
    }

    // lk.function_tool.arguments - tool call input
    if let Some(args) = attrs.get(keys::LK_FUNCTION_TOOL_ARGS) {
        let tool_name = attrs.get(keys::LK_FUNCTION_TOOL_NAME);
        let tool_id = attrs.get(keys::LK_FUNCTION_TOOL_ID);
        let args_val = serde_json::from_str::<JsonValue>(args).unwrap_or(json!(args));

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
            keys::LK_FUNCTION_TOOL_ARGS,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    // lk.function_tool.output - tool result
    if let Some(output) = attrs.get(keys::LK_FUNCTION_TOOL_OUTPUT) {
        let tool_name = attrs.get(keys::LK_FUNCTION_TOOL_NAME);
        let tool_id = attrs.get(keys::LK_FUNCTION_TOOL_ID);
        let is_error = attrs
            .get(keys::LK_FUNCTION_TOOL_IS_ERROR)
            .is_some_and(|v| v == "true");
        let output_val = serde_json::from_str::<JsonValue>(output).unwrap_or(json!(output));

        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("tool"));
        if let Some(name) = tool_name {
            msg.insert("name".to_string(), json!(name));
        }
        if let Some(id) = tool_id {
            msg.insert("tool_call_id".to_string(), json!(id));
        }
        msg.insert("content".to_string(), output_val);
        if is_error {
            msg.insert("is_error".to_string(), json!(true));
        }
        messages.push(RawMessage::from_attr(
            keys::LK_FUNCTION_TOOL_OUTPUT,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    // lk.response.text - assistant response text
    if let Some(text) = attrs.get(keys::LK_RESPONSE_TEXT) {
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("assistant"));
        msg.insert("content".to_string(), json!(text));

        // lk.response.function_calls - tool calls in response
        if let Some(calls) = extract_json::<JsonValue>(attrs, keys::LK_RESPONSE_FUNCTION_CALLS) {
            msg.insert("tool_calls".to_string(), calls);
        }

        messages.push(RawMessage::from_attr(
            keys::LK_RESPONSE_TEXT,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    } else if let Some(calls) = extract_json::<JsonValue>(attrs, keys::LK_RESPONSE_FUNCTION_CALLS) {
        // Response with only function calls (no text)
        let msg = json!({
            "role": "assistant",
            "tool_calls": calls
        });
        messages.push(RawMessage::from_attr(
            keys::LK_RESPONSE_FUNCTION_CALLS,
            timestamp,
            msg,
        ));
        found = true;
    }

    found
}

/// MLflow message extraction
#[cfg(test)]
pub(crate) fn try_mlflow(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // mlflow.spanInputs - JSON string for input
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::MLFLOW_SPAN_INPUTS) {
        messages.push(RawMessage::from_attr(
            keys::MLFLOW_SPAN_INPUTS,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // mlflow.spanOutputs - JSON string for output
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::MLFLOW_SPAN_OUTPUTS) {
        messages.push(RawMessage::from_attr(
            keys::MLFLOW_SPAN_OUTPUTS,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // mlflow.chat.tools - JSON array of tool definitions
    if let Some(content) = extract_json::<JsonValue>(attrs, keys::MLFLOW_CHAT_TOOLS) {
        tool_definitions.push(RawToolDefinition::from_attr(
            keys::MLFLOW_CHAT_TOOLS,
            timestamp,
            content,
        ));
        found = true;
    }

    found
}

/// TraceLoop message extraction
#[cfg(test)]
pub(crate) fn try_traceloop(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // traceloop.entity.input - JSON string
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::TRACELOOP_ENTITY_INPUT) {
        messages.push(RawMessage::from_attr(
            keys::TRACELOOP_ENTITY_INPUT,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // traceloop.entity.output - JSON string
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::TRACELOOP_ENTITY_OUTPUT) {
        messages.push(RawMessage::from_attr(
            keys::TRACELOOP_ENTITY_OUTPUT,
            timestamp,
            parsed,
        ));
        found = true;
    }

    found
}

/// Pydantic AI (via Logfire) message extraction
#[cfg(test)]
pub(crate) fn try_pydantic_ai(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // tool_arguments - tool call input
    if let Some(args) = attrs.get(keys::TOOL_ARGUMENTS) {
        let content = serde_json::from_str::<JsonValue>(args).unwrap_or(json!(args));
        let msg = json!({
            "role": "tool_call",
            "content": content
        });
        messages.push(RawMessage::from_attr(keys::TOOL_ARGUMENTS, timestamp, msg));
        found = true;
    }

    // tool_response - tool call output
    if let Some(response) = attrs.get(keys::TOOL_RESPONSE) {
        let content = serde_json::from_str::<JsonValue>(response).unwrap_or(json!(response));
        let msg = json!({
            "role": "tool",
            "content": content
        });
        messages.push(RawMessage::from_attr(keys::TOOL_RESPONSE, timestamp, msg));
        found = true;
    }

    found
}

#[cfg(test)]
pub(crate) fn try_langsmith(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    // LangSmith detection: must have langsmith.* attributes
    let is_langsmith = attrs.contains_key(keys::LANGSMITH_SPAN_KIND)
        || attrs.contains_key(keys::LANGSMITH_TRACE_SESSION_ID)
        || attrs.contains_key(keys::LANGSMITH_TRACE_NAME)
        || attrs.keys().any(|k| k.starts_with("langsmith."));

    if !is_langsmith {
        return false;
    }

    let mut found = false;

    // LangSmith uses gen_ai.prompt for full input JSON (messages array)
    if let Some(prompt_json) = attrs.get(keys::GEN_AI_PROMPT) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(prompt_json) {
            // Extract messages array from prompt
            if let Some(msgs) = parsed.get("messages").and_then(|m| m.as_array()) {
                for msg in msgs {
                    if msg.get("role").is_some() && msg.get("content").is_some() {
                        messages.push(RawMessage::from_attr(
                            keys::GEN_AI_PROMPT,
                            timestamp,
                            msg.clone(),
                        ));
                        found = true;
                    }
                }
            } else if parsed.get("role").is_some() && parsed.get("content").is_some() {
                // Single message format
                messages.push(RawMessage::from_attr(
                    keys::GEN_AI_PROMPT,
                    timestamp,
                    parsed,
                ));
                found = true;
            }
        }
    }

    // LangSmith uses gen_ai.completion for full output JSON
    if let Some(completion_json) = attrs.get(keys::GEN_AI_COMPLETION) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(completion_json) {
            // Extract message from choices array (OpenAI format)
            if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
                for choice in choices {
                    if let Some(msg) = choice.get("message") {
                        if msg.get("role").is_some() || msg.get("content").is_some() {
                            let mut output_msg = msg.clone();
                            // Add finish_reason if present
                            if let Some(fr) = choice.get("finish_reason") {
                                output_msg["finish_reason"] = fr.clone();
                            }
                            messages.push(RawMessage::from_attr(
                                keys::GEN_AI_COMPLETION,
                                timestamp,
                                output_msg,
                            ));
                            found = true;
                        }
                    }
                }
            } else if parsed.get("role").is_some() || parsed.get("content").is_some() {
                // Direct message format
                messages.push(RawMessage::from_attr(
                    keys::GEN_AI_COMPLETION,
                    timestamp,
                    parsed,
                ));
                found = true;
            }
        }
    }

    found
}

#[cfg(test)]
pub(crate) fn try_langgraph(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    // LangGraph detection: must have langgraph.* attributes or langgraph metadata
    let is_langgraph = attrs.contains_key(keys::LANGGRAPH_NODE)
        || attrs.contains_key(keys::LANGGRAPH_CHECKPOINT_NS)
        || attrs.contains_key(keys::LANGGRAPH_THREAD_ID)
        || attrs
            .get(keys::METADATA)
            .is_some_and(|m| m.contains("langgraph_"));

    if !is_langgraph {
        return false;
    }

    let mut found = false;

    // Try to extract from input.value (node inputs, may contain messages)
    if let Some(input_json) = attrs.get(keys::INPUT_VALUE) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(input_json) {
            if extract_langgraph_messages(messages, &parsed, keys::INPUT_VALUE, timestamp) {
                found = true;
            }
        }
    }

    // Try to extract from output.value (node outputs, may contain messages)
    if let Some(output_json) = attrs.get(keys::OUTPUT_VALUE) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(output_json) {
            if extract_langgraph_messages(messages, &parsed, keys::OUTPUT_VALUE, timestamp) {
                found = true;
            }
        }
    }

    // Try to extract from message attribute (single message)
    if let Some(msg_json) = attrs.get(keys::MESSAGE) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(msg_json) {
            if let Some(normalized) = normalize_langchain_message(&parsed) {
                messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, normalized));
                found = true;
            }
        }
    }

    found
}

/// How deep to look for a `messages` list inside a state object.
///
/// LangGraph state is a dict a graph's nodes write into, so the conversation is not always at the top:
/// `{"state": {"messages": [...]}}` is an ordinary shape and used to yield nothing, because the search
/// looked only at the top level and at direct values. Bounded rather than unbounded: the point is to find a
/// state member, not to trawl a tool's arguments for anything message-shaped.
#[cfg(test)]
const LANGGRAPH_STATE_DEPTH: usize = 4;

/// Extract messages from LangGraph state (handles nested messages in dicts/lists)
#[cfg(test)]
pub(super) fn extract_langgraph_messages(
    messages: &mut Vec<RawMessage>,
    value: &JsonValue,
    source_key: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    extract_langgraph_messages_at(
        messages,
        value,
        source_key,
        timestamp,
        LANGGRAPH_STATE_DEPTH,
    )
}

#[cfg(test)]
pub(super) fn extract_langgraph_messages_at(
    messages: &mut Vec<RawMessage>,
    value: &JsonValue,
    source_key: &str,
    timestamp: DateTime<Utc>,
    depth: usize,
) -> bool {
    let mut found = false;

    // Check if value is a LangChain message type
    if let Some(normalized) = normalize_langchain_message(value) {
        messages.push(RawMessage::from_attr(source_key, timestamp, normalized));
        return true;
    }

    // Check for messages array in state dict
    if let Some(obj) = value.as_object() {
        // Look for "messages" key (common LangGraph pattern)
        if let Some(msgs_value) = obj.get("messages") {
            if let Some(msgs_array) = msgs_value.as_array() {
                for msg in msgs_array {
                    if let Some(normalized) = normalize_langchain_message(msg) {
                        messages.push(RawMessage::from_attr(source_key, timestamp, normalized));
                        found = true;
                    }
                }
            }
        }

        // Also check for direct message values in object
        for (_key, val) in obj {
            if let Some(normalized) = normalize_langchain_message(val) {
                messages.push(RawMessage::from_attr(source_key, timestamp, normalized));
                found = true;
            }
        }

        // The final answer beside the conversation, not inside it.
        //
        // A state object can carry both: CrewAI running through LangGraph writes the history under
        // `messages` and the answer under `raw`. Reading only the list claimed the carrier and left the
        // answer unread by anything - the extractor that knows `raw` never got the chance - so the trace
        // showed the question and no reply. `raw` is this file's own vocabulary for that member; a plain
        // string is the only shape taken, since anything structured is a state member rather than a reply.
        if let Some(raw) = obj.get("raw").and_then(|r| r.as_str())
            && !raw.trim().is_empty()
        {
            messages.push(RawMessage::from_attr(
                source_key,
                timestamp,
                json!({"role": "assistant", "content": raw}),
            ));
            found = true;
        }

        // Nested state: `{"state": {"messages": [...]}}` and the like.
        if depth > 0 {
            for (key, val) in obj {
                if key == "messages" || key == "raw" {
                    continue; // already read above
                }
                if val.is_object() || val.is_array() {
                    found |= extract_langgraph_messages_at(
                        messages,
                        val,
                        source_key,
                        timestamp,
                        depth - 1,
                    );
                }
            }
        }
    }

    // Check if value is an array of messages
    if let Some(arr) = value.as_array() {
        for item in arr {
            if let Some(normalized) = normalize_langchain_message(item) {
                messages.push(RawMessage::from_attr(source_key, timestamp, normalized));
                found = true;
            }
        }
    }

    found
}

/// Normalize LangChain message types to standard SideML format
#[cfg(test)]
pub(super) fn normalize_langchain_message(msg: &JsonValue) -> Option<JsonValue> {
    // Check for LangChain message type discriminator
    let msg_type = msg.get("type").and_then(|t| t.as_str());

    // LangChain messages have lc_type or type field
    let lc_type = msg
        .get("lc")
        .and_then(|lc| lc.get("type"))
        .and_then(|t| t.as_str());

    // Also check kwargs.type for serialized LangChain messages
    let kwargs_type = msg
        .get("kwargs")
        .and_then(|k| k.get("type"))
        .and_then(|t| t.as_str());

    let effective_type = msg_type.or(lc_type).or(kwargs_type);

    match effective_type {
        Some("human") | Some("HumanMessage") => {
            let content = extract_langchain_content(msg)?;
            Some(json!({
                "role": "user",
                "content": content
            }))
        }
        Some("ai") | Some("AIMessage") => {
            let content = extract_langchain_content(msg)?;
            let mut result = json!({
                "role": "assistant",
                "content": content
            });

            // Extract tool_calls if present
            if let Some(tool_calls) = msg
                .get("tool_calls")
                .or_else(|| msg.get("kwargs").and_then(|k| k.get("tool_calls")))
            {
                if tool_calls.as_array().is_some_and(|a| !a.is_empty()) {
                    result["tool_calls"] = tool_calls.clone();
                }
            }

            // Extract additional_kwargs for function calls
            if let Some(additional) = msg
                .get("additional_kwargs")
                .or_else(|| msg.get("kwargs").and_then(|k| k.get("additional_kwargs")))
            {
                if let Some(fc) = additional.get("function_call") {
                    result["function_call"] = fc.clone();
                }
                if let Some(tc) = additional.get("tool_calls") {
                    if result.get("tool_calls").is_none() {
                        result["tool_calls"] = tc.clone();
                    }
                }
            }

            Some(result)
        }
        Some("system") | Some("SystemMessage") => {
            let content = extract_langchain_content(msg)?;
            Some(json!({
                "role": "system",
                "content": content
            }))
        }
        Some("tool") | Some("ToolMessage") => {
            let content = extract_langchain_content(msg)?;
            let mut result = json!({
                "role": "tool",
                "content": content
            });

            // Extract tool_call_id
            if let Some(tool_call_id) = msg
                .get("tool_call_id")
                .or_else(|| msg.get("kwargs").and_then(|k| k.get("tool_call_id")))
                .and_then(|v| v.as_str())
            {
                result["tool_call_id"] = json!(tool_call_id);
            }

            // Extract name
            if let Some(name) = msg
                .get("name")
                .or_else(|| msg.get("kwargs").and_then(|k| k.get("name")))
                .and_then(|v| v.as_str())
            {
                result["name"] = json!(name);
            }

            Some(result)
        }
        Some("function") | Some("FunctionMessage") => {
            let content = extract_langchain_content(msg)?;
            let name = msg
                .get("name")
                .or_else(|| msg.get("kwargs").and_then(|k| k.get("name")))
                .and_then(|v| v.as_str())
                .unwrap_or("function");

            Some(json!({
                "role": "function",
                "name": name,
                "content": content
            }))
        }
        // Check for standard role/content format (already normalized)
        _ => {
            if let Some(role) = msg.get("role").and_then(|r| r.as_str()) {
                if msg.get("content").is_some() {
                    return Some(msg.clone());
                }
                // Has role but missing content - try to extract
                if let Some(content) = extract_langchain_content(msg) {
                    return Some(json!({
                        "role": role,
                        "content": content
                    }));
                }
            }
            None
        }
    }
}

/// Extract content from LangChain message (handles various formats)
#[cfg(test)]
pub(super) fn extract_langchain_content(msg: &JsonValue) -> Option<JsonValue> {
    // Direct content field
    if let Some(content) = msg.get("content") {
        return Some(content.clone());
    }

    // Content in kwargs (serialized LangChain format)
    if let Some(kwargs) = msg.get("kwargs") {
        if let Some(content) = kwargs.get("content") {
            return Some(content.clone());
        }
    }

    // Text field (some LangChain versions)
    if let Some(text) = msg.get("text") {
        return Some(text.clone());
    }

    None
}
