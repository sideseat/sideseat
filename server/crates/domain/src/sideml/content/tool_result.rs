use super::*;

/// Convert content blocks to unified tool_result format when associated with a tool call.
///
/// Behavior:
/// - Multiple tool_results: Keep as-is (LLM span with multiple tool results)
/// - Single tool_result with siblings: Merge siblings INTO the tool_result's content
/// - Single tool_result alone: Keep as-is
/// - No tool_result: Create one wrapping all content blocks
///
/// This ensures consistent nested format: [tool_result with [all content inside]]
///
/// # Important: Role-based conversion only
///
/// Conversion is triggered by ROLE, not by `tool_use_id` presence. While `tool_use_id`
/// can appear on both tool results AND assistant messages (tool calls), we only want
/// to wrap content in tool_result for actual tool role messages.
pub fn convert_to_tool_result(
    content: &JsonValue,
    role: &str,
    tool_use_id: &Option<String>,
) -> JsonValue {
    // Only convert for tool role - NOT based on tool_use_id presence alone
    // (tool_use_id can appear on assistant messages with tool calls too)
    if !ChatRole::is_tool_role(role) {
        return content.clone();
    }

    let Some(arr) = content.as_array() else {
        return content.clone();
    };

    // If content has tool_use blocks, this is an assistant message with tool calls - don't convert
    let has_tool_use = arr.iter().any(|b| get_block_type(b) == Some("tool_use"));
    if has_tool_use {
        return content.clone();
    }

    // Partition into tool_result blocks and other blocks
    let (tool_results, others): (Vec<_>, Vec<_>) = arr
        .iter()
        .partition(|b| get_block_type(b) == Some("tool_result"));

    match (tool_results.len(), others.len()) {
        // No tool_result: create one wrapping all blocks
        (0, _) if !arr.is_empty() => {
            let inner_content = create_inner_content(arr);
            json!([create_tool_result(tool_use_id, inner_content)])
        }

        // Single tool_result, no siblings: keep as-is
        (1, 0) => content.clone(),

        // Single tool_result WITH siblings: merge siblings into its content
        (1, _) => {
            let tool_result = tool_results[0];
            let merged_content = merge_into_tool_result(tool_result, &others);
            json!([merged_content])
        }

        // Multiple tool_results: keep as-is (each is complete)
        (_, _) => content.clone(),
    }
}

/// Create inner content for tool_result from content blocks.
/// Single text block becomes string, single json/unknown becomes raw data, multiple become array.
fn create_inner_content(blocks: &[JsonValue]) -> JsonValue {
    // Track if single block was json/unknown for optimization
    let single_raw_data = if blocks.len() == 1 {
        match get_block_type(&blocks[0]) {
            Some("unknown") => blocks[0].get("raw").cloned(),
            Some("json") => blocks[0].get("data").cloned(),
            _ => None,
        }
    } else {
        None
    };

    // Extract actual content from wrapper types
    let inner: Vec<JsonValue> = blocks
        .iter()
        .filter_map(|block| {
            match get_block_type(block) {
                // For unknown/json, extract the raw data directly
                Some("unknown") => block.get("raw").cloned(),
                Some("json") => block.get("data").cloned(),
                _ => Some(block.clone()),
            }
        })
        .collect();

    // Single block optimizations for cleaner output
    if inner.len() == 1 {
        // Single text block: use string content
        if let Some(text) = inner[0].get("text").and_then(|t| t.as_str()) {
            return json!(text);
        }
        // Single json/unknown block: use raw data directly (tracked from original block type)
        if let Some(raw) = single_raw_data {
            return raw;
        }
    }

    json!(inner)
}

/// Merge sibling blocks into an existing tool_result's content.
fn merge_into_tool_result(tool_result: &JsonValue, siblings: &[&JsonValue]) -> JsonValue {
    let current_content = tool_result.get("content").cloned().unwrap_or(json!(null));

    // Convert current content to array
    let mut content_arr = match current_content {
        JsonValue::Array(arr) => arr,
        JsonValue::String(s) => vec![json!({"type": "text", "text": s})],
        JsonValue::Null => vec![],
        other => vec![json!({"type": "json", "data": other})],
    };

    // Append sibling blocks
    for block in siblings {
        content_arr.push((*block).clone());
    }

    // Recreate tool_result with merged content
    json!({
        "type": "tool_result",
        "tool_use_id": tool_result.get("tool_use_id").cloned(),
        "content": json!(content_arr),
        "is_error": tool_result.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false)
    })
}

/// Create a new tool_result block.
fn create_tool_result(tool_use_id: &Option<String>, content: JsonValue) -> JsonValue {
    json!({
        "type": "tool_result",
        "tool_use_id": tool_use_id.clone(),
        "content": content,
        "is_error": false
    })
}
