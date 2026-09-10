//! Tool call and definition normalization.
//!
//! Normalizes tool calls and tool definitions from various AI provider formats
//! to unified OpenAI-compatible format.

use serde_json::{Value as JsonValue, json};

use super::types::ChatRole;

// ========== Helper functions ==========

/// Convert a JSON value to string representation.
/// If it's already a string, returns the string value.
/// Otherwise, serializes the JSON to a string.
fn json_value_to_string(value: &JsonValue) -> String {
    match value.as_str() {
        Some(s) => s.to_string(),
        None => value.to_string(),
    }
}

// ========== Tool call normalizer ==========

/// Normalize tool_calls from any format to flat format
pub fn normalize_tool_calls(msg: &JsonValue) -> Option<JsonValue> {
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

/// Extract tool_use_id from various known locations in the raw message.
///
/// Centralizes extraction to handle different provider formats:
/// - `tool_call_id`: Standard field (OpenAI, most providers) - any role
/// - `tool_use_id`: Anthropic format
/// - `id`: Used by Logfire events - only for tool/function roles
/// - `call_id`: Used in OpenInference function data - only for tool/function roles
/// - `toolResult.toolUseId`: Bedrock/Strands nested format
/// - `toolUse.toolUseId`: Bedrock/Strands nested format
pub fn extract_tool_use_id(msg: &JsonValue, role: &str) -> Option<String> {
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

// ========== Tool definition normalizer ==========

/// Normalize tool definitions from any provider format to OpenAI format
pub fn normalize_tools(tools: &JsonValue) -> JsonValue {
    match tools {
        JsonValue::Array(arr) => {
            let normalized: Vec<JsonValue> =
                arr.iter().flat_map(normalize_tool_definition).collect();
            json!(normalized)
        }
        JsonValue::Object(_) => {
            // Single tool or Gemini wrapper
            let normalized: Vec<JsonValue> = normalize_tool_definition(tools);
            json!(normalized)
        }
        _ => tools.clone(),
    }
}

/// One tool definition in the canonical shape.
///
/// **Declared**, from `rules/tool-shapes.json`. Five readers used to name providers here and an unrecognised
/// shape was passed through unchanged - then discarded downstream, because no name could be extracted from it.
/// So a producer's shape was a code change to support, and an unknown one silently produced nothing usable.
///
/// The passthrough stays for a shape nothing recognises: it is what the retired chain did, and dropping the
/// payload would lose a definition a reader might still make sense of. The retired readers are `#[cfg(test)]`
/// oracles, compared against the declarations over the whole corpus.
fn normalize_tool_definition(tool: &JsonValue) -> Vec<JsonValue> {
    crate::domain::rules::ruleset()
        .tool_shapes
        .canonical(tool)
        .unwrap_or_else(|| vec![tool.clone()])
}

#[cfg(test)]
/// OpenAI format: {"type": "function", "function": {"name": ..., "parameters": ...}, "strict": ...}
fn try_openai_tool(tool: &JsonValue) -> Option<Vec<JsonValue>> {
    let tool_type = tool.get("type")?.as_str()?;
    if tool_type != "function" {
        return None;
    }
    let func = tool.get("function")?;

    // Build normalized tool, preserving strict field
    let mut normalized = json!({
        "type": "function",
        "function": func.clone()
    });
    if let Some(strict) = tool.get("strict") {
        normalized["strict"] = strict.clone();
    }
    Some(vec![normalized])
}

#[cfg(test)]
/// Anthropic format: {"name": ..., "description": ..., "input_schema": ...}
fn try_anthropic_tool(tool: &JsonValue) -> Option<Vec<JsonValue>> {
    let name = tool.get("name")?.as_str()?;
    // Must have input_schema (Anthropic-specific) and NOT have "function" (OpenAI)
    let input_schema = tool.get("input_schema")?;
    if tool.get("function").is_some() {
        return None;
    }

    Some(vec![json!({
        "type": "function",
        "function": {
            "name": name,
            "description": tool.get("description"),
            "parameters": input_schema.clone()
        }
    })])
}

#[cfg(test)]
/// Bedrock/Strands format: {"toolSpec": {"name": ..., "inputSchema": {"json": ...}}}
fn try_bedrock_tool(tool: &JsonValue) -> Option<Vec<JsonValue>> {
    let tool_spec = tool.get("toolSpec")?;
    let name = tool_spec.get("name")?.as_str()?;
    let description = tool_spec.get("description").and_then(|d| d.as_str());

    // inputSchema can be {"json": {...}} or just {...}
    let parameters = tool_spec
        .get("inputSchema")
        .and_then(|schema| schema.get("json").or(Some(schema)))
        .cloned();

    Some(vec![json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    })])
}

