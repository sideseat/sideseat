//! Whether one reconstructed block shows one truth fact.
//!
//! Every predicate reads the block's FULL content. The golden's preview is truncated at 240 characters
//! and whitespace-collapsed, so a comparison against it would bless a defect past the cut or one that
//! only changes spacing.

use serde_json::Value;

use super::recon::Block;
use super::truth::Fact;

/// The `(kind, match)` pairs a predicate exists for. A truth naming another pair is a consistency
/// defect, never a silently unchecked fact.
pub(super) fn supports(kind: &str, matcher: &str) -> bool {
    matches!(
        (kind, matcher),
        ("system", "exact")
            | ("user_text", "contains" | "exact")
            | ("user_media", "digest")
            | ("text", "exact" | "json")
            | ("reasoning", "exact" | "presence" | "signed")
            | ("tool_call", "semantic")
            | ("tool_result", "semantic" | "error_message")
    )
}

/// How a block shows a fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Shows {
    No,
    Yes,
    /// A tool call whose name and arguments match but whose id is the framework's own, carried here.
    WithRewrittenId(String),
    /// A call the wire never gave an id, shown under the one the framework assigned.
    Assigned(String),
}

/// Whether `block` shows `fact`. A tool result is paired by the id its call carries in this view,
/// `call_id`, which is the truth's id unless the framework rewrote it.
pub(super) fn shows(fact: &Fact, block: &Block, call_id: Option<&str>) -> Shows {
    let Some(require) = &fact.require else {
        return Shows::No;
    };
    let yes = |b: bool| if b { Shows::Yes } else { Shows::No };
    match (fact.kind.as_str(), require.matcher.as_str()) {
        ("system", _) => yes(block.is("system", "text") && block.text() == Some(fact.text())),
        ("user_text", "contains") => {
            yes(block.is("user", "text") && block.text().is_some_and(|t| t.contains(fact.text())))
        }
        ("user_text", _) => yes(block.is("user", "text") && block.text() == Some(fact.text())),
        ("user_media", _) => yes(block.role == "user" && media_matches(&fact.value, block)),
        ("text", "json") => yes(block.role == "assistant" && json_answer_matches(fact, block)),
        ("text", _) => yes(block.is("assistant", "text") && block.text() == Some(fact.text())),
        ("reasoning", "presence") => yes(block.is("assistant", "redacted_thinking")),
        // Reasoning the model signed and withheld the text of: a thinking block with no text that says it
        // was signed. Owed by its presence, role and place, since there is no text to compare - and never
        // as `redacted_thinking`, which is the provider's own redaction of a text that existed.
        ("reasoning", "signed") => yes(block.is("assistant", "thinking")
            && block.text().is_some_and(|text| text.trim().is_empty())
            && block.content.get("signed").and_then(Value::as_bool) == Some(true)),
        ("reasoning", _) => {
            yes(block.is("assistant", "thinking") && block.text() == Some(fact.text()))
        }
        ("tool_call", _) => tool_call_shows(&fact.value, block),
        ("tool_result", matcher) => {
            if !block.is_tool_result() {
                return Shows::No;
            }
            // An empty id is a call shown without one, whose result can carry none either.
            let expected_id = call_id
                .or_else(|| fact.value.get("call_id").and_then(Value::as_str))
                .unwrap_or("");
            if block.result_call_id().unwrap_or("") != expected_id {
                return Shows::No;
            }
            // A result pairs by id; its name is a hint a framework may namespace its own way.
            let names_agree = match (
                block.content.get("name").and_then(Value::as_str),
                fact.value.get("name").and_then(Value::as_str),
            ) {
                (Some(shown), Some(named)) => {
                    same_tool(shown, named) || last_segment(shown) == last_segment(named)
                }
                _ => true,
            };
            // A success shown as an error is wrong. An error shown as an ordinary result is what
            // most frameworks send the model - the message as the tool's output - so it is accepted
            // when the message is there.
            let flagged = block
                .content
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let is_error = fact
                .value
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !names_agree || (flagged && !is_error) {
                return Shows::No;
            }
            let expected = &fact.value["value"];
            let content = &block.content["content"];
            yes(if matcher == "error_message" {
                expected
                    .as_str()
                    .is_some_and(|message| rendered_text(content).contains(message))
            } else {
                semantic_eq(content, expected)
            })
        }
        _ => Shows::No,
    }
}

fn tool_call_shows(value: &Value, block: &Block) -> Shows {
    if !block.is("assistant", "tool_use") {
        return Shows::No;
    }
    let name_matches = match (
        block.content.get("name").and_then(Value::as_str),
        value.get("name").and_then(Value::as_str),
    ) {
        (Some(shown), Some(named)) => same_tool(shown, named),
        _ => false,
    };
    let arguments_match = json_eq(&block.content["input"], &value["arguments"]);
    if !name_matches || !arguments_match {
        return Shows::No;
    }
    // A call the framework executed on the model's behalf has no wire id (`null`): the framework names
    // it, and its result pairs by that name, so the id is carried without being a rewrite.
    if value.get("id").is_some_and(Value::is_null) {
        // A wire id the telemetry never carried (`id_not_exported`, proven absent): whatever the
        // reconstruction shows, an empty id included, is the best it can show.
        if value.get("wire_id").is_some_and(Value::is_string) {
            return Shows::Assigned(block.call_id().unwrap_or("").to_string());
        }
        // The framework must name it: without an id its result cannot pair, which is a rewrite to "".
        return match block.call_id().filter(|id| !id.is_empty()) {
            Some(id) => Shows::Assigned(id.to_string()),
            None => Shows::WithRewrittenId(String::new()),
        };
    }
    let expected = value.get("id").and_then(Value::as_str);
    match block.call_id() {
        actual if actual == expected => Shows::Yes,
        Some(actual) => Shows::WithRewrittenId(actual.to_string()),
        None => Shows::WithRewrittenId(String::new()),
    }
}

