use super::*;

#[cfg(test)]
pub(crate) fn try_autogen(
    messages: &mut Vec<RawMessage>,
    // The retired reader took tools from AutoGen's logging channel, read off span attributes no ingest path
    // writes; that reading went with the rules that declared it, and the signature stays the oracles' one.
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // Claim AutoGen OpenInference spans (agent/chain spans with cancellation_token
    // or output_task_messages). These aggregate data from child spans — actual messages
    // come from child `autogen process` spans. The input.value contains Python repr()
    // garbage, so we claim without extracting.
    if let Some(input_val) = attrs.get(keys::INPUT_VALUE) {
        if input_val.contains("cancellation_token") || input_val.contains("output_task_messages") {
            if let Ok(parsed) = serde_json::from_str::<JsonValue>(input_val) {
                if parsed.get("cancellation_token").is_some()
                    || parsed.get("output_task_messages").is_some()
                {
                    found = true;
                }
            }
        }
    }

    // Try to extract from message attribute (AutoGen message types)
    // OpenInference AutoGen uses formats like:
    // - {"messages":[{type:"TextMessage",...},...], "output_task_messages":true}
    // - {"message":{type:"ToolCallRequestEvent",...}}
    if let Some(json_str) = attrs.get(keys::MESSAGE) {
        if json_str != "No Message" && json_str != "{}" {
            if let Ok(parsed) = serde_json::from_str::<JsonValue>(json_str) {
                // Check if this is a typed AutoGen message directly
                let normalized = normalize_autogen_message(&parsed);
                if !normalized.is_empty() {
                    for n in normalized {
                        messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, n));
                    }
                    found = true;
                } else if is_autogen_skip_type(&parsed) {
                    found = true;
                } else if let Some(msgs_array) = parsed.get("messages").and_then(|m| m.as_array()) {
                    for msg in msgs_array {
                        let normalized = normalize_autogen_message(msg);
                        if !normalized.is_empty() {
                            for n in normalized {
                                messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, n));
                            }
                            found = true;
                        } else if is_autogen_skip_type(msg) {
                            found = true;
                        } else if msg.get("content").is_some() {
                            messages.push(RawMessage::from_attr(
                                keys::MESSAGE,
                                timestamp,
                                msg.clone(),
                            ));
                            found = true;
                        }
                    }
                } else if let Some(nested_msg) = parsed.get("message") {
                    let normalized = normalize_autogen_message(nested_msg);
                    if !normalized.is_empty() {
                        for n in normalized {
                            messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, n));
                        }
                        found = true;
                    } else if is_autogen_skip_type(nested_msg) {
                        found = true;
                    } else if nested_msg.get("content").is_some() {
                        messages.push(RawMessage::from_attr(
                            keys::MESSAGE,
                            timestamp,
                            nested_msg.clone(),
                        ));
                        found = true;
                    }
                } else if let Some(response) = parsed.get("response") {
                    // Handle response wrapper: {"response": {"chat_message": {...}, "inner_messages": [...]}}
                    if let Some(chat_msg) = response.get("chat_message") {
                        for n in normalize_autogen_message(chat_msg) {
                            messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, n));
                            found = true;
                        }
                    }
                    if let Some(inner) = response.get("inner_messages").and_then(|m| m.as_array()) {
                        for msg in inner {
                            for n in normalize_autogen_message(msg) {
                                messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, n));
                                found = true;
                            }
                        }
                    }
                } else if parsed.is_object()
                    && (parsed.get("content").is_some() || parsed.get("role").is_some())
                {
                    messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, parsed));
                    found = true;
                }
            }
        }
    }

    found
}

/// AutoGen message types that should be silently skipped (not extracted as messages).
/// ToolCallSummaryMessage concatenates tool results as Python repr() — duplicate noise.
#[cfg(test)]
pub(super) fn is_autogen_skip_type(msg: &JsonValue) -> bool {
    msg.get("type").and_then(|t| t.as_str()) == Some("ToolCallSummaryMessage")
}

/// Determine role from AutoGen source field: "user" → "user", anything else → "assistant"
#[cfg(test)]
pub(super) fn autogen_role_from_source(source: Option<&str>) -> &'static str {
    match source {
        Some("user") => "user",
        _ => "assistant",
    }
}

