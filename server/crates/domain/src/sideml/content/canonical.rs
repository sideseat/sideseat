use serde_json::Value as JsonValue;

/// Clean nested canonical SideML blocks without reinterpreting the value a tool returned.
///
/// A canonical tool result may legitimately contain a scalar or arbitrary structured JSON. Those
/// values are already in SideML and must remain byte-for-byte equivalent. Typed nested blocks are
/// rebuilt through the canonical passthrough so producer-only members such as AgentScope object
/// ids and timestamps do not leak into the public result.
pub(super) fn normalize_tool_result_content(
    plan: &crate::rules::content_blocks::ContentBlockPlan,
    content: Option<&JsonValue>,
) -> JsonValue {
    match content {
        None => JsonValue::Null,
        Some(JsonValue::Array(items)) => {
            let blocks: Vec<JsonValue> = items
                .iter()
                .map(|item| normalize_nested_block(plan, item))
                .collect();
            // The canonical tool result holds here as on every other path: one typed part that encodes
            // nothing beyond its value is the value, judged against the part as the producer wrote it.
            if let ([block], [source]) = (blocks.as_slice(), items.as_slice())
                && let Some(content) =
                    super::canonical_single_part(plan, block, |value| source == value)
            {
                return content;
            }
            JsonValue::Array(blocks)
        }
        Some(value) => normalize_nested_block(plan, value),
    }
}

fn normalize_nested_block(
    plan: &crate::rules::content_blocks::ContentBlockPlan,
    value: &JsonValue,
) -> JsonValue {
    if let Some(block) = super::try_sideml_passthrough(plan, value) {
        return block;
    }
    // A typed block carrying a member the vocabulary marks as a producer's own shape is that producer's,
    // and the passthrough deliberately declined it: the declared case reads it - a text's citations, a
    // thinking block's chunks - where cloning kept the provider form and the canonical members never
    // appeared. Anything else stays byte-exact, which is what canonical passthrough means.
    if value.get("type").is_some_and(JsonValue::is_string)
        && value
            .as_object()
            .is_some_and(|object| plan.any_marks_producer_shape(object.keys()))
        && let Some(read) = super::normalize_block_in(plan, value, false)
    {
        return read;
    }
    value.clone()
}
