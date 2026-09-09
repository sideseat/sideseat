//! Tool definitions written as a language's `repr`, rather than as JSON.
//!
//! A framework that builds its tools as objects and logs them with `str()` leaves a string that is not
//! JSON and is not prose either - `CrewStructuredTool(name='search', description='Tool Arguments: {...}')`.
//! The grammar of that string is a property of the *language*, so it is sealed here and reached only
//! through a declared spec: which member of a carrier holds the tools, which repr fields name them, which
//! labels the embedded documentation uses, and how the language's type names map to JSON Schema's.
//!
//! Nothing here names a framework. What a particular one calls its members and labels is rule data
//! (`ToolReprSpec`), because those are its vocabulary and not a fact about Python.

use std::collections::HashMap;

use serde_json::{Value as JsonValue, json};

use crate::domain::sideml::tools::tool_definition_quality;

use super::message_rules::query;
use super::schema::ToolReprSpec;

/// Every tool definition a carrier holds, best copy per name, in the order the names first appeared.
///
/// One entry of the carrier at a time, and within an entry the declared paths in order - so two entries
/// each carrying a list interleave as the payload has them rather than by path, which is what keeps the
/// reported order the order the framework wrote.
pub(super) fn tools_from_carrier(
    parsed: &JsonValue,
    spec: &ToolReprSpec,
) -> Option<Vec<JsonValue>> {
    let mut best_by_name: HashMap<String, (i32, JsonValue)> = HashMap::new();
    let mut order = Vec::new();
    for entry in query(parsed, &spec.entries) {
        for path in &spec.candidates {
            for candidate in query(entry, path) {
                if let Some(tool) = tool_from_value(candidate, spec) {
                    upsert_best(tool, &mut best_by_name, &mut order);
                }
            }
        }
    }
    in_first_seen_order(best_by_name, order)
}

/// One tool definition from a `repr` string.
fn tool_from_repr(tool_repr: &str, spec: &ToolReprSpec) -> Option<JsonValue> {
    let fallback_name = repr_field(tool_repr, &spec.name_field)
        .or_else(|| labelled_value(tool_repr, &spec.name_label))
        .or_else(|| {
            let trimmed = tool_repr.trim();
            if !trimmed.is_empty() && !trimmed.contains('=') && !trimmed.contains(' ') {
                Some(trimmed.to_string())
            } else {
                None
            }
        })?;

    // Prefer the explicit description field when present; otherwise parse the
    // whole repr to catch labels in non-standard shapes.
    let details =
        loosely_quoted_repr_field(tool_repr, &spec.description_field, &spec.field_terminators)
            .unwrap_or_else(|| tool_repr.into());
    let (name, description, parameters) = labelled_details(&details, &fallback_name, spec);

    let mut function = json!({ "name": name });
    if let Some(desc) = description {
        function["description"] = json!(desc);
    }
    if let Some(params) = parameters {
        function["parameters"] = params;
    }

    Some(json!({
        "type": "function",
        "function": function
    }))
}

/// The details a framework embeds inside its documentation member, by label.
fn labelled_details(
    details: &str,
    fallback_name: &str,
    spec: &ToolReprSpec,
) -> (String, Option<String>, Option<JsonValue>) {
    // A payload may escape newlines inside a repr string.
    // Normalize to real newlines so label extraction is stable.
    let normalized = details.replace("\\n", "\n");

    let name = labelled_value(&normalized, &spec.name_label)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback_name.to_string());

    let description =
        labelled_value(&normalized, &spec.description_label).filter(|v| !v.is_empty());

    let parameters = labelled_python_dict(&normalized, &spec.arguments_label)
        .and_then(python_literal_to_json)
        .and_then(|value| python_args_to_json_schema(&value, spec));

    (name, description, parameters)
}

/// Whether a match of `field=` at `at` is the field itself rather than the tail of a longer identifier.
///
/// `name=` matches inside `username=`, and a plain substring search therefore read `username='admin'` as the
/// tool's `name` - inventing a tool called `admin` out of a constructor that never named one. An identifier
/// character immediately before the match means the match is a suffix of something else.
fn at_identifier_boundary(input: &str, at: usize) -> bool {
    input[..at]
        .chars()
        .next_back()
        .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'))
}

