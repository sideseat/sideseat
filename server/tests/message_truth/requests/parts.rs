//! How a recorded request's parts are read against a reconstruction: the role a part is shown under, whether it
//! names a conversation fact, and the id a view shows a rewritten wire id under.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::message_truth::truth::Occurrence;

/// The role a reconstruction shows a part under: a tool's result is the tool's turn, whatever message
/// carried it, and a provider's own names for the assistant and the tool are normalised.
pub(in crate::message_truth) fn shown_role(message_role: &str, part: &Value) -> &'static str {
    if part.get("type").and_then(Value::as_str) == Some("tool_result") {
        return "tool";
    }
    match message_role {
        "system" => "system",
        "user" => "user",
        "assistant" | "model" => "assistant",
        "tool" | "function" => "tool",
        _ => "unknown",
    }
}

/// Whether an occurrence's lineage names a conversation fact, which the fact checks then account for.
pub(super) fn names_fact(occurrence: &Occurrence) -> bool {
    [&occurrence.new_fact, &occurrence.replay_of]
        .into_iter()
        .flatten()
        .any(|named| named.starts_with("fact-"))
}

/// The id a view shows a wire id under, where the framework rewrote it.
pub(super) fn rewritten(id: &Value, rewrites: &BTreeMap<String, String>) -> Value {
    match id.as_str().and_then(|id| rewrites.get(id)) {
        Some(shown) => Value::String(shown.clone()),
        None => id.clone(),
    }
}