/// Convert AutoGen tool call array [{id, name, arguments}] to OpenAI-compatible format.
#[cfg(test)]
pub(super) fn normalize_autogen_tool_calls(tool_calls: &[JsonValue]) -> Vec<JsonValue> {
    tool_calls
        .iter()
        .filter_map(|tc| {
            let id = tc.get("id").and_then(|i| i.as_str())?;
            let name = tc.get("name").and_then(|n| n.as_str())?;
            let args = tc.get("arguments").cloned().unwrap_or(json!({}));

            let parsed_args = if let Some(args_str) = args.as_str() {
                serde_json::from_str(args_str).unwrap_or(json!(args_str))
            } else {
                args
            };

            Some(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": parsed_args
                }
            }))
        })
        .collect()
}

/// Convert AutoGen tool result array [{content, name, call_id}] to individual tool messages.
/// Processes ALL items (not just first). Falls back to parent's call_id when item lacks one.
#[cfg(test)]
pub(super) fn normalize_autogen_tool_results(
    items: &[JsonValue],
    parent: &JsonValue,
) -> Vec<JsonValue> {
    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(|n| n.as_str());
            let call_id = item
                .get("call_id")
                .and_then(|c| c.as_str())
                .or_else(|| parent.get("call_id").and_then(|c| c.as_str()));
            let inner_content = item.get("content").cloned().unwrap_or(json!(""));

            // Skip items with no useful content
            if name.is_none() && call_id.is_none() && inner_content == json!("") {
                return None;
            }

            let mut result = json!({"role": "tool", "content": inner_content});
            if let Some(n) = name {
                result["name"] = json!(n);
            }
            if let Some(id) = call_id {
                result["tool_call_id"] = json!(id);
            }
            Some(result)
        })
        .collect()
}

/// Infer AutoGen message type when the `type` field is missing.
/// Uses content structure heuristics to determine the message kind.
#[cfg(test)]
pub(super) fn infer_autogen_message_type(msg: &JsonValue) -> Vec<JsonValue> {
    // Already in SideML format (has role) — preserve as-is
    if msg.get("role").is_some() {
        return vec![msg.clone()];
    }

    let content = match msg.get("content") {
        Some(c) => c,
        None => return vec![],
    };

    // Array content: could be tool calls or tool results
    if let Some(arr) = content.as_array() {
        if arr.is_empty() {
            return vec![];
        }
        let first = &arr[0];

        // Tool call request: [{id, name, arguments}]
        if first.get("arguments").is_some() {
            let normalized_calls = normalize_autogen_tool_calls(arr);
            if normalized_calls.is_empty() {
                return vec![];
            }
            let source = msg.get("source").and_then(|s| s.as_str());
            let mut result = json!({
                "role": "assistant",
                "tool_calls": normalized_calls
            });
            if let Some(src) = source {
                result["name"] = json!(src);
            }
            return vec![result];
        }

        // Tool execution: [{call_id, ...}] or [{content, name}] without arguments
        if first.get("call_id").is_some()
            || (first.get("content").is_some()
                && first.get("name").is_some()
                && first.get("arguments").is_none())
        {
            let results = normalize_autogen_tool_results(arr, msg);
            if results.is_empty() {
                return vec![];
            }
            return results;
        }

        return vec![];
    }

    // String content: TextMessage equivalent
    if let Some(text) = content.as_str() {
        if text.is_empty() {
            return vec![];
        }
        let source = msg.get("source").and_then(|s| s.as_str());
        let role = autogen_role_from_source(source);
        let mut result = json!({
            "role": role,
            "content": text
        });
        if let Some(src) = source {
            if src != "user" {
                result["name"] = json!(src);
            }
        }
        return vec![result];
    }

    vec![]
}

