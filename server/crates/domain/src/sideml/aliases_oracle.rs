//! The retired member tables of SideML normalisation, kept as equivalence oracles for the `message_members`
//! declarations that replaced them: the spellings of SideSeat's own members (`alias_of`), the call wrapper, the
//! context members, and the call id inside a message's first block. Compared by
//! `the_declared_spellings_normalise_as_the_tables_they_replace`.

use serde_json::{Value as JsonValue, json};

use super::ChatRole;
use super::types::ContentBlock;

fn json_value_to_string(value: &JsonValue) -> String {
    match value.as_str() {
        Some(s) => s.to_string(),
        None => value.to_string(),
    }
}

fn has_meaningful_data(val: &JsonValue) -> bool {
    super::has_meaningful_data_for_oracle(val)
}

pub(super) fn legacy_normalize_tool_calls(msg: &JsonValue) -> Option<JsonValue> {
    let tool_calls = msg.get("tool_calls")?.as_array()?;

    let normalized: Vec<JsonValue> = tool_calls
        .iter()
        .filter_map(|tc| {
            // OpenInference unflattened format: {tool_call: {function: {name, arguments}, id}}
            let tc = if let Some(inner) = tc.get("tool_call") {
                inner
            } else {
                tc
            };

            // Extract name and arguments from various formats
            let (name, arguments, id) = if let Some(func) = tc.get("function") {
                // OpenAI nested format: {function: {name, arguments}}
                let args = func
                    .get("arguments")
                    .or_else(|| func.get("args"))
                    .map(json_value_to_string)
                    .unwrap_or_default();
                (func.get("name")?.as_str()?.to_string(), args, tc.get("id"))
            } else {
                // Already flat format (includes LangChain which uses "args")
                let args = tc
                    .get("arguments")
                    .or_else(|| tc.get("args"))
                    .map(json_value_to_string)
                    .unwrap_or_default();
                (tc.get("name")?.as_str()?.to_string(), args, tc.get("id"))
            };

            Some(json!({
                "id": id,
                "type": "function",
                "name": name,
                "arguments": arguments
            }))
        })
        .collect();

    if normalized.is_empty() {
        None
    } else {
        Some(json!(normalized))
    }
}

pub(super) fn legacy_extract_tool_use_id(msg: &JsonValue, role: &str) -> Option<String> {
    // Standard tool_call_id field (OpenAI format, any role)
    if let Some(id) = msg.get("tool_call_id").and_then(|i| i.as_str()) {
        return Some(id.to_string());
    }

    // Anthropic tool_use_id field (any role)
    if let Some(id) = msg.get("tool_use_id").and_then(|i| i.as_str()) {
        return Some(id.to_string());
    }

    // Generic id/call_id fields - only for tool/function roles to avoid
    // extracting message IDs from assistant/user messages
    if ChatRole::is_tool_role(role) {
        if let Some(id) = msg.get("id").and_then(|i| i.as_str()) {
            return Some(id.to_string());
        }
        if let Some(id) = msg.get("call_id").and_then(|i| i.as_str()) {
            return Some(id.to_string());
        }
    }

    // Nested in content (Bedrock/Strands format)
    if let Some(content) = msg.get("content").and_then(|c| c.as_array())
        && let Some(first) = content.first()
    {
        // toolResult.toolUseId
        if let Some(id) = first
            .get("toolResult")
            .and_then(|tr| tr.get("toolUseId"))
            .and_then(|id| id.as_str())
        {
            return Some(id.to_string());
        }
        // toolUse.toolUseId
        if let Some(id) = first
            .get("toolUse")
            .and_then(|tu| tu.get("toolUseId"))
            .and_then(|id| id.as_str())
        {
            return Some(id.to_string());
        }
    }

    None
}

