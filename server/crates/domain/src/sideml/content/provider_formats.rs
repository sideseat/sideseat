use super::*;

// ========== Provider-specific content format handlers ==========

/// Type-tagged format (OpenAI and compatible providers).
/// Handles: text, image_url, input_audio, audio, refusal, output_json, thinking, etc.
pub(super) fn try_openai_format(block: &JsonValue) -> Option<JsonValue> {
    let block_type = block.get("type")?.as_str()?;
    match block_type {
        "text" | "input_text" | "output_text" => {
            // Standard: "text" field (OpenAI, Anthropic, etc.)
            // Agent Framework uses "content" field instead of "text"
            let text = block
                .get("text")
                .or_else(|| block.get("content"))
                .and_then(|t| t.as_str())?;
            Some(json!({"type": "text", "text": text}))
        }
        // Agent Framework binary data: {"type": "data", "uri": "#!B64!#...", "media_type": "...", "filename"?: "..."}
        "data" => {
            let uri = block.get("uri").and_then(|u| u.as_str())?;
            let media_type = block.get("media_type").and_then(|m| m.as_str());
            let content_type = mime_to_content_type(media_type.unwrap_or(""));
            let mut result = json!({
                "type": content_type,
                "media_type": media_type,
                "source": if files::is_file_uri(uri) { "file" } else { "url" },
                "data": uri
            });
            if let Some(name) = block.get("filename").and_then(|n| n.as_str()) {
                result["name"] = json!(name);
            }
            Some(result)
        }
        // Agent Framework tool call: {"type": "tool_call", "id": "...", "name": "...", "arguments": {...}}
        // arguments may be a JSON-encoded string or an object
        "tool_call" => {
            let name = block.get("name").and_then(|n| n.as_str())?;
            let id = block.get("id").and_then(|i| i.as_str());
            let input = block
                .get("arguments")
                .map(|a| {
                    if let Some(s) = a.as_str() {
                        serde_json::from_str::<JsonValue>(s).unwrap_or_else(|_| json!(s))
                    } else {
                        a.clone()
                    }
                })
                .unwrap_or(json!({}));
            Some(json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input
            }))
        }
        // Agent Framework tool result: {"type": "tool_call_response", "id": "...", "response": "..."}
        // response is json.dumps(result) — may be a JSON string of an object, array, or plain string
        "tool_call_response" => {
            let id = block.get("id").and_then(|i| i.as_str());
            let response = block.get("response");
            let content = if let Some(s) = response.and_then(|r| r.as_str()) {
                match serde_json::from_str::<JsonValue>(s) {
                    // Object or array → render as json block
                    Ok(v @ (JsonValue::Object(_) | JsonValue::Array(_))) => {
                        try_normalize_python_constructor_content(&v)
                            .unwrap_or_else(|| json!([{"type": "json", "data": v}]))
                    }
                    // JSON-encoded string (e.g. json.dumps("text result")) → unwrap to text
                    Ok(JsonValue::String(text)) => {
                        let value = JsonValue::String(text.clone());
                        try_normalize_python_constructor_content(&value)
                            .unwrap_or_else(|| json!([{"type": "text", "text": text}]))
                    }
                    // Non-parseable or scalar → plain text
                    _ => {
                        let value = JsonValue::String(s.to_string());
                        try_normalize_python_constructor_content(&value)
                            .unwrap_or_else(|| json!([{"type": "text", "text": s}]))
                    }
                }
            } else if let Some(v) = response {
                json!([{"type": "json", "data": v}])
            } else {
                json!([])
            };
            Some(json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": content,
                "is_error": false
            }))
        }
        "image_url" | "input_image" => {
            let image_obj = block.get("image_url")?;
            let url = image_obj
                .get("url")
                .or(Some(image_obj))
                .and_then(|u| u.as_str())?;
            let (source, data, media_type) = parse_data_url(url);
            // Preserve detail field (affects token usage: "auto", "low", "high")
            let detail = image_obj.get("detail").and_then(|d| d.as_str());
            let mut result =
                json!({"type": "image", "media_type": media_type, "source": source, "data": data});
            if let Some(d) = detail {
                result["detail"] = json!(d);
            }
            Some(result)
        }
        // semconv 1.37 reasoning part. Without this it fell through to the unknown
        // passthrough and rendered as {"type":"unknown"} instead of a thinking block.
        "reasoning" => {
            let text = block
                .get("text")
                .or_else(|| block.get("content"))
                .and_then(|t| t.as_str())?;
            Some(json!({"type": "thinking", "text": text, "signature": null}))
        }
        // semconv 1.37 binary part: same shape as Agent Framework's "data", but the
        // payload lives under `data` rather than `uri`.
        "blob" => {
            let raw = block
                .get("data")
                .or_else(|| block.get("uri"))
                .and_then(|u| u.as_str())?;
            let media_type = block
                .get("media_type")
                .or_else(|| block.get("mime_type"))
                .and_then(|m| m.as_str())
                .unwrap_or("application/octet-stream");
            // parse_data_url returns the media type only when the payload carried one;
            // fall back to the block's own field.
            let (source, data, parsed_media) = parse_data_url(raw);
            Some(json!({
                "type": "document",
                "media_type": parsed_media.as_deref().unwrap_or(media_type),
                "source": source,
                "data": data
            }))
        }
        // Audio blocks: input_audio, audio (same pattern)
        "input_audio" | "audio" => {
            let audio_field = if block_type == "input_audio" {
                "input_audio"
            } else {
                "audio"
            };
            let audio = block.get(audio_field)?;
            let data = audio.get("data")?.as_str()?;
            let format = audio.get("format").and_then(|f| f.as_str());

            // Check if data was replaced with #!B64!# file reference
            let source_type = if files::is_file_uri(data) {
                "file"
            } else {
                "base64"
            };

            Some(json!({
                "type": "audio",
                "media_type": build_media_type(format, "audio"),
                "source": source_type,
                "data": data
            }))
        }
        // The Responses API's `input_file` holds its members flat; Chat Completions' `file` part holds the
        // same members under `file`. A `file` block without that object is another dialect's, so it is
        // left to the rest of the chain.
        "input_file" | "file" => {
            let block = if block_type == "file" {
                block.get("file").filter(|file| file.is_object())?
            } else {
                block
            };
            if let Some(data_str) = block.get("file_data").and_then(|d| d.as_str()) {
                let (source, data, media_type) = parse_data_url(data_str);
                let content_type = media_type
                    .as_deref()
                    .map(mime_to_content_type)
                    .unwrap_or("file");
                let mut result = json!({
                    "type": content_type,
                    "source": source,
                    "data": data,
                    "media_type": media_type
                });
                if let Some(name) = block.get("filename").and_then(|n| n.as_str()) {
                    result["name"] = json!(name);
                }
                Some(result)
            } else if let Some(url) = block.get("file_url").and_then(|u| u.as_str()) {
                Some(json!({"type": "file", "source": "url", "data": url}))
            } else {
                block
                    .get("file_id")
                    .and_then(|id| id.as_str())
                    .map(|file_id| json!({"type": "file", "source": "file_id", "data": file_id}))
            }
        }
        "refusal" => {
            // Handle both {"type": "refusal", "message": "..."} and {"type": "refusal", "refusal": "..."}
            let message = block
                .get("message")
                .or_else(|| block.get("refusal"))
                .and_then(|m| m.as_str())?;
            Some(json!({"type": "refusal", "message": message}))
        }
        "output_json" | "json_object" => {
            let json_data = block.get("json").cloned().unwrap_or(json!({}));
            Some(json!({"type": "json", "data": json_data}))
        }
        // Claude extended thinking / Mistral reasoning / PydanticAI
        "thinking" => {
            // Extract thinking content from various provider formats
            let text = extract_thinking_text(block);
            let signature = block.get("signature").and_then(|s| s.as_str());
            Some(json!({
                "type": "thinking",
                "text": text,
                "signature": signature
            }))
        }
        // Claude redacted thinking (when thinking is not exposed)
        "redacted_thinking" => {
            let data = block.get("data").and_then(|d| d.as_str()).unwrap_or("");
            Some(json!({
                "type": "redacted_thinking",
                "data": data
            }))
        }
        _ => None,
    }
}

