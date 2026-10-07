//! The retired member tables of the query-time expansion and the tool-message categorisation, kept as
//! equivalence oracles for the `message_members` declarations that replaced them: which member holds a
//! streamed reply, a system prompt, one result of a bundle and its call id, and which members mark a block as a
//! tool call or result. `the_declared_members_expand_and_categorise_as_the_tables_they_replace` compares the
//! two over every shape these tables named and the ones where they declined.

use super::*;
use sideseat_ports::types::MessageCategory;

pub(super) fn legacy_expand_bundled_tool_results(raw_messages: &[RawMessage]) -> Vec<Observed> {
    let mut result = Vec::with_capacity(raw_messages.len());

    for (index, raw) in raw_messages.iter().enumerate() {
        let path = PositionPath::root(index);
        // Check source to determine expansion type
        let source_name = match &raw.source {
            MessageSource::Event { name, .. } => Some(name.as_str()),
            MessageSource::Attribute { key, .. } => Some(key.as_str()),
        };

        // Handle message array expansion from known sources
        if source_name.is_some_and(is_expandable_message_array_source) {
            legacy_expand_message_array(&mut result, raw, &path);
            continue;
        }

        // Handle bundled tool results
        let role = raw.content.get("role").and_then(|r| r.as_str());
        let is_tool_result_event = source_name == Some("gen_ai.tool.result");

        if (role == Some("tool") || is_tool_result_event)
            && legacy_expand_bundled_tool_result(&mut result, raw, &path)
        {
            continue;
        }

        // No expansion needed - keep as-is
        result.push((raw.clone(), path));
    }

    result
}

fn legacy_expand_message_array(result: &mut Vec<Observed>, raw: &RawMessage, path: &PositionPath) {
    // Find an array to expand. Only consider nested "content"/"messages" fields
    // if they ARE arrays. A string "content" field is the message's actual content,
    // not a nested message array.
    // The member the array came from is part of the path, so an observation can say it was the third
    // entry of `messages` rather than merely "the third thing here".
    let (array_to_expand, array_key): (Option<&Vec<JsonValue>>, Option<&str>) =
        if let Some(direct) = raw.content.as_array() {
            (Some(direct), None)
        } else if let Some(nested) = raw.content.get("content").and_then(|c| c.as_array()) {
            (Some(nested), Some("content"))
        } else if let Some(messages) = raw.content.get("messages").and_then(|m| m.as_array()) {
            (Some(messages), Some("messages"))
        } else {
            (None, None)
        };
    let array_path = match array_key {
        Some(key) => path.child_key(key),
        None => path.clone(),
    };

    let Some(arr) = array_to_expand else {
        // Single nested message: {message: {role, content, ...}, usage: ...}
        if let Some(msg) = raw
            .content
            .get("message")
            .filter(|m| is_message_like_object(m))
        {
            result.push((
                RawMessage {
                    source: raw.source.clone(),
                    content: msg.clone(),
                },
                path.child_key("message"),
            ));
        // Streaming combined content: {combined_chunk_content: "...", chunk_count: N}
        } else if let Some(text) = raw
            .content
            .get("combined_chunk_content")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
        {
            result.push((
                RawMessage {
                    source: raw.source.clone(),
                    content: json!({"role": "assistant", "content": text}),
                },
                path.child_key("combined_chunk_content"),
            ));
        } else {
            // Not an expandable structure — keep as-is
            result.push((raw.clone(), path.clone()));
        }
        return;
    };

    // Check if this array contains message-like objects
    let has_message_like_items = arr.iter().any(is_message_like_object);

    if !has_message_like_items {
        // Array doesn't contain messages (e.g., content blocks array)
        result.push((raw.clone(), path.clone()));
        return;
    }

    // Anthropic: top-level "system" field is not inside "messages" array.
    // Synthesize a system message before expanding the messages array.
    if let Some(system) = raw.content.get("system") {
        let system_text = if let Some(s) = system.as_str() {
            // Simple string: "system": "You are helpful."
            Some(s.to_string())
        } else if let Some(arr) = system.as_array() {
            // Array of blocks: "system": [{"type": "text", "text": "..."}]
            let texts: Vec<&str> = arr
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        } else {
            None
        };
        if let Some(text) = system_text {
            result.push((
                RawMessage {
                    source: raw.source.clone(),
                    content: json!({"role": "system", "content": text}),
                },
                path.child_key("system"),
            ));
        }
    }

    // `opentelemetry-util-genai` represents a streamed Gemini answer as one output message per
    // chunk. That is transport framing, not a conversation with the assistant taking several
    // turns. It is distinguishable from multiple candidates: every intermediate chunk is
    // text-only and unfinished, while the final chunk alone carries a finish reason.
    //
    // A span covering several model responses - a client that runs the tool loop itself streams
    // one response per round - holds several such streams back to back, each ended by the one
    // chunk with a finish reason. Each stream is combined on its own; finished messages between
    // them, such as a round's function call, stay as they are.
    //
    // Do this at query time so captures already stored with that shape are repaired too. Keep
    // the first item's position because the combined observation starts where the stream starts.
    let source_name = match &raw.source {
        MessageSource::Event { name, .. } => name.as_str(),
        MessageSource::Attribute { key, .. } => key.as_str(),
    };
    if source_name == "gen_ai.output.messages" {
        let runs = streamed_runs(arr);
        if runs.iter().any(|run| run.combined.is_some()) {
            for run in runs {
                match run.combined {
                    Some(combined) => result.push((
                        RawMessage {
                            source: raw.source.clone(),
                            content: combined,
                        },
                        array_path.child_index(run.start),
                    )),
                    None => {
                        let messages = arr.iter().enumerate().take(run.end).skip(run.start);
                        for (position, message) in messages {
                            if is_message_like_object(message) {
                                result.push((
                                    RawMessage {
                                        source: raw.source.clone(),
                                        content: message.clone(),
                                    },
                                    array_path.child_index(position),
                                ));
                            }
                        }
                    }
                }
            }
            return;
        }
    }

    // Expand array into individual messages
    let mut expanded_count = 0;
    let mut skipped_count = 0;

    for (position, item) in arr.iter().enumerate() {
        if is_message_like_object(item) {
            result.push((
                RawMessage {
                    source: raw.source.clone(),
                    content: item.clone(),
                },
                array_path.child_index(position),
            ));
            expanded_count += 1;
        } else {
            skipped_count += 1;
        }
    }

    if skipped_count > 0 {
        tracing::trace!(
            expanded = expanded_count,
            skipped = skipped_count,
            "Expanded message array with some non-message items skipped"
        );
    }
}