/// The first index at which `needle` occurs on an identifier boundary.
fn find_field(input: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = input[from..].find(needle) {
        let at = from + offset;
        if at_identifier_boundary(input, at) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// Extract `field='...'` or `field="..."` from repr-like strings.
fn repr_field(input: &str, field: &str) -> Option<String> {
    for quote in ['\'', '"'] {
        let prefix = format!("{field}={quote}");
        if let Some(start) = find_field(input, &prefix) {
            let rest = &input[start + prefix.len()..];
            let mut escaped = false;
            for (idx, ch) in rest.char_indices() {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == quote {
                    return Some(rest[..idx].to_string());
                }
            }
        }
    }
    None
}

/// A repr field whose single-quoted value may contain unescaped single quotes.
///
/// A Python dict repr inside the documentation does exactly that, so the closing quote cannot be found
/// by scanning - the value runs to the `')` that closes the constructor.
fn loosely_quoted_repr_field(input: &str, field: &str, terminators: &[String]) -> Option<String> {
    let opening = format!("{field}='");
    if let Some(start) = find_field(input, opening.as_str()) {
        let rest = &input[start + opening.len()..];
        // **The earliest boundary wins**, over every candidate at once. Two defects came from taking them in
        // turn: `rfind("')")` looked for the *last* constructor close, so a quoted field after this one
        // (`env_vars='SECRET')`) put its own closing quote at the end and the description swallowed
        // `env_vars='SECRET`; and the declared terminators were tried in *declaration* order, so which one
        // applied depended on how the asset listed them rather than on where the value actually ends.
        let mut boundary: Option<usize> = None;
        let mut consider = |end: Option<usize>| {
            if let Some(end) = end {
                boundary = Some(boundary.map_or(end, |best: usize| best.min(end)));
            }
        };
        // A constructor repr closes with `')`, whatever the language put inside it - the **first** such close
        // past this field, since a later field's quote makes another.
        consider(rest.find("')"));
        // And the value ends where the next declared field begins. Which fields those are is the framework's own
        // vocabulary, so they are declared - the two that used to be written here were one framework's, sitting
        // in a module that claims to name none.
        for terminator in terminators {
            for needle in [format!("' {terminator}="), format!("', {terminator}=")] {
                consider(rest.find(needle.as_str()));
            }
        }
        // Nothing closes it: the value is the remainder.
        return Some(rest[..boundary.unwrap_or(rest.len())].to_string());
    }
    // The strictly-quoted spelling of the same field, not a fixed member name.
    repr_field(input, field)
}

/// Extract one-line value from `Label: value` pattern.
fn labelled_value(input: &str, label: &str) -> Option<String> {
    input.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix(label)
            .map(|v| v.trim().to_string())
    })
}

/// Extract balanced Python dict text after a label.
fn labelled_python_dict<'a>(input: &'a str, label: &str) -> Option<&'a str> {
    let idx = input.find(label)?;
    let rest = &input[idx + label.len()..];
    balanced_braces(rest)
}

/// Extract first balanced `{...}` block, honoring quoted strings.
fn balanced_braces(input: &str) -> Option<&str> {
    let mut start: Option<usize> = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;

    for (idx, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                in_string = true;
                quote = ch;
            }
            '{' => {
                if start.is_none() {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let s = start?;
                    return Some(&input[s..=idx]);
                }
            }
            _ => {}
        }
    }

    None
}