/// Normalize AutoGen message types to standard SideML format.
///
/// Returns a Vec because some message types (ToolCallExecutionEvent,
/// FunctionExecutionResultMessage) contain arrays of results that expand
/// to multiple individual messages.
#[cfg(test)]
pub(super) fn normalize_autogen_message(msg: &JsonValue) -> Vec<JsonValue> {
    let msg_type = match msg.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return infer_autogen_message_type(msg),
    };

    match msg_type {
        "SystemMessage" => match msg.get("content") {
            Some(content) => vec![json!({"role": "system", "content": content})],
            None => vec![],
        },
        "UserMessage" => match msg.get("content") {
            Some(content) => {
                let mut result = json!({"role": "user", "content": content});
                if let Some(source) = msg.get("source") {
                    result["name"] = source.clone();
                }
                vec![result]
            }
            None => vec![],
        },
        "AssistantMessage" => {
            match msg.get("content") {
                Some(content) => {
                    let mut result = json!({"role": "assistant"});
                    if let Some(source) = msg.get("source") {
                        result["name"] = source.clone();
                    }
                    // Prepend thought as thinking content block so SideML can process it
                    let has_thought = msg
                        .get("thought")
                        .map(|t| !t.is_null() && t.as_str().is_none_or(|s| !s.is_empty()))
                        .unwrap_or(false);
                    if has_thought {
                        let thinking_block = json!({
                            "type": "thinking",
                            "text": msg["thought"],
                            "signature": null
                        });
                        result["content"] = json!([thinking_block, content]);
                    } else {
                        result["content"] = content.clone();
                    }
                    vec![result]
                }
                None => vec![],
            }
        }
        "TextMessage" => match msg.get("content") {
            Some(content) => {
                let source = msg.get("source").and_then(|s| s.as_str());
                let role = autogen_role_from_source(source);
                let mut result = json!({"role": role, "content": content});
                if let Some(src) = source {
                    if src != "user" {
                        result["name"] = json!(src);
                    }
                }
                vec![result]
            }
            None => vec![],
        },
        "MultiModalMessage" => match msg.get("content") {
            Some(content) => {
                let source = msg.get("source").and_then(|s| s.as_str());
                let role = autogen_role_from_source(source);
                let mut result = json!({"role": role, "content": content});
                if let Some(src) = source {
                    if src != "user" {
                        result["name"] = json!(src);
                    }
                }
                vec![result]
            }
            None => vec![],
        },
        "StopMessage" => match msg.get("content") {
            Some(content) => {
                let source = msg.get("source").and_then(|s| s.as_str());
                let role = autogen_role_from_source(source);
                let mut result = json!({"role": role, "content": content});
                if let Some(src) = source {
                    if src != "user" {
                        result["name"] = json!(src);
                    }
                }
                vec![result]
            }
            None => vec![],
        },
        "HandoffMessage" => match msg.get("content") {
            Some(content) => {
                let mut result = json!({"role": "assistant", "content": content});
                if let Some(source) = msg.get("source").and_then(|s| s.as_str()) {
                    result["name"] = json!(source);
                }
                vec![result]
            }
            None => vec![],
        },
        "ThoughtEvent" => match msg.get("content") {
            Some(content) => {
                if content.as_str().is_some_and(|s| s.is_empty()) {
                    return vec![];
                }
                vec![json!({
                    "role": "assistant",
                    "content": [{
                        "type": "thinking",
                        "text": content,
                        "signature": null
                    }]
                })]
            }
            None => vec![],
        },
        "ToolCallRequestEvent" => {
            let content = match msg.get("content") {
                Some(c) => c,
                None => return vec![],
            };
            let source = msg.get("source").and_then(|s| s.as_str());

            if let Some(tool_calls) = content.as_array() {
                let normalized_calls = normalize_autogen_tool_calls(tool_calls);
                if !normalized_calls.is_empty() {
                    let mut result = json!({
                        "role": "assistant",
                        "tool_calls": normalized_calls
                    });
                    if let Some(src) = source {
                        result["name"] = json!(src);
                    }
                    return vec![result];
                }
            }
            vec![]
        }
        "ToolCallExecutionEvent" => match msg.get("content").and_then(|c| c.as_array()) {
            Some(arr) if !arr.is_empty() => normalize_autogen_tool_results(arr, msg),
            _ => vec![],
        },
        // ToolCallSummaryMessage is an internal AutoGen mechanism that concatenates
        // str(result) for each tool call. Individual tool results are already captured
        // as proper tool_result entries via ToolCallExecutionEvent/FunctionExecutionResultMessage.
        // Skipping avoids duplicate raw Python repr text in the conversation display.
        "ToolCallSummaryMessage" => vec![],
        "FunctionExecutionResultMessage" => match msg.get("content").and_then(|c| c.as_array()) {
            Some(arr) if !arr.is_empty() => normalize_autogen_tool_results(arr, msg),
            _ => vec![],
        },
        _ => {
            if msg.get("role").is_some() || msg.get("content").is_some() {
                vec![msg.clone()]
            } else {
                vec![]
            }
        }
    }
}

// ============================================================================
#[cfg(test)]
pub(super) fn is_chat_message(msg: &JsonValue) -> bool {
    msg.get("role").is_some()
        && (msg.get("content").is_some()
            || msg.get("tool_calls").is_some()
            || msg.get("toolCalls").is_some())
}