#[cfg(test)]
/// Gemini format: {"functionDeclarations": [{"name": ..., "parameters": ...}]}
fn try_gemini_tool(tool: &JsonValue) -> Option<Vec<JsonValue>> {
    let declarations = tool.get("functionDeclarations")?.as_array()?;

    let tools: Vec<JsonValue> = declarations
        .iter()
        .filter_map(|decl| {
            let name = decl.get("name")?.as_str()?;
            Some(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": decl.get("description"),
                    "parameters": decl.get("parameters").cloned()
                }
            }))
        })
        .collect();

    if tools.is_empty() { None } else { Some(tools) }
}

#[cfg(test)]
/// Cohere format: {"name": ..., "description": ..., "parameter_definitions": {...}}
fn try_cohere_tool(tool: &JsonValue) -> Option<Vec<JsonValue>> {
    let name = tool.get("name")?.as_str()?;
    // Must have parameter_definitions (Cohere-specific) and NOT have input_schema (Anthropic)
    let param_defs = tool.get("parameter_definitions")?;
    if tool.get("input_schema").is_some() || tool.get("function").is_some() {
        return None;
    }

    // Convert Cohere parameter_definitions to JSON Schema format
    let parameters = cohere_params_to_json_schema(param_defs);

    Some(vec![json!({
        "type": "function",
        "function": {
            "name": name,
            "description": tool.get("description"),
            "parameters": parameters
        }
    })])
}

#[cfg(test)]
/// Convert Cohere parameter_definitions to JSON Schema format
fn cohere_params_to_json_schema(param_defs: &JsonValue) -> JsonValue {
    let Some(obj) = param_defs.as_object() else {
        return json!({"type": "object", "properties": {}});
    };

    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for (name, def) in obj {
        let mut prop = serde_json::Map::new();
        if let Some(t) = def.get("type").and_then(|t| t.as_str()) {
            prop.insert("type".to_string(), json!(t));
        }
        if let Some(d) = def.get("description") {
            prop.insert("description".to_string(), d.clone());
        }
        properties.insert(name.clone(), JsonValue::Object(prop));

        if def
            .get("required")
            .and_then(|r| r.as_bool())
            .unwrap_or(false)
        {
            required.push(json!(name));
        }
    }

    json!({
        "type": "object",
        "properties": properties,
        "required": required
    })
}

// ========== Tool name extraction ==========

/// The name a tool definition states, whatever shape states it.
///
/// **Declared**, from `rules/tool-shapes.json`: our own canonical form first, then whatever the assets say a
/// producer writes - whose last clause is the named-object shape, so this needs no fallback of its own. It used
/// to be a seven-arm provider chain, a *second* vocabulary beside `normalize_tool_definition`'s, and the two
/// disagreed about two spellings; the retired chain is a `#[cfg(test)]` oracle.
///
/// `None` where the payload is not an object or states no name - and that is what the read side acts on, since
/// `deduplicate_tools` keeps only what can be named.
pub fn extract_tool_name(tool: &JsonValue) -> Option<String> {
    if let Some(name) = canonical_tool_name(tool) {
        return Some(name.to_string());
    }
    let declared = crate::domain::rules::ruleset()
        .tool_shapes
        .canonical(tool)?;
    declared
        .iter()
        .find_map(|definition| canonical_tool_name(definition).map(str::to_string))
}

