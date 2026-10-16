//! The raw spans a framework text is proven against: every string each holds, and apart from them the strings of
//! the carriers where it states its own output.

use std::path::PathBuf;

use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value::Value as Otlp};
use serde_json::Value;
use sideseat_domain::sideml::ChatRole;

use super::super::truth::hex_digest;

/// One raw span: its id, and every string its attributes and events hold, JSON decoded, each with when it was
/// written - an event's own time, or the span's end for its attributes.
pub(super) struct RawSpan {
    pub trace: String,
    pub id: String,
    pub strings: Vec<(String, u64)>,
    /// The strings of the carriers where the span states its own output under the published conventions - the
    /// output messages and completion attributes, the generic output, and the content members of a choice or of
    /// the inference-details event - and nothing else it holds, such as a diagnostic member beside them. None for a
    /// tool's execution, whose output is the tool's.
    pub outputs: Vec<(String, u64)>,
}

/// The attributes, and the (event, member) pairs, where a span states its own output under the published
/// conventions and the generic input and output convention - an event's in the order a reader takes them.
const OUTPUT_ATTRIBUTES: &[&str] = &[
    "gen_ai.output.messages",
    "gen_ai.completion",
    "output.value",
];
const OUTPUT_EVENT_MEMBERS: &[(&str, &str)] = &[
    ("gen_ai.choice", "content"),
    ("gen_ai.choice", "message"),
    ("gen_ai.content.completion", "content"),
    ("gen_ai.content.completion", "gen_ai.completion"),
    ("gen_ai.content.completion", "message"),
    (
        "gen_ai.client.inference.operation.details",
        "gen_ai.output.messages",
    ),
];

/// Whether an indexed completion's content sits beside a role other than the assistant's
/// (`gen_ai.completion.<n>.role`).
fn indexed_by_another_role(
    attributes: &[opentelemetry_proto::tonic::common::v1::KeyValue],
    key: &str,
) -> bool {
    let Some(prefix) = key
        .strip_suffix(".content")
        .filter(|p| p.starts_with("gen_ai.completion."))
    else {
        return false;
    };
    let role = format!("{prefix}.role");
    attributes.iter().filter(|kv| kv.key == role).any(|kv| {
        !matches!(
            kv.value.as_ref().and_then(|v| v.value.as_ref()),
            Some(Otlp::StringValue(r))
                if !r.eq_ignore_ascii_case("tool_call") && ChatRole::try_from_str(r) == Some(ChatRole::Assistant)
        )
    })
}

/// Whether an attribute is an output carrier: one of `OUTPUT_ATTRIBUTES`, or the content of one indexed completion
/// (`gen_ai.completion.<n>.content`).
fn output_attribute(key: &str) -> bool {
    OUTPUT_ATTRIBUTES.contains(&key)
        || key
            .strip_prefix("gen_ai.completion.")
            .and_then(|rest| rest.strip_suffix(".content"))
            .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
}

pub(super) fn raw_spans(paths: &[PathBuf]) -> Vec<RawSpan> {
    let mut out = Vec::new();
    for path in paths {
        let request = crate::decode_request(path);
        for span in request
            .resource_spans
            .iter()
            .flat_map(|r| &r.scope_spans)
            .flat_map(|s| &s.spans)
        {
            let mut strings = Vec::new();
            let mut outputs = Vec::new();
            for kv in &span.attributes {
                if let Some(value) = &kv.value {
                    let mut found = Vec::new();
                    collect_otlp(value, &mut found);
                    let found: Vec<(String, u64)> = found
                        .into_iter()
                        .map(|s| (s, span.end_time_unix_nano))
                        .collect();
                    if output_attribute(&kv.key)
                        && !indexed_by_another_role(&span.attributes, &kv.key)
                    {
                        outputs.extend(
                            output_strings(value)
                                .into_iter()
                                .map(|s| (s, span.end_time_unix_nano)),
                        );
                    }
                    strings.extend(found);
                }
            }
            for event in &span.events {
                for kv in &event.attributes {
                    if let Some(value) = &kv.value {
                        let mut found = Vec::new();
                        collect_otlp(value, &mut found);
                        strings.extend(found.into_iter().map(|s| (s, event.time_unix_nano)));
                    }
                }
                // The one output member of the event a reader takes, the first its convention lists that is present:
                // a member passed over beside it is not what the event says.
                let taken = OUTPUT_EVENT_MEMBERS
                    .iter()
                    .filter(|(name, _)| *name == event.name)
                    .find_map(|(_, member)| {
                        event
                            .attributes
                            .iter()
                            .find(|kv| kv.key == *member)
                            .and_then(|kv| kv.value.as_ref())
                            // An unset value states nothing: the reader passes over it.
                            .filter(|value| value.value.is_some())
                    });
                if let Some(value) = taken {
                    outputs.extend(
                        output_strings(value)
                            .into_iter()
                            .map(|s| (s, event.time_unix_nano)),
                    );
                }
            }
            // A tool's execution states the tool's output, not an assistant's: the conventions' own tool-execution
            // signals (`semconv.tool_execution`) - the operation, or a tool name with a call id.
            let named = |key: &str| {
                span.attributes
                    .iter()
                    .find(|kv| kv.key == key)
                    .and_then(|kv| match kv.value.as_ref()?.value.as_ref()? {
                        Otlp::StringValue(s) => Some(s.as_str()),
                        _ => None,
                    })
            };
            if named("gen_ai.operation.name") == Some("execute_tool")
                || (named("gen_ai.tool.name").is_some() && named("gen_ai.tool.call.id").is_some())
            {
                outputs.clear();
            }
            out.push(RawSpan {
                trace: hex_digest(&span.trace_id),
                id: hex_digest(&span.span_id),
                strings,
                outputs,
            });
        }
    }
    out
}