fn legacy_expand_bundled_tool_result(
    result: &mut Vec<Observed>,
    raw: &RawMessage,
    path: &PositionPath,
) -> bool {
    // Check if content is nested in "content" field or is a direct array
    let (content_array, is_nested) =
        if let Some(nested) = raw.content.get("content").and_then(|c| c.as_array()) {
            (nested, true)
        } else if let Some(direct) = raw.content.as_array() {
            (direct, false)
        } else {
            return false;
        };

    // Check if this is bundled format: array containing multiple {toolResult: ...} objects
    // Also check for snake_case variant: {tool_result: ...}
    let tool_results: Vec<&JsonValue> = content_array
        .iter()
        .filter(|item| item.get("toolResult").is_some() || item.get("tool_result").is_some())
        .collect();

    // Not bundled or only one toolResult - don't expand
    if tool_results.len() <= 1 {
        return false;
    }

    tracing::trace!(
        count = tool_results.len(),
        "Expanding bundled tool results into separate messages"
    );

    // Split bundled tool results into separate messages. Each result keeps the position it held in
    // the bundle, so two results of two identical calls are two observations rather than one.
    let bundle_path = if is_nested {
        path.child_key("content")
    } else {
        path.clone()
    };
    for (position, &item) in tool_results.iter().enumerate() {
        // Handle both camelCase (Bedrock) and snake_case variants
        let tr = item.get("toolResult").or_else(|| item.get("tool_result"));

        // Create new content structure
        let new_content = if is_nested {
            // Original had nested "content" field - preserve structure
            let mut new_obj = raw.content.clone();
            new_obj["content"] = json!([item.clone()]);
            // Set tool_call_id from this specific toolResult
            if let Some(tr) = tr {
                let id = tr
                    .get("toolUseId")
                    .or_else(|| tr.get("tool_use_id"))
                    .and_then(|v| v.as_str());
                if let Some(id) = id {
                    new_obj["tool_call_id"] = json!(id);
                }
            }
            new_obj
        } else {
            // Original was a direct array - wrap in object
            let mut new_obj = json!({
                "role": "tool",
                "content": [item.clone()]
            });
            // Set tool_call_id from this specific toolResult
            if let Some(tr) = tr {
                let id = tr
                    .get("toolUseId")
                    .or_else(|| tr.get("tool_use_id"))
                    .and_then(|v| v.as_str());
                if let Some(id) = id {
                    new_obj["tool_call_id"] = json!(id);
                }
            }
            new_obj
        };

        result.push((
            RawMessage {
                source: raw.source.clone(),
                content: new_content,
            },
            bundle_path.child_index(position),
        ));
    }

    true
}

