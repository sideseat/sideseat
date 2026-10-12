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
            // A message's `content` list is its list of parts, by every chat convention - never the tool's return
            // value, which is what a part holds. So a bare element is a part the producer left untyped, and no
            // source can show a tool's own list here: that evidence exists only where a member holds the return
            // value itself (a result block's `result`), which the declared cases judge.
            let inner_content =
                canonical_inner_content(&crate::rules::ruleset().content_blocks, arr, None);
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

/// A tool result's content from its blocks, in the canonical form: one part that encodes nothing beyond its
/// value is that value ([`canonical_single_part`]), judged against `source`, the content as written. Any other
/// single part stays a part (a json primitive, a block-shaped value, a text with citations); an `unknown` part
/// keeps its raw value, and several parts read as [`create_inner_content`] reads them.
pub(crate) fn canonical_inner_content(
    plan: &crate::rules::content_blocks::ContentBlockPlan,
    blocks: &[JsonValue],
    source: Option<&JsonValue>,
) -> JsonValue {
    let [block] = blocks else {
        return several_parts(plan, blocks);
    };
    if let Some(content) = canonical_single_part(plan, block, |value| {
        source.is_some_and(|s| listed_by_tool(s, value))
    }) {
        return content;
    }
    if get_block_type(block) == Some("unknown")
        && let Some(raw) = block.get("raw")
    {
        return raw.clone();
    }
    json!(blocks)
}

/// Several parts as a tool result's content: each json part as its data and each `unknown` part as its raw value,
/// as the retired reduction reads them - except a json part around data that is not plain ([`is_plain_value`]) or a
/// string, which keeps its part: unwrapped, returned JSON spelled like an image would become the image, and a json
/// string text.
fn several_parts(
    plan: &crate::rules::content_blocks::ContentBlockPlan,
    blocks: &[JsonValue],
) -> JsonValue {
    // A json string keeps its part too: bare in a list, a string is read as text, which it was not.
    let block_shaped = |block: &JsonValue| {
        get_block_type(block) == Some("json")
            && block
                .get("data")
                .is_some_and(|data| data.is_string() || !is_plain_value(plan, data))
    };
    if !blocks.iter().any(block_shaped) {
        return create_inner_content(blocks);
    }
    JsonValue::Array(
        blocks
            .iter()
            .filter_map(|block| match get_block_type(block) {
                _ if block_shaped(block) => Some(block.clone()),
                Some("unknown") => block.get("raw").cloned(),
                Some("json") => block.get("data").cloned(),
                _ => Some(block.clone()),
            })
            .collect(),
    )
}

/// One part as a tool result's whole content, where it encodes nothing beyond its value
/// ([`single_part_value`]): the value - or, where `listed_by_tool` says the tool returned a one-element list
/// holding exactly that value, that list.
///
/// **The tool's list is spelled with its bare element, `[value]`, never with the part.** The part,
/// `[{"type": "json", "data": X}]`, is what a producer that typed `X` writes, so re-reading it collapses to `X`;
/// had the list been spelled that way, reading a stored result again would have turned the tool's list into its
/// element. `[X]` re-reads as itself on every path, which is what makes the form canonical rather than a
/// function of how many times it was read.
pub(crate) fn canonical_single_part(
    plan: &crate::rules::content_blocks::ContentBlockPlan,
    block: &JsonValue,
    listed_by_tool: impl FnOnce(&JsonValue) -> bool,
) -> Option<JsonValue> {
    let value = single_part_value(plan, block)?;
    Some(if listed_by_tool(&value) {
        json!([value])
    } else {
        value
    })
}

/// Whether `source` is a one-element list holding exactly `value` - written as a list, or as the JSON text of
/// one, which normalisation decodes. A list of one is not its element: the tool returned the list.
fn listed_by_tool(source: &JsonValue, value: &JsonValue) -> bool {
    let holds = |items: &[JsonValue]| matches!(items, [only] if only == value);
    match source {
        JsonValue::Array(items) => holds(items),
        // Decoded only where the text can be a list: a long plain string is not parsed to find out.
        JsonValue::String(text) if text.trim_start().starts_with('[') => {
            serde_json::from_str::<Vec<JsonValue>>(text).is_ok_and(|items| holds(&items))
        }
        _ => false,
    }
}