#[cfg(test)]
pub(crate) fn try_crewai(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    // CrewAI detection: must have CrewAI-specific attributes
    let is_crewai = attrs.contains_key("crew_key")
        || attrs.contains_key("crew_id")
        || attrs.contains_key("crew_tasks")
        || attrs.contains_key("task_key");

    if !is_crewai {
        return false;
    }

    let mut found = false;

    // Tool definitions are extracted by extract_tool_definitions() which runs on all spans.

    // crew_tasks attribute (task definitions)
    if let Some(tasks_json) = attrs.get("crew_tasks") {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(tasks_json) {
            messages.push(RawMessage::from_attr("crew_tasks", timestamp, parsed));
            found = true;
        }
    }

    // output.value - try to extract messages array, fall back to raw output
    if let Some(output_json) = attrs.get(keys::OUTPUT_VALUE) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(output_json) {
            let mut extracted_messages = false;

            // Try to extract messages array from the output
            if let Some(msgs) = parsed.get("messages").and_then(|m| m.as_array()) {
                for msg in msgs.iter().filter(|m| is_chat_message(m)) {
                    messages.push(RawMessage::from_attr(
                        keys::OUTPUT_VALUE,
                        timestamp,
                        msg.clone(),
                    ));
                    extracted_messages = true;
                }
            }

            // Also check tasks_output for messages
            if let Some(tasks) = parsed.get("tasks_output").and_then(|t| t.as_array()) {
                for task in tasks {
                    if let Some(msgs) = task.get("messages").and_then(|m| m.as_array()) {
                        for msg in msgs.iter().filter(|m| is_chat_message(m)) {
                            messages.push(RawMessage::from_attr(
                                keys::OUTPUT_VALUE,
                                timestamp,
                                msg.clone(),
                            ));
                            extracted_messages = true;
                        }
                    }
                }
            }

            // The answer itself lives in the top-level `raw`, alongside the history in
            // `messages` - so it is emitted *in addition to* those messages, not as a fallback
            // when they are absent. Treating it as a fallback dropped the assistant output of
            // every CrewAI run that carried history: `crewai/reasoning` recorded
            // system -> user -> user -> user and no answer at all, and the goldens recorded that
            // as correct because no invariant requires an assistant message to exist.
            if let Some(raw) = parsed
                .get("raw")
                .and_then(|r| r.as_str())
                .map(str::trim)
                .filter(|r| !r.is_empty())
            {
                messages.push(RawMessage::from_attr(
                    keys::OUTPUT_VALUE,
                    timestamp,
                    serde_json::json!({"role": "assistant", "content": raw}),
                ));
            } else if !extracted_messages {
                // No messages and no raw answer: keep the payload itself rather than nothing, so
                // the span is not silently empty.
                messages.push(RawMessage::from_attr(keys::OUTPUT_VALUE, timestamp, parsed));
            }
            found = true;
        }
    }

    found
}

/// Prefix of a Claude tool-use id, used to tell an id-tagged `TOOL RESULT` apart from
/// a tool-name-tagged one.
#[cfg(test)]
const CLAUDE_CODE_TOOL_USE_ID_PREFIX: &str = "toolu";

/// Span-name prefix the Claude Code CLI uses for every span it emits.
#[cfg(test)]
const CLAUDE_CODE_SPAN_PREFIX: &str = "claude_code.";

/// Separator between tagged sections inside one `new_context` value. Parallel tool
/// calls put several results in a single attribute.
#[cfg(test)]
const CLAUDE_CODE_SECTION_SEPARATOR: &str = "\n\n---\n\n";

/// Split a Claude Code content attribute into its bracketed tag and body.
///
/// The CLI prefixes these values with a label, e.g. `"[USER PROMPT]\n..."`,
/// `"[TOOL INPUT: Glob]\n{...}"` or `"[TOOL RESULT: toolu_abc]\n..."`. Values with no
/// marker yield `(None, trimmed_value)`.
#[cfg(test)]
pub(super) fn split_bracket_tag(value: &str) -> (Option<&str>, &str) {
    match value
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]\n"))
    {
        Some((tag, body)) => (Some(tag.trim()), body.trim()),
        None => (None, value.trim()),
    }
}

/// Strip a leading `[TAG]\n` marker, keeping only the body.
#[cfg(test)]
pub(super) fn strip_bracket_tag(value: &str) -> &str {
    split_bracket_tag(value).1
}