pub(super) fn legacy_categorize_tool_message(raw_message: &JsonValue) -> MessageCategory {
    // Check for tool INPUT indicators (assistant calling tools)
    if raw_message.get("tool_calls").is_some() {
        return MessageCategory::GenAIToolInput;
    }

    // Check content blocks for tool_use (input) vs tool_result (output)
    if let Some(content) = raw_message.get("content")
        && let Some(arr) = content.as_array()
    {
        for block in arr {
            // Tool INPUT indicators in content
            if block.get("toolUse").is_some()
                || block.get("functionCall").is_some()
                || block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
            {
                return MessageCategory::GenAIToolInput;
            }
            // Tool OUTPUT indicators in content
            if block.get("toolResult").is_some()
                || block.get("functionResponse").is_some()
                || block.get("type").and_then(|t| t.as_str()) == Some("tool_result")
            {
                return MessageCategory::GenAIToolMessage;
            }
        }
    }

    // Default: tool result/output (role="tool" with content)
    MessageCategory::GenAIToolMessage
}

/// The declared members expand and categorise every shape exactly as the retired tables did - including the
/// shapes where they declined - and the one stated difference is pinned below.
#[test]
fn the_declared_members_expand_and_categorise_as_the_tables_they_replace() {
    use chrono::TimeZone;
    let time = chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let at = |key: &str, content: JsonValue| RawMessage {
        source: MessageSource::Attribute {
            key: key.to_string(),
            time,
        },
        content,
    };
    let on = |name: &str, content: JsonValue| RawMessage {
        source: MessageSource::Event {
            name: name.to_string(),
            time,
        },
        content,
    };
    let array = "gen_ai.input.messages";
    let messages = json!([{"role": "user", "content": "hi"}]);
    let raws: Vec<(&str, RawMessage)> = vec![
        ("a bare message array", at(array, messages.clone())),
        (
            "a system prompt as text beside the array",
            at(array, json!({"system": "be brief", "messages": messages})),
        ),
        (
            "a system prompt as text blocks",
            at(
                array,
                json!({"system": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}], "messages": messages}),
            ),
        ),
        (
            "a system prompt as blocks with no text",
            at(
                array,
                json!({"system": [{"type": "image"}], "messages": messages}),
            ),
        ),
        (
            "a system prompt of another kind",
            at(array, json!({"system": 7, "messages": messages})),
        ),
        (
            "a system prompt beside content that is no message array",
            at(
                array,
                json!({"system": "be brief", "content": [{"type": "text", "text": "x"}]}),
            ),
        ),
        (
            "a streamed reply's combined text",
            at(
                array,
                json!({"combined_chunk_content": "the answer", "chunk_count": 3}),
            ),
        ),
        (
            "an empty combined text",
            at(array, json!({"combined_chunk_content": ""})),
        ),
        (
            "a combined text that is not a string",
            at(array, json!({"combined_chunk_content": ["a"]})),
        ),
        (
            "a single nested message beside a combined text",
            at(
                array,
                json!({"message": {"role": "assistant", "content": "x"}, "combined_chunk_content": "y"}),
            ),
        ),
        (
            "a bundle of two results",
            on(
                "gen_ai.tool.result",
                json!({"content": [{"toolResult": {"toolUseId": "a", "content": []}}, {"toolResult": {"toolUseId": "b"}}]}),
            ),
        ),
        (
            "a bundle as the content list itself",
            on(
                "gen_ai.tool.result",
                json!([{"toolResult": {"toolUseId": "a"}}, {"tool_result": {"tool_use_id": "b"}}]),
            ),
        ),
        (
            "a bundle whose results carry both id spellings, or a non-string one",
            on(
                "gen_ai.tool.result",
                json!({"role": "tool", "content": [{"toolResult": {"toolUseId": "a", "tool_use_id": "z"}}, {"toolResult": {"toolUseId": 5, "tool_use_id": "b"}}, {"toolResult": {}}]}),
            ),
        ),
        (
            "a role-tool message with one result, which is no bundle",
            at(
                "x.results",
                json!({"role": "tool", "content": [{"toolResult": {"toolUseId": "a"}}]}),
            ),
        ),
        (
            "an item holding both spellings of a result",
            at(
                "x.results",
                json!({"role": "tool", "content": [{"toolResult": {"toolUseId": "a"}, "tool_result": {"tool_use_id": "z"}}, {"tool_result": {"tool_use_id": "b"}}]}),
            ),
        ),
        (
            "results that are not bundled members",
            at(
                "x.results",
                json!({"role": "tool", "content": [{"text": "a"}, {"text": "b"}]}),
            ),
        ),
    ];
    let render = |observed: Vec<Observed>| format!("{observed:?}");
    for (what, raw) in &raws {
        assert_eq!(
            render(expand_bundled_tool_results(std::slice::from_ref(raw))),
            render(legacy_expand_bundled_tool_results(std::slice::from_ref(
                raw
            ))),
            "the declared members expand differently: {what}"
        );
    }

    let tool_messages: Vec<(&str, JsonValue)> = vec![
        (
            "tool calls on the message",
            json!({"tool_calls": [], "content": [{"toolResult": {}}]}),
        ),
        ("tool calls that are null", json!({"tool_calls": null})),
        (
            "a call block",
            json!({"content": [{"toolUse": {"name": "f"}}]}),
        ),
        (
            "a Gemini call block",
            json!({"content": [{"functionCall": {"name": "f"}}]}),
        ),
        (
            "a canonical call",
            json!({"content": [{"type": "tool_use", "name": "f"}]}),
        ),
        ("a result block", json!({"content": [{"toolResult": {}}]})),
        (
            "a Gemini result block",
            json!({"content": [{"functionResponse": {"name": "f"}}]}),
        ),
        (
            "a canonical result",
            json!({"content": [{"type": "tool_result"}]}),
        ),
        (
            "a result before a call",
            json!({"content": [{"toolResult": {}}, {"toolUse": {}}]}),
        ),
        (
            "text before a call",
            json!({"content": [{"text": "x"}, {"functionCall": {}}]}),
        ),
        ("plain content", json!({"content": "done"})),
        ("no content", json!({"role": "tool"})),
        (
            "content blocks that are strings",
            json!({"content": ["a", "b"]}),
        ),
        ("a message that is not an object", json!("x")),
    ];
    for (what, message) in &tool_messages {
        assert_eq!(
            super::categorization::categorize_tool_message(message),
            legacy_categorize_tool_message(message),
            "the declared members categorise differently: {what}"
        );
    }

    // The stated difference: a spelling family is one flag vector, so the snake_case spellings of a Gemini call
    // and result say what the camelCase ones always did. The retired table checked only the camelCase.
    for (message, call) in [
        (json!({"content": [{"function_call": {"name": "f"}}]}), true),
        (
            json!({"content": [{"function_response": {"name": "f"}}, {"toolUse": {}}]}),
            false,
        ),
    ] {
        assert_eq!(
            super::categorization::categorize_tool_message(&message),
            if call {
                MessageCategory::GenAIToolInput
            } else {
                MessageCategory::GenAIToolMessage
            }
        );
        assert_ne!(
            super::categorization::categorize_tool_message(&message),
            legacy_categorize_tool_message(&message),
            "the retired table read the snake_case spelling differently"
        );
    }
}