/// Anthropic format: {"type": "text|image|document|tool_use|tool_result", ...}
/// Also handles Strands JS SDK camelCase variants: {"type": "toolUse|toolResult", ...}
pub(super) fn try_anthropic_format(block: &JsonValue) -> Option<JsonValue> {
    let block_type = block.get("type")?.as_str()?;
    match block_type {
        // Media blocks: image, document (same pattern with source object)
        "image" | "document" => try_anthropic_media(block, block_type),
        "tool_use" => Some(json!({
            "type": "tool_use",
            "id": block.get("id"),
            "name": block.get("name"),
            "input": block.get("input").cloned().unwrap_or(json!({}))
        })),
        // Strands JS SDK flat camelCase format: {"type": "toolUse", "toolUseId": "...", "name": "...", "input": {...}}
        "toolUse" => Some(json!({
            "type": "tool_use",
            "id": block.get("toolUseId"),
            "name": block.get("name"),
            "input": block.get("input").cloned().unwrap_or(json!({}))
        })),
        "tool_result" => {
            let raw_content = block.get("content").cloned();
            let normalized_content = normalize_tool_result_content(raw_content);
            Some(json!({
                "type": "tool_result",
                "tool_use_id": block.get("tool_use_id"),
                "content": normalized_content,
                "is_error": block.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false)
            }))
        }
        // Strands JS SDK flat camelCase format: {"type": "toolResult", "toolUseId": "...", "content": [...], "status": "..."}
        "toolResult" => {
            let raw_content = block.get("content").cloned();
            let normalized_content = normalize_tool_result_content(raw_content);
            Some(json!({
                "type": "tool_result",
                "tool_use_id": block.get("toolUseId"),
                "content": normalized_content,
                "is_error": block.get("status").and_then(|s| s.as_str()) == Some("error")
            }))
        }
        _ => None, // "text" handled by try_openai_format
    }
}