/// What an output carrier states as content: the whole string the producer wrote, and, where that string is a JSON
/// message or message list, the strings under its content members (`text`, `content`) - never another member beside
/// them, such as a diagnostic one, nor anything inside a message stating a role other than the assistant's.
fn output_strings(value: &AnyValue) -> Vec<String> {
    // A message's role compared as the readers normalise it - except `tool_call`, which they read as a call, its
    // content the call's arguments.
    fn assistant(role: &str) -> bool {
        !role.eq_ignore_ascii_case("tool_call")
            && ChatRole::try_from_str(role) == Some(ChatRole::Assistant)
    }
    // A JSON message or part list: each element read as a message, a part, or - in a list of parts - a bare string
    // the readers take as text; nothing else is content.
    fn content(value: &Value, in_parts: bool, out: &mut Vec<String>) {
        match value {
            Value::String(s) if in_parts => out.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|v| content(v, in_parts, out)),
            Value::Object(members) => {
                let message = members.contains_key("role");
                // Not another role's message, nor a part of another kind than text. A part the reasoning wrapper
                // claims - `reasoning` or `reasoning_content` present at all, as the wrapper tests them - or one a
                // set `thought` flag marks is thinking; a message envelope (`type: message`) is no part, and a
                // message's own `thought` beside its content says nothing of the content.
                if members
                    .get("role")
                    .and_then(Value::as_str)
                    .is_some_and(|role| !assistant(role))
                    || members
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| !matches!(kind, "text" | "output_text" | "message"))
                    || (!message
                        && (["reasoning", "reasoning_content"]
                            .iter()
                            .any(|key| members.contains_key(*key))
                            || members
                                .get("thought")
                                .is_some_and(|flag| *flag == Value::Bool(true))))
                {
                    return;
                }
                // The one content member a reader takes, in its precedence - a message's as the message-members
                // vocabulary orders them, a part's text before a content beside it - and nothing it passes over. A
                // text counts only as a string; a content or parts member as a string or a list of parts.
                let precedence: &[&str] = if message {
                    &["contents", "content", "message", "parts", "text"]
                } else {
                    &["text", "content", "parts", "message"]
                };
                match precedence
                    .iter()
                    .find_map(|key| members.get(*key).map(|member| (*key, member)))
                {
                    Some((_, Value::String(s))) => out.push(s.clone()),
                    Some((key, parts @ Value::Array(_))) if key != "text" => {
                        content(parts, true, out)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    match &value.value {
        Some(Otlp::StringValue(s)) => {
            let mut out = vec![s.clone()];
            if let Ok(parsed) = serde_json::from_str::<Value>(s)
                && (parsed.is_object() || parsed.is_array())
            {
                content(&parsed, parsed.is_array(), &mut out);
            }
            out
        }
        // A message written as a native OTLP map or list, read as the JSON it would be.
        Some(Otlp::KvlistValue(_) | Otlp::ArrayValue(_)) => {
            let mut out = Vec::new();
            let parsed = otlp_json(value);
            content(&parsed, parsed.is_array(), &mut out);
            out
        }
        _ => Vec::new(),
    }
}

/// A native OTLP value as the JSON value it states: maps as objects, lists as arrays, scalars as themselves.
fn otlp_json(value: &AnyValue) -> Value {
    match &value.value {
        Some(Otlp::StringValue(s)) => Value::String(s.clone()),
        Some(Otlp::BoolValue(b)) => Value::Bool(*b),
        Some(Otlp::IntValue(i)) => Value::from(*i),
        Some(Otlp::DoubleValue(d)) => Value::from(*d),
        Some(Otlp::ArrayValue(items)) => Value::Array(items.values.iter().map(otlp_json).collect()),
        Some(Otlp::KvlistValue(list)) => Value::Object(
            list.values
                .iter()
                .filter_map(|kv| {
                    let value = kv.value.as_ref().filter(|v| v.value.is_some())?;
                    Some((kv.key.clone(), otlp_json(value)))
                })
                .collect(),
        ),
        _ => Value::Null,
    }
}

pub(super) fn collect_otlp(value: &AnyValue, out: &mut Vec<String>) {
    match &value.value {
        Some(Otlp::StringValue(s)) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(s)
                && (parsed.is_object() || parsed.is_array())
            {
                collect_json(&parsed, out);
            }
            out.push(s.clone());
        }
        Some(Otlp::ArrayValue(items)) => items.values.iter().for_each(|v| collect_otlp(v, out)),
        Some(Otlp::KvlistValue(list)) => list
            .values
            .iter()
            .filter_map(|kv| kv.value.as_ref())
            .for_each(|v| collect_otlp(v, out)),
        _ => {}
    }
}

fn collect_json(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| collect_json(v, out)),
        Value::Object(members) => members.values().for_each(|v| collect_json(v, out)),
        _ => {}
    }
}
