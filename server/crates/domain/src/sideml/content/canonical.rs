use serde_json::Value as JsonValue;

/// Clean nested canonical SideML blocks without reinterpreting the value a tool returned.
///
/// A canonical tool result may legitimately contain a scalar or arbitrary structured JSON. Those
/// values are already in SideML and must remain byte-for-byte equivalent. Typed nested blocks are
/// rebuilt through the canonical passthrough so producer-only members such as AgentScope object
/// ids and timestamps do not leak into the public result.
pub(super) fn normalize_tool_result_content(content: Option<&JsonValue>) -> JsonValue {
    match content {
        None => JsonValue::Null,
        Some(JsonValue::Array(items)) => {
            JsonValue::Array(items.iter().map(normalize_nested_block).collect())
        }
        Some(value) => normalize_nested_block(value),
    }
}

fn normalize_nested_block(value: &JsonValue) -> JsonValue {
    super::try_sideml_passthrough(value).unwrap_or_else(|| value.clone())
}