/// Extract Anthropic media block (image, document).
pub(super) fn try_anthropic_media(block: &JsonValue, block_type: &str) -> Option<JsonValue> {
    let source = block.get("source")?;
    let source_type = source.get("type")?.as_str()?;
    let (src, data) = match source_type {
        "base64" => {
            let data = source.get("data")?.as_str()?;
            // Check if data was replaced with #!B64!# file reference
            if files::is_file_uri(data) {
                ("file", data)
            } else {
                ("base64", data)
            }
        }
        "url" => ("url", source.get("url")?.as_str()?),
        _ => return None,
    };
    Some(json!({
        "type": block_type,
        "media_type": source.get("media_type").and_then(|m| m.as_str()),
        "source": src,
        "data": data
    }))
}

/// Bedrock/Strands format: {"text": "..."}, {"image": {...}}, {"toolUse": {...}}, {"reasoningContent": {...}}
pub(super) fn try_bedrock_format(block: &JsonValue) -> Option<JsonValue> {
    // Text block: STRICT match - must be exactly {"text": "..."} with no other fields
    // This prevents structured output like {"text": "summary", "confidence": 0.9} from
    // being incorrectly parsed as just a text block (which would lose the confidence field)
    if let Some(obj) = block.as_object()
        && obj.len() == 1
        && let Some(text) = obj.get("text").and_then(|t| t.as_str())
    {
        return Some(json!({"type": "text", "text": text}));
    }
    // A tool result's structured value: exactly `{"json": ...}`, which Converse writes for a result that is
    // not text. Read as plain data, the value came back wrapped in the member that only labels it.
    if let Some(obj) = block.as_object()
        && obj.len() == 1
        && let Some(data) = obj.get("json")
    {
        return Some(json!({"type": "json", "data": data}));
    }

    // Bedrock extended thinking (reasoningContent)
    // Format: {"reasoningContent": {"reasoningText": {"text": "...", "signature": "..."}}}
    if let Some(reasoning) = block.get("reasoningContent") {
        // Primary format: reasoningText with text content
        if let Some(reasoning_text) = reasoning.get("reasoningText") {
            let text = reasoning_text
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let signature = reasoning_text.get("signature").and_then(|s| s.as_str());
            return Some(json!({
                "type": "thinking",
                "text": text,
                "signature": signature
            }));
        }
        // Redacted thinking variant
        if let Some(redacted) = reasoning.get("redactedContent") {
            let data = redacted.get("data").and_then(|d| d.as_str()).unwrap_or("");
            return Some(json!({
                "type": "redacted_thinking",
                "data": data
            }));
        }
        // Future-proofing: unknown reasoningContent variant
        // Preserve raw content so we don't silently lose data
        return Some(json!({
            "type": "unknown",
            "raw": block.clone()
        }));
    }

    // Media blocks: image, document, video (all follow same pattern)
    if let Some(result) = try_bedrock_media(block, "image", "image") {
        return Some(result);
    }
    if let Some(result) = try_bedrock_media(block, "document", "application") {
        return Some(result);
    }
    if let Some(result) = try_bedrock_media(block, "video", "video") {
        return Some(result);
    }
    // Tool use (Strands/Bedrock native)
    if let Some(tool_use) = block.get("toolUse") {
        return Some(json!({
            "type": "tool_use",
            "id": tool_use.get("toolUseId"),
            "name": tool_use.get("name"),
            "input": tool_use.get("input").cloned().unwrap_or(json!({}))
        }));
    }
    // Tool result
    if let Some(tool_result) = block.get("toolResult") {
        let raw_content = tool_result.get("content").cloned();
        // The same canonical form a tool-role message's content takes, so one result reads alike whether a
        // producer wrote it as a Converse block or as a tool message: a lone text is its string, a lone
        // structured value is that value.
        let normalized_content = match normalize_tool_result_content(raw_content) {
            JsonValue::Array(blocks) => super::tool_result::create_inner_content(&blocks),
            other => other,
        };
        return Some(json!({
            "type": "tool_result",
            "tool_use_id": tool_result.get("toolUseId"),
            "content": normalized_content,
            "is_error": tool_result.get("status").and_then(|s| s.as_str()) == Some("error")
        }));
    }
    None
}