pub(super) fn legacy_unwrap_choice_envelope(raw: JsonValue) -> JsonValue {
    // A role may already be on the envelope: the event's name implies one, and it is assigned before this.
    let is_envelope = ["content", "contents", "parts", "tool_calls"]
        .iter()
        .all(|member| raw.get(member).is_none());
    let Some(inner) = raw
        .get("message")
        .and_then(JsonValue::as_object)
        .filter(|inner| inner.contains_key("content") || inner.contains_key("tool_calls"))
    else {
        return raw;
    };
    if !is_envelope {
        return raw;
    }
    let mut message = inner.clone();
    if let Some(reason) = raw.get("finish_reason")
        && !message.contains_key("finish_reason")
    {
        message.insert("finish_reason".to_string(), reason.clone());
    }
    JsonValue::Object(message)
}
const CITATION_CONTEXT_FIELDS: &[(&str, &str)] = &[
    ("groundingMetadata", "grounding"),
    ("citationMetadata", "citations"),
    ("data_sources", "data_sources"),
    ("search_results", "search_results"),
    ("citations", "citations"),
    ("attributions", "attributions"),
];
pub(super) fn legacy_extract_citation_contexts(raw: &JsonValue) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();

    for (field, context_type) in CITATION_CONTEXT_FIELDS {
        if let Some(data) = raw.get(*field).filter(|v| has_meaningful_data(v)) {
            blocks.push(ContentBlock::Context {
                data: data.clone(),
                context_type: Some(context_type.to_string()),
            });
        }
    }

    if let Some(context) = raw.get("context").filter(|v| v.is_object()) {
        if let Some(citations) = context.get("citations").filter(|v| has_meaningful_data(v)) {
            blocks.push(ContentBlock::Context {
                data: citations.clone(),
                context_type: Some("citations".to_string()),
            });
        }
        if let Some(obj) = context.as_object() {
            let other: serde_json::Map<String, JsonValue> = obj
                .iter()
                .filter(|(k, v)| *k != "citations" && has_meaningful_data(v))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !other.is_empty() {
                blocks.push(ContentBlock::Context {
                    data: JsonValue::Object(other),
                    context_type: Some("azure_context".to_string()),
                });
            }
        }
    }

    blocks
}

/// The retired role inference: `tool_calls` or `toolCalls` present means the assistant.
pub(super) fn legacy_inferred_role(raw: &JsonValue) -> &str {
    raw.get("role").and_then(|r| r.as_str()).unwrap_or_else(|| {
        if raw.get("tool_calls").is_some() || raw.get("toolCalls").is_some() {
            "assistant"
        } else {
            "user"
        }
    })
}

/// The retired finish-reason and stop-sequence spellings.
pub(super) fn legacy_finish_and_stop(raw: &JsonValue) -> (Option<&JsonValue>, Option<&JsonValue>) {
    (
        raw.get("finish_reason").or_else(|| raw.get("finishReason")),
        raw.get("stop").or_else(|| raw.get("stop_sequences")),
    )
}

