//! A tool result's identity: every spelling of one returned value hashes alike, and nothing the canonical form
//! keeps apart merges.
//!
//! Consistent by construction rather than by case: every content reduces to one of three things - a **text**
//! (a string, or a list of nothing but plain text parts), a **value** the tool returned (a json part's plain
//! data, a declared wrapper's inner value, a bare structured object, a primitive, a list of such), or a **list of
//! parts** - and each is hashed under its own form. Two spellings agree exactly when they reduce to the same
//! thing, so a returned list is the same list whichever reader wrapped it.

use std::hash::{Hash, Hasher};

use super::hash_json_into;

/// Feed a tool result's canonical form to a hasher, unwrapping the shapes frameworks wrap them in.
///
/// A tool result arrives wrapped differently per framework - a bare string, an array of content
/// blocks, `{"json": ...}`, `{"type": "text", "text": ...}`, `{"type": "json", "data": ...}` - and two
/// wrappings of one answer must hash alike, while two answers must not. The rule is the canonical tool
/// result's (`sideml::content::canonical_single_part`): a part encoding nothing beyond its value is that
/// value, and nothing else is. A one-element list of a bare value is the tool's own list, a json part around
/// block-shaped data is not the block it is spelled like, and a part with citations or any other member says
/// more than its text. The joined text is small by construction: only plain text parts build it, and the
/// megabytes live in image blocks, which take the streaming JSON path.
pub(in crate::sideml::feed) fn hash_tool_result_content_into<H: Hasher>(
    content: &serde_json::Value,
    hasher: &mut H,
) {
    use serde_json::Value as JsonValue;
    match content {
        JsonValue::String(s) => s.trim().hash(hasher),
        JsonValue::Array(parts) => hash_result_parts_into(parts, hasher),
        JsonValue::Object(obj) => {
            // A declared wrapper is a json part by another spelling, so it hashes as the one-part list holding
            // that part: its value where plain, and otherwise the part - a wrapped list of image blocks is
            // returned JSON, not the images. Built only for that rare non-plain value.
            if let Some(inner) = structured_value_under(obj) {
                if crate::sideml::content::is_plain_value(
                    &crate::rules::ruleset().content_blocks,
                    inner,
                ) {
                    return hash_returned_value_into(inner, hasher);
                }
                let part = serde_json::json!({"type": "json", "data": inner});
                return hash_result_parts_into(std::slice::from_ref(&part), hasher);
            }
            if obj.get("type").is_some_and(JsonValue::is_string) {
                return hash_result_parts_into(std::slice::from_ref(content), hasher);
            }
            hash_returned_value_into(content, hasher)
        }
        primitive => hash_returned_value_into(primitive, hasher),
    }
}

/// A list of result parts: texts are their joined text, one json part holding plain data is that value, and any
/// other list is its elements in order. A text is a plain text part or a bare string, which a list's reading makes
/// one - except a list of one bare string, the tool's own list of one.
fn hash_result_parts_into<H: Hasher>(parts: &[serde_json::Value], hasher: &mut H) {
    let tool_listed_string = matches!(parts, [serde_json::Value::String(_)]);
    if !parts.is_empty() && !tool_listed_string && parts.iter().all(|part| text_of(part).is_some())
    {
        let texts: Vec<&str> = parts.iter().filter_map(text_of).map(str::trim).collect();
        return texts.join("\n").trim().hash(hasher);
    }
    if let [only] = parts
        && let Some(value) = json_part_value(only)
    {
        return hash_returned_value_into(value, hasher);
    }
    "tool_result_parts".hash(hasher);
    parts.len().hash(hasher);
    for part in parts {
        if let Some(text) = text_of(part) {
            "text".hash(hasher);
            text.trim().hash(hasher);
        } else {
            hash_returned_value_into(element_value(part).unwrap_or(part), hasher);
        }
    }
}

/// A value the tool returned: a list as the list of parts it reads as, a number, boolean or null as the text it
/// renders as, and anything else as its JSON.
///
/// A scalar is its text because producers write one result both ways: openai-agents' legacy carriers state a
/// tool's output as the text `"49"` on one copy and the number `49` on the other, and only reading them as one
/// answer keeps the copy that carries the call id (`openai-agents/legacy/agent_core`). A json string keeps its
/// JSON form, as it hashed before.
fn hash_returned_value_into<H: Hasher>(value: &serde_json::Value, hasher: &mut H) {
    match value {
        serde_json::Value::Array(items) => hash_result_parts_into(items, hasher),
        serde_json::Value::Number(_) | serde_json::Value::Bool(_) | serde_json::Value::Null => {
            value.to_string().hash(hasher)
        }
        _ => hash_json_into(value, hasher),
    }
}

/// The text of a text part that holds nothing else, or of a bare string.
fn text_of(part: &serde_json::Value) -> Option<&str> {
    if let Some(text) = part.as_str() {
        return Some(text);
    }
    let object = part.as_object()?;
    (object.len() == 2 && object.get("type")?.as_str()? == "text")
        .then(|| object.get("text")?.as_str())
        .flatten()
}

/// The value a json part holding nothing but its data stands for: the data itself, where it is plain - no element of it is something the content
/// chain reads as a block. Data spelled like a block keeps its part, so it never hashes as the block it imitates.
fn json_part_value(part: &serde_json::Value) -> Option<&serde_json::Value> {
    let object = part.as_object()?;
    if object.len() != 2 || object.get("type")?.as_str()? != "json" {
        return None;
    }
    // The data itself, judged as it stands: a declared wrapper inside it, `{"json": X}`, is block-shaped, and the
    // canonical form keeps the part around it - so unwrapping it here would merge it with the bare `X`.
    let data = object.get("data")?;
    crate::sideml::content::is_plain_value(&crate::rules::ruleset().content_blocks, data)
        .then_some(data)
}

/// An element's value in a list of parts: a json part's plain data, or the raw value of an `unknown` part holding
/// nothing else - what a reader makes of a primitive element - where that value is plain.
fn element_value(part: &serde_json::Value) -> Option<&serde_json::Value> {
    if let Some(value) = json_part_value(part) {
        return Some(value);
    }
    let object = part.as_object()?;
    if object.len() != 2 || object.get("type")?.as_str()? != "unknown" {
        return None;
    }
    let raw = object.get("raw")?;
    crate::sideml::content::is_plain_value(&crate::rules::ruleset().content_blocks, raw)
        .then_some(raw)
}

/// The value an object wraps, under a member declared `wraps_structured_value`, whatever sits beside it.
fn structured_value_under(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Value> {
    crate::rules::ruleset()
        .message_members
        .structured_value_wrapper()
        .find_map(|member| object.get(member))
}
