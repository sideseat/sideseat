use super::*;

#[cfg(test)]
// ============================================================================
// INDEXED MESSAGE EXTRACTION
// ============================================================================
#[cfg(test)]
pub(super) fn extract_indices(attrs: &HashMap<String, String>, prefix: &str) -> BTreeSet<usize> {
    attrs
        .keys()
        .filter_map(|k| {
            k.strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('.'))
                .and_then(|rest| rest.split('.').next())
                .and_then(|idx| idx.parse().ok())
        })
        .collect()
}

#[cfg(test)]
pub(super) fn extract_indexed_message(
    attrs: &HashMap<String, String>,
    prefix: &str,
    idx: usize,
    timestamp: DateTime<Utc>,
) -> Option<RawMessage> {
    let msg_prefix = format!("{}.{}", prefix, idx);

    // Check for content - either direct (gen_ai.prompt.0.content)
    // or nested (gen_ai.prompt.0.content.0.text)
    let content_prefix = format!("{}.content", msg_prefix);
    let has_content = attrs.contains_key(&content_prefix)
        || attrs
            .keys()
            .any(|k| k.starts_with(&format!("{}.", content_prefix)));

    if !has_content {
        return None;
    }

    // Collect all raw attributes with this prefix (literal, no metadata)
    let mut raw = serde_json::Map::new();
    let attr_prefix = format!("{}.", msg_prefix);
    for (key, value) in attrs {
        if let Some(suffix) = key.strip_prefix(&attr_prefix) {
            let json_val = if value.starts_with('{') || value.starts_with('[') {
                parse_json_with_fallback(value, &format!("indexed.{}", key))
            } else {
                json!(value)
            };
            raw.insert(suffix.to_string(), json_val);
        }
    }

    Some(RawMessage::from_attr(
        &msg_prefix,
        timestamp,
        JsonValue::Object(raw),
    ))
}

#[cfg(test)]
pub(super) fn extract_openinference_message(
    attrs: &HashMap<String, String>,
    prefix: &str,
    idx: usize,
    timestamp: DateTime<Utc>,
) -> Option<RawMessage> {
    let item_prefix = format!("{}.{}", prefix, idx);
    let msg_prefix = format!("{}.message", item_prefix);

    // Check role exists
    attrs.get(&format!("{}.role", msg_prefix))?;

    // Check if message has meaningful content:
    // - Direct content (llm.input_messages.0.message.content)
    // - Nested content blocks (llm.input_messages.0.message.contents.0.*)
    // - Tool calls (llm.input_messages.0.message.tool_calls.*)
    // - Tool call ID (for tool result messages)
    // - Function call (legacy format)
    let content_key = format!("{}.content", msg_prefix);
    let contents_prefix = format!("{}.contents", msg_prefix);
    let tool_calls_prefix = format!("{}.tool_calls", msg_prefix);
    let tool_call_id_key = format!("{}.tool_call_id", msg_prefix);
    let function_call_prefix = format!("{}.function_call", msg_prefix);

    let has_content = attrs.contains_key(&content_key)
        || attrs
            .keys()
            .any(|k| k.starts_with(&format!("{}.", contents_prefix)))
        || attrs
            .keys()
            .any(|k| k.starts_with(&format!("{}.", tool_calls_prefix)))
        || attrs.contains_key(&tool_call_id_key)
        || attrs
            .keys()
            .any(|k| k.starts_with(&format!("{}.", function_call_prefix)));

    if !has_content {
        return None;
    }

    // Collect all raw attributes (literal, no metadata)
    let mut raw = serde_json::Map::new();

    // Collect message.* attributes including nested content blocks
    let attr_prefix = format!("{}.", msg_prefix);
    for (key, value) in attrs {
        if let Some(suffix) = key.strip_prefix(&attr_prefix) {
            let json_val = if value.starts_with('{') || value.starts_with('[') {
                parse_json_with_fallback(value, &format!("openinference.{}", key))
            } else {
                json!(value)
            };
            raw.insert(suffix.to_string(), json_val);
        }
    }

    // Also collect ALL item-level attributes (not just message.*)
    let item_attr_prefix = format!("{}.", item_prefix);
    for (key, value) in attrs {
        if let Some(suffix) = key.strip_prefix(&item_attr_prefix) {
            // Skip message.* as we already collected those above
            if suffix.starts_with("message.") {
                continue;
            }
            let json_val = if value.starts_with('{') || value.starts_with('[') {
                parse_json_with_fallback(value, &format!("openinference.{}", key))
            } else {
                json!(value)
            };
            raw.insert(suffix.to_string(), json_val);
        }
    }

    Some(RawMessage::from_attr(
        &msg_prefix,
        timestamp,
        JsonValue::Object(raw),
    ))
}

/// Extract OpenInference documents (retrieval.documents.N.* or reranker.*.documents.N.*)
#[cfg(test)]
pub(super) fn extract_openinference_documents(
    attrs: &HashMap<String, String>,
    prefix: &str,
    indices: &BTreeSet<usize>,
    timestamp: DateTime<Utc>,
) -> Option<RawMessage> {
    let mut documents = Vec::new();

    for &idx in indices {
        let doc_prefix = format!("{}.{}.document", prefix, idx);
        let mut doc = serde_json::Map::new();

        // Extract document.id
        if let Some(id) = attrs.get(&format!("{}.id", doc_prefix)) {
            doc.insert("id".to_string(), json!(id));
        }

        // Extract document.content
        if let Some(content) = attrs.get(&format!("{}.content", doc_prefix)) {
            doc.insert("content".to_string(), json!(content));
        }

        // Extract document.score
        if let Some(score) = attrs.get(&format!("{}.score", doc_prefix)) {
            if let Ok(score_f64) = score.parse::<f64>() {
                doc.insert("score".to_string(), json!(score_f64));
            } else {
                doc.insert("score".to_string(), json!(score));
            }
        }

        // Extract document.metadata (JSON)
        if let Some(metadata) = attrs.get(&format!("{}.metadata", doc_prefix)) {
            if let Ok(parsed) = serde_json::from_str::<JsonValue>(metadata) {
                doc.insert("metadata".to_string(), parsed);
            } else {
                doc.insert("metadata".to_string(), json!(metadata));
            }
        }

        if !doc.is_empty() {
            documents.push(JsonValue::Object(doc));
        }
    }

    if documents.is_empty() {
        return None;
    }

    let mut msg = serde_json::Map::new();
    msg.insert("role".to_string(), json!("documents"));
    msg.insert("content".to_string(), JsonValue::Array(documents));
    msg.insert("_source".to_string(), json!(prefix));

    Some(RawMessage::from_attr(
        prefix,
        timestamp,
        JsonValue::Object(msg),
    ))
}