#[test]
fn the_declared_spellings_normalise_as_the_tables_they_replace() {
    let messages = [
        json!({"role": "assistant", "content": "x"}),
        json!({"tool_calls": [{"id": "c", "function": {"name": "f", "arguments": "{}"}}]}),
        json!({"toolCalls": [{"toolCallId": "c", "toolName": "f", "args": {}}]}),
        json!({"tool_calls": [{"tool_call": {"id": "c", "function": {"name": "f", "arguments": {"a": 1}}}}]}),
        json!({"tool_calls": [{"id": "c", "name": "f", "args": {"a": 1}}]}),
        json!({"tool_calls": [{"id": "c", "function": {"name": "f", "args": "x"}}, {"function": {"arguments": 1}}]}),
        json!({"tool_calls": [{"id": "c", "name": "f", "arguments": "a", "args": "b"}]}),
        json!({"tool_calls": "not a list"}),
        json!({"finish_reason": "stop", "finishReason": "length", "stop": ["x"], "stop_sequences": ["y"]}),
        json!({"finishReason": "STOP", "stop_sequences": "END"}),
        json!({"role": "tool", "id": "a", "call_id": "b"}),
        json!({"role": "tool", "id": 5, "call_id": "b"}),
        json!({"role": "function", "call_id": "b"}),
        json!({"role": "user", "id": "a"}),
        json!({"role": "tool", "tool_call_id": "t", "tool_use_id": "u"}),
        json!({"role": "tool", "content": [{"toolResult": {"toolUseId": "r"}}]}),
        json!({"role": "user", "content": [{"toolUse": {"toolUseId": "u", "name": "f"}}]}),
        json!({"role": "user", "content": [{"toolResult": {}, "toolUse": {"toolUseId": "u"}}]}),
        json!({"role": "user", "content": [{"text": "x"}, {"toolResult": {"toolUseId": "r"}}]}),
        json!({"role": "user", "content": [{"toolResult": {"toolUseId": 5}}]}),
        json!({"groundingMetadata": {"chunks": [1]}, "citationMetadata": {}, "citations": ["a"], "attributions": [0], "data_sources": [{"x": 1}], "search_results": "r"}),
        json!({"context": {"citations": [{"url": "u"}], "intent": "i", "empty": ""}}),
        json!({"context": {"citations": []}}),
        json!({"context": "a string"}),
        json!({"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": "x"}}),
        json!({"index": 0, "message": {"role": "assistant", "tool_calls": []}, "contents": []}),
        json!({"message": {"role": "assistant", "content": "x"}, "parts": []}),
        json!({"message": {"role": "assistant", "content": "x"}, "toolCalls": []}),
        json!({"message": {"role": "assistant"}}),
    ];
    for raw in &messages {
        let role = raw.get("role").and_then(|r| r.as_str()).unwrap_or("tool");
        assert_eq!(
            super::tools::normalize_tool_calls(raw),
            legacy_normalize_tool_calls(raw),
            "tool calls: {raw}"
        );
        assert_eq!(
            super::tools::extract_tool_use_id(raw, role),
            legacy_extract_tool_use_id(raw, role),
            "call id: {raw}"
        );
        assert_eq!(
            format!("{:?}", super::extract_citation_contexts(raw)),
            format!("{:?}", legacy_extract_citation_contexts(raw)),
            "contexts: {raw}"
        );
        assert_eq!(
            super::unwrap_choice_envelope(raw.clone()),
            legacy_unwrap_choice_envelope(raw.clone()),
            "choice envelope: {raw}"
        );
        let normalized = super::normalize(raw);
        let (finish, stop) = legacy_finish_and_stop(raw);
        let legacy_finish = finish
            .and_then(|f| f.as_str())
            .and_then(super::FinishReason::from_str_normalized);
        assert_eq!(normalized.finish_reason, legacy_finish, "finish: {raw}");
        let legacy_stop = stop.and_then(|s| {
            if let Some(items) = s.as_array() {
                Some(
                    items
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>(),
                )
            } else {
                s.as_str().map(|s| vec![s.to_string()])
            }
        });
        assert_eq!(normalized.stop, legacy_stop, "stop: {raw}");
        if raw.get("role").is_none() && raw.get("message").is_none() {
            assert_eq!(
                normalized.role,
                ChatRole::from_str_normalized(legacy_inferred_role(raw)),
                "role: {raw}"
            );
        }
    }

    // The stated differences. A call id inside a first block is read under every declared spelling of the
    // id, and the block may be any declared result or call; and an envelope holding prose of its own beside
    // its message is no envelope.
    let snake = json!({"role": "user", "content": [{"toolResult": {"tool_use_id": "r"}}]});
    assert_eq!(legacy_extract_tool_use_id(&snake, "user"), None);
    assert_eq!(
        super::tools::extract_tool_use_id(&snake, "user").as_deref(),
        Some("r")
    );
    // Precedence is declared, not the payload's member order: a result before a call, whichever comes first.
    for block in [
        json!({"functionResponse": {"toolUseId": "f"}, "toolResult": {"toolUseId": "r"}}),
        json!({"toolResult": {"toolUseId": "r"}, "functionResponse": {"toolUseId": "f"}}),
        json!({"toolUse": {"toolUseId": "u"}, "toolResult": {"toolUseId": "r"}}),
    ] {
        let message = json!({"role": "user", "content": [block]});
        assert_eq!(
            super::tools::extract_tool_use_id(&message, "user").as_deref(),
            Some("r"),
            "{message}"
        );
        assert_eq!(
            legacy_extract_tool_use_id(&message, "user").as_deref(),
            Some("r")
        );
    }
    // First present, as everywhere a member is read: a preferred id that is not a string hides the other.
    let masked = json!({"role": "user", "content": [{"toolResult": {"toolUseId": null, "tool_use_id": "r"}}]});
    assert_eq!(super::tools::extract_tool_use_id(&masked, "user"), None);
    let prose_beside = json!({"message": {"role": "assistant", "content": "x"}, "text": "y"});
    assert_ne!(
        super::unwrap_choice_envelope(prose_beside.clone()),
        legacy_unwrap_choice_envelope(prose_beside)
    );
}