/// The name in our own canonical wrapper.
fn canonical_tool_name(definition: &JsonValue) -> Option<&str> {
    definition.get("function")?.get("name")?.as_str()
}

#[cfg(test)]
/// The provider chain this file used to name a tool with, kept as an **oracle**.
///
/// A *second* vocabulary of provider shapes beside the declared one, and it knew two spellings the declared one
/// did not (`inputSchema`, and `function_declarations` in snake_case) while the declared one knew a conversion
/// it did not. So which shapes SideSeat understood depended on which question was being asked about them.
///
/// Handles multiple provider formats:
/// - OpenAI: `function.name`
/// - Anthropic: `name` (top-level, with `input_schema`)
/// - Vercel AI SDK: `name` (top-level, with `inputSchema` - camelCase)
/// - Bedrock: `toolSpec.name`
/// - Gemini: `functionDeclarations[0].name`
/// - Cohere: `name` (top-level, with `parameter_definitions`)
/// - Fallback: any object with top-level `name` field
///
/// Returns `None` only if the input is not an object or has no extractable name.
fn extract_tool_name_retired(tool: &JsonValue) -> Option<String> {
    // OpenAI: function.name
    if let Some(name) = tool
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
    {
        return Some(name.to_string());
    }

    // Anthropic: name (top-level, has input_schema)
    if tool.get("input_schema").is_some()
        && let Some(name) = tool.get("name").and_then(|n| n.as_str())
    {
        return Some(name.to_string());
    }

    // Vercel AI SDK: name (top-level, has inputSchema - camelCase)
    if tool.get("inputSchema").is_some()
        && let Some(name) = tool.get("name").and_then(|n| n.as_str())
    {
        return Some(name.to_string());
    }

    // Bedrock: toolSpec.name
    if let Some(name) = tool
        .get("toolSpec")
        .and_then(|ts| ts.get("name"))
        .and_then(|n| n.as_str())
    {
        return Some(name.to_string());
    }

    // Gemini: functionDeclarations[0].name (camelCase)
    if let Some(name) = tool
        .get("functionDeclarations")
        .and_then(|fd| fd.get(0))
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
    {
        return Some(name.to_string());
    }

    // Gemini ADK: function_declarations[0].name (snake_case)
    if let Some(name) = tool
        .get("function_declarations")
        .and_then(|fd| fd.get(0))
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
    {
        return Some(name.to_string());
    }

    // Cohere: name (top-level, has parameter_definitions)
    if tool.get("parameter_definitions").is_some()
        && let Some(name) = tool.get("name").and_then(|n| n.as_str())
    {
        return Some(name.to_string());
    }

    // Fallback: any object with top-level "name" field (must be an object, not primitive)
    // This catches unknown formats that follow the common pattern
    if tool.is_object()
        && let Some(name) = tool.get("name").and_then(|n| n.as_str())
    {
        return Some(name.to_string());
    }

    None
}