/// Extract Bedrock media block (image, document, video).
pub(super) fn try_bedrock_media(
    block: &JsonValue,
    field: &str,
    mime_prefix: &str,
) -> Option<JsonValue> {
    let media = block.get(field)?;
    let format = media.get("format").and_then(|f| f.as_str());
    let source = media.get("source")?;
    let data = source.get("bytes")?.as_str()?;

    // Check if data was replaced with #!B64!# file reference
    let source_type = if files::is_file_uri(data) {
        "file"
    } else {
        "base64"
    };

    let mut result = json!({
        "type": field,
        "media_type": build_media_type(format, mime_prefix),
        "source": source_type,
        "data": data
    });
    // Document can have a name field
    if field == "document"
        && let Some(name) = media.get("name").and_then(|n| n.as_str())
    {
        result["name"] = json!(name);
    }
    Some(result)
}

/// Gemini format: {"text": "..."}, {"inline_data": {...}}, {"file_data": {...}}, {"functionCall": {...}}, {"thinking": "..."}
pub(super) fn try_gemini_format(block: &JsonValue) -> Option<JsonValue> {
    // Gemini thinking part: {"thinking": "..."}
    // Similar to text parts {"text": "..."} but for extended thinking
    // STRICT match: must be exactly {"thinking": "..."} with no other fields
    if let Some(obj) = block.as_object()
        && obj.len() == 1
        && let Some(thinking) = obj.get("thinking").and_then(|t| t.as_str())
    {
        return Some(json!({"type": "thinking", "text": thinking, "signature": null}));
    }

    // Gemini/ADK thought block: {"text": "...", "thought": true}
    // ADK sends thinking content as text blocks with a thought flag
    if let Some(obj) = block.as_object()
        && obj
            .get("thought")
            .is_some_and(|t| t.as_bool().unwrap_or(false))
        && let Some(text) = obj.get("text").and_then(|t| t.as_str())
    {
        return Some(json!({"type": "thinking", "text": text, "signature": null}));
    }

    // inline_data (base64)
    if let Some(inline) = block.get("inline_data") {
        let mime = inline.get("mime_type")?.as_str()?;
        let data = inline.get("data")?.as_str()?;
        let block_type = mime_to_content_type(mime);

        // Check if data was replaced with #!B64!# file reference
        let source_type = if files::is_file_uri(data) {
            "file"
        } else {
            "base64"
        };

        return Some(json!({
            "type": block_type,
            "media_type": mime,
            "source": source_type,
            "data": data
        }));
    }
    // file_data (URL)
    if let Some(file) = block.get("file_data") {
        let mime = file.get("mime_type")?.as_str()?;
        let uri = file.get("file_uri")?.as_str()?;
        let block_type = mime_to_content_type(mime);
        return Some(json!({
            "type": block_type,
            "media_type": mime,
            "source": "url",
            "data": uri
        }));
    }
    // functionCall / function_call (Gemini tool use - both camelCase and snake_case)
    // Gemini doesn't provide tool call IDs, so we generate synthetic IDs based on
    // function name + args hash to prevent deduplication collisions when the same
    // function is called multiple times with different arguments.
    if let Some(fc) = block
        .get("functionCall")
        .or_else(|| block.get("function_call"))
    {
        let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
        let args = fc.get("args").cloned().unwrap_or(json!({}));
        let args_hash = compute_short_hash(&args);
        let synthetic_id = format!("gemini_{name}_call_{args_hash}");

        return Some(json!({
            "type": "tool_use",
            "id": synthetic_id,
            "name": name,
            "input": args
        }));
    }
    // functionResponse / function_response (Gemini tool result - both cases)
    // Gemini doesn't provide tool call IDs, so we generate synthetic IDs based on
    // function name + response hash to prevent deduplication collisions when the same
    // function returns different results.
    if let Some(fr) = block
        .get("functionResponse")
        .or_else(|| block.get("function_response"))
    {
        let name = fr.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
        let raw_content = fr.get("response").cloned();
        let normalized_content = normalize_tool_result_content(raw_content);
        // No `tool_use_id`: Gemini supplies none, and deriving one from the RESPONSE (as this
        // did) produced an id no call ever had, so every ADK tool result was permanently
        // dangling - correlating by it found nothing. The name is carried instead and the
        // feed's correlation stage ties the result to its call, which is the only place both
        // halves are visible at once.
        return Some(json!({
            "type": "tool_result",
            "name": name,
            "content": normalized_content,
            "is_error": false
        }));
    }
    None
}