/// Create inner content for tool_result from content blocks - the retired reduction, which the retired
/// provider readers still use and [`canonical_inner_content`] defers to for several parts.
/// Single text block becomes string, single json/unknown becomes raw data, multiple become array.
pub(crate) fn create_inner_content(blocks: &[JsonValue]) -> JsonValue {
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
        // A single json/unknown block is its data, whole: asked before the text reduction, which otherwise took
        // a structured value's `text` member and dropped the members beside it.
        if let Some(raw) = single_raw_data {
            return raw;
        }
        // A single text block is its string. Only an actual text block: the reduction is about rendering,
        // not about any object that happens to hold a `text` member.
        if get_block_type(&blocks[0]) == Some("text")
            && let Some(text) = inner[0].get("text").and_then(|t| t.as_str())
        {
            return json!(text);
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

    // The result as it was, with the merged content: rebuilding it from a fixed field list dropped the
    // `name` a Gemini-style result pairs by and the `provider_executed` that says whose execution it was.
    let mut merged = tool_result.as_object().cloned().unwrap_or_default();
    merged.insert("content".to_string(), json!(content_arr));
    merged
        .entry("is_error".to_string())
        .or_insert_with(|| json!(false));
    JsonValue::Object(merged)
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

/// The bare value a one-part tool result stands for, where the part encodes nothing beyond it.
///
/// A tool result's content is what the tool returned. A tool that returned one structured value returned that
/// value, and one that returned text returned that text: `[{"type": "json", "data": X}]` and
/// `[{"type": "text", "text": T}]` are layers around them, so one producer release wrote the bare value where
/// the next wrote the typed part, and the two read as different results. The collapse holds only where the
/// part says nothing more:
///
/// - a json part holds a **bare structured value** ([`is_bare_structured_value`]);
/// - a text part holds **only its text** - citations or any other member are statements a string cannot make.
///
/// Whether the producer typed the part or the tool listed the value is the caller's to judge, from the source:
/// see [`canonical_single_part`].
pub(crate) fn single_part_value(
    plan: &crate::rules::content_blocks::ContentBlockPlan,
    block: &JsonValue,
) -> Option<JsonValue> {
    match block.get("type").and_then(JsonValue::as_str)? {
        // Only a part holding nothing but its value: any other member is a statement the bare value would drop.
        "json" => {
            let object = block.as_object()?;
            let data = object.get("data")?;
            (object.len() == 2 && is_bare_structured_value(plan, data)).then(|| data.clone())
        }
        "text" => {
            let object = block.as_object()?;
            let text = object.get("text")?.as_str()?;
            (object.len() == 2).then(|| JsonValue::String(text.to_string()))
        }
        _ => None,
    }
}

/// Whether a tool's structured return value is stored as itself rather than as a `json` part: an object the
/// content chain reads as plain data - no string `type`, no member the vocabulary reads as a content block, and
/// no declared case that reads it as a block. A block-shaped object keeps its wrapper, because unwrapped it would
/// be re-read as the block it is spelled like - a tool returning `{"type": "text", "text": "..."}`, or an untyped
/// shape a case declares, such as a MIME type beside data. Asked of the chain itself, so the guard cannot drift
/// from the cases. A primitive keeps its part, because a json number is not text.
pub(crate) fn is_bare_structured_value(
    plan: &crate::rules::content_blocks::ContentBlockPlan,
    value: &JsonValue,
) -> bool {
    value.as_object().is_some_and(|object| {
        !object.get("type").is_some_and(JsonValue::is_string)
            && !plan.any_means_content_block(object.keys())
            && super::declared_block_in(plan, value, false).is_none()
    })
}

/// Data no case reads as a block, at any depth: a bare structured object ([`is_bare_structured_value`]), a
/// primitive, or a list of plain values. The one test every result path and the result identity share for
/// whether a json part's data may stand without its part. Judged to serde_json's own nesting limit, so a parsed
/// value is judged whole; deeper is not plain, which can only keep a part.
pub(crate) fn is_plain_value(
    plan: &crate::rules::content_blocks::ContentBlockPlan,
    value: &JsonValue,
) -> bool {
    fn plain(
        plan: &crate::rules::content_blocks::ContentBlockPlan,
        value: &JsonValue,
        depth: usize,
    ) -> bool {
        match value {
            JsonValue::Object(_) => is_bare_structured_value(plan, value),
            JsonValue::Array(items) => {
                depth < 128 && items.iter().all(|item| plain(plan, item, depth + 1))
            }
            _ => true,
        }
    }
    plain(plan, value, 0)
}