/// Score tool definition quality based on metadata richness.
///
/// Higher score = more complete definition. Used for deduplication:
/// when the same tool appears from multiple sources, the highest-quality
/// version is preferred as the merge base.
///
/// Weights: description(2) + parameters(2) + properties(4) + required(1)
pub fn tool_definition_quality(def: &JsonValue) -> i32 {
    let func = def.get("function").unwrap_or(def);
    let has_description = func
        .get("description")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.trim().is_empty());
    let params = func.get("parameters");
    let has_params = params.is_some();
    let has_properties = params
        .and_then(|p| p.get("properties"))
        .and_then(|p| p.as_object())
        .is_some_and(|m| !m.is_empty());
    let has_required = params
        .and_then(|p| p.get("required"))
        .and_then(|r| r.as_array())
        .is_some_and(|a| !a.is_empty());

    (has_description as i32) * 2
        + (has_params as i32) * 2
        + (has_properties as i32) * 4
        + (has_required as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every shape either chain knew, plus the two neither wholly did.
    fn tool_definition_payloads() -> Vec<JsonValue> {
        vec![
            // Our own canonical wrapper, with and without `strict`.
            json!({"type": "function", "function": {"name": "a", "description": "d", "parameters": {"type": "object"}}}),
            json!({"type": "function", "function": {"name": "a"}, "strict": true}),
            // A name beside a schema, in all three spellings a producer uses.
            json!({"name": "b", "description": "d", "input_schema": {"type": "object", "properties": {"q": {"type": "string"}}}}),
            json!({"type": "function", "name": "b", "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}}),
            json!({"name": "b", "parameters": {"type": "object"}, "strict": false}),
            // Nested under a spec, schema wrapped and direct.
            json!({"toolSpec": {"name": "c", "description": "d", "inputSchema": {"json": {"type": "object"}}}}),
            json!({"toolSpec": {"name": "c", "inputSchema": {"type": "object"}}}),
            // Several declarations in one payload, both spellings.
            json!({"functionDeclarations": [{"name": "d1", "parameters": {"type": "object"}}, {"name": "d2"}]}),
            json!({"function_declarations": [{"name": "e1", "description": "d"}]}),
            // Arguments as a map from name to facts.
            json!({"name": "f", "parameter_definitions": {"q": {"type": "str", "required": true, "description": "d"}}}),
            // A named object stating nothing else about itself.
            json!({"name": "g"}),
            json!({"name": "g", "unknown_member": 1}),
            // Nothing nameable.
            json!({"description": "no name"}),
            json!({"name": 7}),
            json!({"functionDeclarations": []}),
            json!("a string"),
            json!(123),
            json!(null),
            json!([1, 2, 3]),
        ]
    }

    /// The declared shapes name every payload the retired provider chain named.
    ///
    /// One direction only, deliberately: the chain is the floor, not the ceiling. The declarations *also* name
    /// two shapes it could not - `parameter_definitions` reaches a name through a conversion the chain had no
    /// arm for - and requiring equality would forbid exactly the extension the assets exist to allow. What must
    /// never happen is losing a name that used to be found, which is what this asserts.
    #[test]
    fn the_declared_shapes_name_every_tool_the_retired_chain_named() {
        for payload in tool_definition_payloads() {
            let retired = extract_tool_name_retired(&payload);
            let declared = extract_tool_name(&payload);
            if let Some(name) = retired {
                assert_eq!(
                    declared.as_deref(),
                    Some(name.as_str()),
                    "the declarations lost a name the retired chain found, for {payload}"
                );
            }
        }
        // And the two spellings that were only ever in the retired chain are declared now, which is the
        // disagreement this migration existed to remove.
        for payload in [
            json!({"type": "function", "name": "vercel", "inputSchema": {"type": "object"}}),
            json!({"function_declarations": [{"name": "snake"}]}),
        ] {
            assert!(
                extract_tool_name(&payload).is_some(),
                "a spelling only the retired chain knew is still unnamed: {payload}"
            );
        }
    }

    #[test]
    fn test_extract_tool_name_openai() {
        let tool = json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather",
                "parameters": {}
            }
        });
        assert_eq!(extract_tool_name(&tool), Some("get_weather".to_string()));
    }

    #[test]
    fn test_extract_tool_name_anthropic() {
        let tool = json!({
            "name": "get_weather",
            "description": "Get weather",
            "input_schema": {
                "type": "object",
                "properties": {}
            }
        });
        assert_eq!(extract_tool_name(&tool), Some("get_weather".to_string()));
    }

    #[test]
    fn test_extract_tool_name_vercel_ai() {
        let tool = json!({
            "type": "function",
            "name": "temperature_forecast",
            "description": "Get the temperature forecast",
            "inputSchema": {
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                }
            }
        });
        assert_eq!(
            extract_tool_name(&tool),
            Some("temperature_forecast".to_string())
        );
    }

    #[test]
    fn test_extract_tool_name_bedrock() {
        let tool = json!({
            "toolSpec": {
                "name": "get_weather",
                "description": "Get weather",
                "inputSchema": {}
            }
        });
        assert_eq!(extract_tool_name(&tool), Some("get_weather".to_string()));
    }

    #[test]
    fn test_extract_tool_name_unknown_format() {
        let tool = json!({
            "unknown": "format",
            "no_name_field": true
        });
        assert_eq!(extract_tool_name(&tool), None);
    }

    #[test]
    fn test_extract_tool_name_fallback_simple_object() {
        // Simple object with just name - fallback should catch it
        let tool = json!({
            "name": "simple_tool",
            "description": "A simple tool without schema"
        });
        assert_eq!(extract_tool_name(&tool), Some("simple_tool".to_string()));
    }

    #[test]
    fn test_extract_tool_name_primitive_values() {
        // Primitives should return None
        assert_eq!(extract_tool_name(&json!("string")), None);
        assert_eq!(extract_tool_name(&json!(123)), None);
        assert_eq!(extract_tool_name(&json!(null)), None);
        assert_eq!(extract_tool_name(&json!([1, 2, 3])), None);
    }

    #[test]
    fn test_normalize_tool_calls_langchain_args() {
        // LangChain uses "args" instead of "arguments"
        let msg = json!({
            "tool_calls": [{
                "name": "Person",
                "args": {"name": "Jane Doe", "age": 28},
                "id": "tooluse_123",
                "type": "tool_call"
            }]
        });
        let result = normalize_tool_calls(&msg).unwrap();
        let calls = result.as_array().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["name"], "Person");
        // Arguments should be stringified JSON from "args"
        let args_str = calls[0]["arguments"].as_str().unwrap();
        let args: serde_json::Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(args["name"], "Jane Doe");
        assert_eq!(args["age"], 28);
    }

    #[test]
    fn test_normalize_tool_calls_openai_arguments() {
        // OpenAI uses "arguments" in function wrapper
        let msg = json!({
            "tool_calls": [{
                "id": "call_123",
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "arguments": "{\"city\":\"NYC\"}"
                }
            }]
        });
        let result = normalize_tool_calls(&msg).unwrap();
        let calls = result.as_array().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["name"], "get_weather");
        assert_eq!(calls[0]["arguments"], "{\"city\":\"NYC\"}");
    }

    /// The declared shapes answer as the five retired readers did.
    ///
    /// The readers are kept as oracles rather than deleted, because a golden can be regenerated and bless a
    /// regression while an oracle cannot: it states the answer independently of the code that replaced it.
    ///
    /// One shape per provider, plus the cases each reader's guards exist for - a canonical wrapper beside a
    /// `name`, an `inputSchema` written both ways, a payload holding several declarations, and an argument map
    /// beside an `input_schema` that must not claim it.
    #[test]
    fn the_declared_tool_shapes_answer_as_the_retired_readers() {
        let retired = |tool: &JsonValue| -> Vec<JsonValue> {
            try_openai_tool(tool)
                .or_else(|| try_anthropic_tool(tool))
                .or_else(|| try_bedrock_tool(tool))
                .or_else(|| try_gemini_tool(tool))
                .or_else(|| try_cohere_tool(tool))
                .unwrap_or_else(|| vec![tool.clone()])
        };

        let cases = vec![
            // The canonical wrapper, with and without the member carried beside it.
            json!({"type": "function", "function": {"name": "search", "parameters": {"type": "object"}}}),
            json!({"type": "function", "function": {"name": "search"}, "strict": true}),
            // A name beside a JSON Schema.
            json!({"name": "search", "description": "find things", "input_schema": {"type": "object"}}),
            json!({"name": "search", "input_schema": {"type": "object"}}),
            // Nested under `toolSpec`, schema written both ways.
            json!({"toolSpec": {"name": "search", "inputSchema": {"json": {"type": "object"}}}}),
            json!({"toolSpec": {"name": "search", "description": "d", "inputSchema": {"type": "object"}}}),
            // Several declarations in one payload.
            json!({"functionDeclarations": [
                {"name": "a", "parameters": {"type": "object"}},
                {"name": "b", "description": "second"}
            ]}),
            // An argument map, including the constraints the retired converter kept.
            json!({"name": "search", "parameter_definitions": {
                "city": {"type": "string", "description": "where", "required": true},
                "days": {"type": "integer"}
            }}),
            // The guards: each of these is a payload one reader must *not* claim.
            json!({"type": "not_a_function", "function": {"name": "x"}}),
            json!({"name": "x", "parameter_definitions": {}, "input_schema": {"type": "object"}}),
            json!({"name": "x", "parameter_definitions": {}, "function": {"name": "y"}}),
            json!({"functionDeclarations": []}),
            // And a shape nothing recognises, which both must pass through.
            json!({"spec": {"id": "lookup", "doc": "search records"}}),
            json!("a bare string"),
        ];

        // **One deliberate divergence, named rather than absorbed.** The retired readers inserted
        // `tool.get("description")` directly, and a `None` there serialises as `null` - so a tool with no
        // description got `"description": null`, which is a statement that the description *is* null. The
        // declarations omit the member instead. Compared with explicit nulls stripped from both sides, so the
        // oracle still covers every other difference, and the divergence is asserted on its own below.
        fn without_nulls(value: &JsonValue) -> JsonValue {
            match value {
                JsonValue::Object(members) => JsonValue::Object(
                    members
                        .iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, v)| (k.clone(), without_nulls(v)))
                        .collect(),
                ),
                JsonValue::Array(items) => {
                    JsonValue::Array(items.iter().map(without_nulls).collect())
                }
                other => other.clone(),
            }
        }
        for tool in &cases {
            let declared: Vec<JsonValue> = normalize_tool_definition(tool)
                .iter()
                .map(without_nulls)
                .collect();
            let oracle: Vec<JsonValue> = retired(tool).iter().map(without_nulls).collect();
            assert_eq!(
                declared, oracle,
                "the declarations disagree with the retired readers on {tool}"
            );
        }

        // The divergence itself: absent is absent, not null.
        let no_description = json!({"name": "search", "input_schema": {"type": "object"}});
        assert!(
            normalize_tool_definition(&no_description)[0]["function"]
                .get("description")
                .is_none(),
            "a tool with no description has no description member"
        );
        assert_eq!(
            retired(&no_description)[0]["function"]["description"],
            JsonValue::Null,
            "where the retired reader wrote `null` - which says the description is null, and it is not"
        );
        // Corpus-neutral: no captured fixture has a definition whose description is absent in a shape that
        // reaches this path, which is why the goldens do not move.
    }

    /// The argument-map converter keeps every constraint it is given, which the retired one did not.
    ///
    /// Cycle 13's finding 8: only `type` and `description` survived, so the output said an argument was optional
    /// where the producer said it was required, and discarded its default and its allowed values. `required`
    /// belongs at the schema level, which is where JSON Schema puts it.
    ///
    /// Constraints are copied **by name**, so one this code has never heard of survives - the alternative is a
    /// schema that silently permits what the producer forbade.
    #[test]
    fn an_argument_map_keeps_every_constraint_it_was_given() {
        let schema = crate::domain::rules::tool_shapes::argument_map_to_json_schema(&json!({
            "query": {
                "type": "string",
                "description": "what to search for",
                "required": true,
                "default": "all",
                "enum": ["all", "recent"]
            },
            "limit": {"type": "integer"}
        }));
        let query = &schema["properties"]["query"];
        assert_eq!(query["type"].as_str(), Some("string"));
        assert_eq!(query["description"].as_str(), Some("what to search for"));
        assert_eq!(
            query["default"].as_str(),
            Some("all"),
            "a default the producer stated is what the model sends when it says nothing"
        );
        assert_eq!(
            query["enum"].as_array().map(Vec::len),
            Some(2),
            "and the allowed values are what it may send at all"
        );
        assert_eq!(
            schema["required"].as_array(),
            Some(&vec![json!("query")]),
            "requiredness is a schema-level fact, and dropping it said an argument was optional"
        );
        // `required` is not copied into the property: it is not a JSON Schema keyword there.
        assert!(query.get("required").is_none());
        // An argument with only a type is unremarkable and stays that way.
        assert_eq!(schema["properties"]["limit"], json!({"type": "integer"}));
    }
}