/// The same tool under a framework's namespace: `travel-get_weather` and `mcp__travel__get_weather` name
/// `get_weather`. Only a whole trailing segment after a separator counts, so `get_weather_v2` does not.
pub(super) fn same_tool(shown: &str, named: &str) -> bool {
    let within = |long: &str, short: &str| {
        long.strip_suffix(short).is_some_and(|prefix| {
            ["-", ".", ":", "/", "__"]
                .iter()
                .any(|separator| prefix.ends_with(separator))
        })
    };
    shown == named || within(shown, named) || within(named, shown)
}

/// A tool name after its namespace: `mcp__travel__get_weather` and `travel-get_weather` end the same.
fn last_segment(name: &str) -> &str {
    ["__", "-", ".", ":", "/"]
        .iter()
        .filter_map(|separator| name.rfind(separator).map(|at| at + separator.len()))
        .max()
        .map_or(name, |at| &name[at..])
}

/// A schema-constrained answer, as a `json` block or as text that parses to the same value.
fn json_answer_matches(fact: &Fact, block: &Block) -> bool {
    let Ok(expected) = serde_json::from_str::<Value>(fact.text()) else {
        return false;
    };
    match block.kind.as_str() {
        "json" => json_eq(&block.content["data"], &expected),
        "text" => block
            .text()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
            .is_some_and(|actual| json_eq(&actual, &expected)),
        _ => false,
    }
}

/// An attachment of the fact's modality, with the same bytes wherever the reconstruction kept them.
///
/// Inline base64 is decoded and digested. Data that does not decode - a URL, a placeholder an
/// instrumentation writes in place of the bytes - can only be checked by modality and media type.
fn media_matches(value: &Value, block: &Block) -> bool {
    if value.get("modality").and_then(Value::as_str) != Some(block.kind.as_str()) {
        return false;
    }
    // The media type is part of the attachment: where the bytes are kept, shown absent is shown
    // wrong; a placeholder for dropped bytes may have lost it with them.
    let shown = block.content.get("media_type").and_then(Value::as_str);
    let stated = value.get("media_type").and_then(Value::as_str);
    if shown != stated && (shown.is_some() || block.media_sha256.is_some()) {
        return false;
    }
    match &block.media_sha256 {
        Some(digest) => value.get("sha256").and_then(Value::as_str) == Some(digest.as_str()),
        None => true,
    }
}

/// JSON equality with numbers compared by value, so `395` equals `395.0`.
pub(super) fn json_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        // Integers exactly; only a pair involving a fraction compares as floating point, so two
        // distinct large integers never collapse onto one double.
        (Value::Number(a), Value::Number(b)) => {
            match (a.as_i64(), b.as_i64(), a.as_u64(), b.as_u64()) {
                (Some(x), Some(y), _, _) => x == y,
                (_, _, Some(x), Some(y)) => x == y,
                _ if a.is_f64() || b.is_f64() => a.as_f64() == b.as_f64(),
                _ => a == b,
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| json_eq(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, x)| b.get(key).is_some_and(|y| json_eq(x, y)))
        }
        _ => left == right,
    }
}

/// Whether a tool result's content carries `expected`, through the encodings frameworks wrap it in.
///
/// Accepted: the value itself; JSON inside a string; a list of `{type: text, text}` parts or one
/// `{type: json, data}` part; and a
/// single-member `result`, `error`, `content` or `output` envelope, recursively. A Python literal
/// (`{'city': 'Paris'}`) is not JSON and is not accepted: it is a rendering defect, not an encoding.
pub(super) fn semantic_eq(content: &Value, expected: &Value) -> bool {
    interpretations(content, 0)
        .iter()
        .any(|candidate| json_eq(candidate, expected))
}

/// Every reading of a tool result's content the encodings above allow, the content itself first.
pub(super) fn interpretations(value: &Value, depth: usize) -> Vec<Value> {
    let mut out = vec![value.clone()];
    if depth > 6 {
        return out;
    }
    match value {
        Value::String(text) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(text)
                && parsed != *value
            {
                out.extend(interpretations(&parsed, depth + 1));
            }
        }
        Value::Array(items) if !items.is_empty() && items.iter().all(is_text_part) => {
            let joined: String = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect();
            out.extend(interpretations(&Value::String(joined), depth + 1));
        }
        // SideML's own structured part: `[{type: json, data}]`.
        Value::Array(items) if items.len() == 1 && is_json_part(&items[0]) => {
            out.extend(interpretations(&items[0]["data"], depth + 1));
        }
        Value::Object(map) if is_text_part(value) => {
            if let Some(text) = map.get("text") {
                out.extend(interpretations(text, depth + 1));
            }
        }
        Value::Object(map) if map.len() == 1 => {
            let (key, inner) = map.iter().next().expect("one member");
            if ["result", "error", "content", "output"].contains(&key.as_str()) {
                out.extend(interpretations(inner, depth + 1));
            }
        }
        _ => {}
    }
    out
}

fn is_json_part(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("json")
        && value.get("data").is_some()
        && value.as_object().is_some_and(|m| m.len() == 2)
}

fn is_text_part(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("text")
        && value.get("text").is_some_and(Value::is_string)
        && value.as_object().is_some_and(|m| m.len() == 2)
}

/// Every string a content value holds, for "contains the error message" checks.
fn rendered_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(rendered_text)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => map
            .values()
            .map(rendered_text)
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}