/// Vercel AI SDK formats:
///
/// 1. Typed blocks: `{"type": "tool-call"|"tool-result"|"json"|"text", ...}`
/// 2. Aggregated response: `{"content": "...", "finishReason": "stop", "role": "assistant"}`
///
/// Vercel AI uses hyphenated type names and camelCase field names.
/// Retired: declared in `rules/vocabulary/content-blocks-vercel.json`, at the `after_provider_formats` position. Kept as
/// the equivalence oracle - `the_declared_dialect_blocks_match_the_reader_they_replace` runs both over every
/// form it recognised and every shape where it declined.
#[cfg(test)]
pub(super) fn try_vercel_format(block: &JsonValue) -> Option<JsonValue> {
    // Try typed block format first
    if let Some(block_type) = block.get("type").and_then(|t| t.as_str()) {
        return match block_type {
            // Tool call: {"type": "tool-call", "toolCallId": "...", "toolName": "...", "input": {...}}
            "tool-call" => {
                let id = block.get("toolCallId").and_then(|v| v.as_str());
                let name = block.get("toolName").and_then(|v| v.as_str())?;
                let input = block.get("input").cloned().unwrap_or(json!({}));
                // Also check for "args" field (alternative format)
                let input = if input == json!({}) {
                    block.get("args").cloned().unwrap_or(json!({}))
                } else {
                    input
                };
                Some(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input
                }))
            }
            // Tool result: {"type": "tool-result", "toolCallId": "...", "result": {...}}
            "tool-result" => {
                let tool_use_id = block.get("toolCallId").and_then(|v| v.as_str());
                let raw_content = block.get("result").or_else(|| block.get("output")).cloned();
                let normalized_content = normalize_tool_result_content(raw_content);
                let is_error = block
                    .get("isError")
                    .or_else(|| block.get("is_error"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Some(json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": normalized_content,
                    "is_error": is_error
                }))
            }
            // JSON content block: {"type": "json", "value": {...}}
            "json" => {
                let data = block.get("value").cloned().unwrap_or(json!({}));
                Some(json!({"type": "json", "data": data}))
            }
            // Text content block: {"type": "text", "value": "..."}
            "text" if block.get("value").is_some() => {
                let text = block.get("value").and_then(|v| v.as_str())?;
                Some(json!({"type": "text", "text": text}))
            }
            // File content block: {"type": "file", "mediaType": "image/jpeg", "data": "..."}
            // Also handles "mimeType" variant (older API versions)
            "file" => {
                let media_type = block
                    .get("mediaType")
                    .or_else(|| block.get("mimeType"))
                    .and_then(|m| m.as_str())?;
                let data = block.get("data")?.as_str()?;

                // Determine content type from MIME type
                let content_type = mime_to_content_type(media_type);

                // Determine source type (file reference or base64)
                let source_type = if files::is_file_uri(data) {
                    "file"
                } else {
                    "base64"
                };

                Some(json!({
                    "type": content_type,
                    "media_type": media_type,
                    "source": source_type,
                    "data": data
                }))
            }
            _ => None,
        };
    }

    // Try aggregated response format: {"content": "...", "finishReason": "stop", "role": "assistant"}
    let obj = block.as_object()?;
    let content = obj.get("content").and_then(|v| v.as_str())?;
    let has_vercel_fields = obj.contains_key("finishReason")
        || obj.contains_key("providerMetadata")
        || obj.get("role").and_then(|v| v.as_str()) == Some("assistant");

    if has_vercel_fields {
        return Some(json!({"type": "text", "text": content}));
    }

    None
}