/// Claude Code CLI (Claude Agent SDK).
///
/// The CLI names its conversation attributes outside the `gen_ai.*` conventions, and
/// emits them only under detailed beta tracing (`ENABLE_BETA_TRACING_DETAILED=1` plus
/// `BETA_TRACING_ENDPOINT`), so no other extractor recognises them:
///
/// | Attribute              | Shape                              |
/// |------------------------|------------------------------------|
/// | `user_system_prompt`   | plain text                         |
/// | `new_context`          | `[USER PROMPT]\n<text>`            |
/// | `response.model_output`| plain text (assistant reply)       |
/// | `tool_input`           | `[TOOL INPUT: <name>]\n<json>`     |
///
/// Gated on the `claude_code.` span-name prefix. `span.type` alone is too generic a
/// name to key on, and without a gate a bare `tool_name` would let this hijack spans
/// from other frameworks.
#[cfg(test)]
pub(crate) fn try_claude_code(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    span_name: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    if !span_name.starts_with(CLAUDE_CODE_SPAN_PREFIX) {
        return false;
    }

    let non_empty = |key: &str| attrs.get(key).filter(|v| !v.trim().is_empty());
    let mut found = false;

    // Caller-supplied system prompt. Emitted once per session, not per request.
    if let Some(system) = non_empty(keys::CLAUDE_CODE_USER_SYSTEM_PROMPT) {
        messages.push(RawMessage::from_attr(
            keys::CLAUDE_CODE_USER_SYSTEM_PROMPT,
            timestamp,
            json!({"role": "system", "content": system}),
        ));
        found = true;
    }

    // new_context carries either side of the conversation, distinguished by its tag:
    //   [USER PROMPT] / [USER]          the user turn
    //   [TOOL RESULT: <tool_use_id>]    the result text the model was shown
    //   [TOOL RESULT: <ToolName>]       the same result as raw structured telemetry
    //
    // Only the id-tagged form is emitted: it is what the model actually saw, and the
    // id pairs it with the tool_use block. The name-tagged duplicate is skipped so the
    // feed does not show every tool result twice; it remains on the tool span itself.
    // A single new_context can hold several tagged sections, one per parallel tool
    // call, joined by CLAUDE_CODE_SECTION_SEPARATOR. Each is parsed independently so
    // every result keeps its own tool_use_id.
    if let Some(context) = non_empty(keys::CLAUDE_CODE_NEW_CONTEXT) {
        for section in context.split(CLAUDE_CODE_SECTION_SEPARATOR) {
            let (tag, body) = split_bracket_tag(section);
            if body.is_empty() {
                continue;
            }
            let tool_result_target = tag
                .and_then(|t| t.strip_prefix("TOOL RESULT:"))
                .map(str::trim);

            // A name-tagged section is the structured duplicate of an id-tagged one, so
            // it is skipped — but only when it really looks like that duplicate (JSON
            // body). If the id prefix ever changes, an unrecognised tag then still
            // reaches the feed unlinked instead of vanishing from it.
            let is_structured_duplicate = |target: &str, body: &str| {
                !target.starts_with(CLAUDE_CODE_TOOL_USE_ID_PREFIX) && body.starts_with('{')
            };

            match tool_result_target {
                Some(target) if !is_structured_duplicate(target, body) => {
                    messages.push(RawMessage::from_attr(
                        keys::CLAUDE_CODE_NEW_CONTEXT,
                        timestamp,
                        json!({
                            "role": "tool",
                            "content": [{
                                "type": "tool_result",
                                "tool_use_id": target,
                                "content": body,
                            }],
                        }),
                    ));
                    found = true;
                }
                // Structured duplicate of a result already emitted above.
                Some(_) => {}
                None => {
                    messages.push(RawMessage::from_attr(
                        keys::CLAUDE_CODE_NEW_CONTEXT,
                        timestamp,
                        json!({"role": "user", "content": body}),
                    ));
                    found = true;
                }
            }
        }
    }

    // Assistant reply text.
    if let Some(output) = non_empty(keys::CLAUDE_CODE_MODEL_OUTPUT) {
        messages.push(RawMessage::from_attr(
            keys::CLAUDE_CODE_MODEL_OUTPUT,
            timestamp,
            json!({"role": "assistant", "content": output}),
        ));
        found = true;
    }

    // Tool call: rebuild an assistant tool_use block from the tool span.
    if let Some(tool_name) = non_empty(keys::CLAUDE_CODE_TOOL_NAME) {
        let input = non_empty(keys::CLAUDE_CODE_TOOL_INPUT)
            .map(|raw| strip_bracket_tag(raw))
            .and_then(|body| serde_json::from_str::<JsonValue>(body).ok())
            .unwrap_or_else(|| json!({}));

        let mut block = serde_json::Map::new();
        block.insert("type".to_string(), json!("tool_use"));
        block.insert("name".to_string(), json!(tool_name));
        block.insert("input".to_string(), input);
        if let Some(id) = non_empty(keys::CLAUDE_CODE_TOOL_USE_ID) {
            block.insert("id".to_string(), json!(id));
        }

        messages.push(RawMessage::from_attr(
            keys::CLAUDE_CODE_TOOL_NAME,
            timestamp,
            json!({"role": "assistant", "content": [JsonValue::Object(block)]}),
        ));
        found = true;
    }

    found
}
