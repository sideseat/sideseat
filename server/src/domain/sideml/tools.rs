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

/// Normalize a single tool definition to OpenAI format
fn normalize_tool_definition(tool: &JsonValue) -> Vec<JsonValue> {
    // Try each provider format
    try_openai_tool(tool)
        .or_else(|| try_anthropic_tool(tool))
        .or_else(|| try_bedrock_tool(tool))
        .or_else(|| try_gemini_tool(tool))
        .or_else(|| try_cohere_tool(tool))
        .unwrap_or_else(|| vec![tool.clone()]) // passthrough if unknown
}

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

/// Extract tool name from a tool definition in any supported format.
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
pub fn extract_tool_name(tool: &JsonValue) -> Option<String> {
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
}