/// Parse Python repr JSON-like strings (`{'k': True, 'v': None}`) into JSON.
///
/// **The accepted language, specified rather than described**, because "Python repr" names a language far
/// larger than this and a rule author needs to know where the edge is. It is JSON with four differences:
///
/// | Accepted | Meaning |
/// | --- | --- |
/// | A leading `{` or `[` | Required. Anything else is refused outright, so a bare word or a sentence is not a literal |
/// | `'…'` as well as `"…"` | Either quote opens a string, and the closing quote must be the same character |
/// | `True`, `False`, `None` | Outside a string only, and only as whole words - `Nonetheless` is not `null` |
/// | Everything else | Passed through unchanged, so numbers, nesting and separators are JSON's |
///
/// So the whole of it is: reject anything not object- or array-shaped, rewrite quotes and those three words,
/// and hand the rest to a JSON parser. **Not** accepted, each a real Python literal, and each refused
/// because what comes out is not JSON: a tuple `(1, 2)`, a set `{1, 2}`, a trailing comma, `b'bytes'`, an
/// `f'…'` string, `1_000`, `0x1f`, `inf`/`nan`, a concatenated pair of adjacent string literals, and any
/// escape sequence Python spells differently from JSON. A refusal is `None`, which leaves the string to be
/// read as text - so the failure mode is a tool definition not recognised, never a wrong one.
fn python_literal_to_json(s: &str) -> Option<JsonValue> {
    if !s.starts_with('{') && !s.starts_with('[') {
        return None;
    }

    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + 16);
    let mut i = 0;
    let mut in_string = false;
    let mut quote_char = 0u8;

    while i < len {
        let b = bytes[i];

        if in_string {
            if b == quote_char {
                out.push('"');
                in_string = false;
                i += 1;
            } else if b == b'"' && quote_char == b'\'' {
                out.push_str("\\\"");
                i += 1;
            } else if b == b'\\' && i + 1 < len {
                let next = bytes[i + 1];
                match next {
                    b'\'' => {
                        out.push('\'');
                        i += 2;
                    }
                    b'"' => {
                        out.push_str("\\\"");
                        i += 2;
                    }
                    b'\\' | b'/' | b'n' | b't' | b'r' | b'b' | b'f' => {
                        out.push('\\');
                        out.push(next as char);
                        i += 2;
                    }
                    b'u' => {
                        out.push('\\');
                        out.push('u');
                        i += 2;
                    }
                    _ => {
                        out.push('\\');
                        i += 1;
                    }
                }
            } else {
                let ch = s[i..].chars().next()?;
                out.push(ch);
                i += ch.len_utf8();
            }
        } else {
            match b {
                b'\'' | b'"' => {
                    out.push('"');
                    in_string = true;
                    quote_char = b;
                    i += 1;
                }
                b'T' if matches_python_literal(bytes, i, b"True") => {
                    out.push_str("true");
                    i += 4;
                }
                b'F' if matches_python_literal(bytes, i, b"False") => {
                    out.push_str("false");
                    i += 5;
                }
                b'N' if matches_python_literal(bytes, i, b"None") => {
                    out.push_str("null");
                    i += 4;
                }
                _ => {
                    let ch = s[i..].chars().next()?;
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
    }

    serde_json::from_str(&out).ok()
}

#[inline]
fn matches_python_literal(bytes: &[u8], i: usize, literal: &[u8]) -> bool {
    let end = i + literal.len();
    if end > bytes.len() || bytes[i..end] != *literal {
        return false;
    }
    if end < bytes.len() {
        let after = bytes[end];
        if after.is_ascii_alphanumeric() || after == b'_' {
            return false;
        }
    }
    if i > 0 {
        let before = bytes[i - 1];
        if before.is_ascii_alphanumeric() || before == b'_' {
            return false;
        }
    }
    true
}

fn python_args_to_json_schema(value: &JsonValue, spec: &ToolReprSpec) -> Option<JsonValue> {
    let args = value.as_object()?;
    let mut properties = serde_json::Map::new();

    for (name, meta) in args {
        let mut prop = serde_json::Map::new();
        match meta {
            JsonValue::Object(m) => {
                if let Some(type_name) = m.get("type").and_then(|v| v.as_str())
                    && let Some(mapped) = json_schema_type(type_name, spec)
                {
                    prop.insert("type".to_string(), json!(mapped));
                }
                if let Some(desc) = m.get("description").and_then(|v| v.as_str())
                    && !desc.trim().is_empty()
                {
                    prop.insert("description".to_string(), json!(desc));
                }
            }
            JsonValue::String(type_name) => {
                if let Some(mapped) = json_schema_type(type_name, spec) {
                    prop.insert("type".to_string(), json!(mapped));
                }
            }
            _ => {}
        }
        // A property with nothing to say stays `{}`, which is JSON Schema for "unconstrained" and accepts
        // anything. It used to become `type: string` here - a literal in Rust overriding the asset's declared
        // `type_default`, so `unconstrained` produced a schema that **rejects a number**. Same defect as the
        // default itself, one layer further down, and the layer nobody looks at.
        properties.insert(name.clone(), JsonValue::Object(prop));
    }

    Some(json!({
        "type": "object",
        "properties": properties
    }))
}

/// The JSON Schema type for a producer's type name, or `None` where the declaration says to constrain nothing.
///
/// `None` writes **no** `type` member, which is what "the widest type" means in JSON Schema. The previous
/// default was `"string"`, which is the opposite of widest: a schema saying `type: string` rejects the number a
/// producer may well pass.
fn json_schema_type(type_name: &str, spec: &ToolReprSpec) -> Option<String> {
    let name = type_name.trim();
    if let Some((_, target)) = spec
        .type_map
        .iter()
        .find(|(source, _)| source.eq_ignore_ascii_case(name))
    {
        return Some(target.clone());
    }
    match &spec.type_default {
        super::schema::UnknownType::Unconstrained => None,
        super::schema::UnknownType::MapTo(target) => Some(target.clone()),
    }
}

fn tool_from_value(value: &JsonValue, spec: &ToolReprSpec) -> Option<JsonValue> {
    if let Some(s) = value.as_str() {
        // A marker on an **identifier boundary**. A marker like `name=` matched inside `username=`, so a
        // constructor repr that names no tool was decoded as one anyway - and `repr_field` then found
        // `name='admin'` inside that same token, inventing a tool called `admin`. A marker that is not an
        // identifier prefix (`CrewStructuredTool(`) is unaffected: the boundary test only rejects a match whose
        // preceding character continues an identifier.
        if spec
            .repr_markers
            .iter()
            .any(|marker| find_field(s, marker.as_str()).is_some())
        {
            return tool_from_repr(s, spec);
        }

        let name = s.trim();
        if name.is_empty() {
            return None;
        }
        return Some(json!({
            "type": "function",
            "function": { "name": name }
        }));
    }

    let obj = value.as_object()?;

    // Already OpenAI-style function definition.
    if let Some(function) = obj.get("function")
        && function.get("name").and_then(|n| n.as_str()).is_some()
    {
        let mut tool = json!({
            "type": "function",
            "function": function.clone()
        });
        if let Some(strict) = obj.get("strict") {
            tool["strict"] = strict.clone();
        }
        return Some(tool);
    }

    let fallback_name = obj
        .get(spec.name_field.as_str())
        .and_then(|n| n.as_str())?
        .trim();
    if fallback_name.is_empty() {
        return None;
    }

    let mut function = json!({ "name": fallback_name });

    if let Some(desc) = obj
        .get(spec.description_field.as_str())
        .and_then(|d| d.as_str())
    {
        let (name, parsed_desc, parsed_params) = labelled_details(desc, fallback_name, spec);
        function["name"] = json!(name);
        if let Some(parsed_desc) = parsed_desc {
            function["description"] = json!(parsed_desc);
        } else if !desc.trim().is_empty() {
            function["description"] = json!(desc.trim());
        }
        if let Some(parsed_params) = parsed_params {
            function["parameters"] = parsed_params;
        }
    }

    if function.get("parameters").is_none()
        && let Some(params_value) = spec
            .parameter_members
            .iter()
            .find_map(|member| obj.get(member.as_str()))
        && let Some(params) = normalised_parameters(params_value, spec)
    {
        function["parameters"] = params;
    }

    Some(json!({
        "type": "function",
        "function": function
    }))
}

fn normalised_parameters(value: &JsonValue, spec: &ToolReprSpec) -> Option<JsonValue> {
    if value.is_null() {
        return None;
    }

    if let Some(s) = value.as_str() {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(s) {
            return normalised_parameters(&parsed, spec);
        }
        if let Some(parsed) = python_literal_to_json(s) {
            return normalised_parameters(&parsed, spec);
        }
    }

    if value
        .get("type")
        .is_some_and(|t| t.is_string() || t.is_object() || t.is_array())
        || value.get("properties").is_some()
    {
        return Some(value.clone());
    }

    python_args_to_json_schema(value, spec)
}

fn declared_name(tool: &JsonValue) -> Option<String> {
    tool.get("function")
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
        .or_else(|| tool.get("name").and_then(|n| n.as_str()))
        .or_else(|| tool.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

fn upsert_best(
    tool: JsonValue,
    best_by_name: &mut HashMap<String, (i32, JsonValue)>,
    order: &mut Vec<String>,
) -> bool {
    let Some(name) = declared_name(&tool) else {
        return false;
    };
    let quality = tool_definition_quality(&tool);
    if !best_by_name.contains_key(&name) {
        order.push(name.clone());
    }
    match best_by_name.get(&name) {
        Some((best_quality, _)) if *best_quality >= quality => false,
        _ => {
            best_by_name.insert(name, (quality, tool));
            true
        }
    }
}

fn in_first_seen_order(
    mut best_by_name: HashMap<String, (i32, JsonValue)>,
    order: Vec<String>,
) -> Option<Vec<JsonValue>> {
    if best_by_name.is_empty() {
        return None;
    }

    let mut tools = Vec::with_capacity(best_by_name.len());
    for name in order {
        if let Some((_, tool)) = best_by_name.remove(&name) {
            tools.push(tool);
        }
    }

    if tools.is_empty() { None } else { Some(tools) }
}

/// The `repr` grammar, reachable from the structural test that pins its specified language.
#[cfg(test)]
pub(crate) fn python_literal_to_json_for_test(s: &str) -> Option<JsonValue> {
    python_literal_to_json(s)
}
