use super::*;

// ---------------------------------------------------------------------------
// Request formatting helpers
// ---------------------------------------------------------------------------

pub(super) fn format_contents(messages: &[Message]) -> Result<Value, ProviderError> {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        let role = match &msg.role {
            Role::System => continue, // System handled via systemInstruction
            Role::User | Role::Tool => "user",
            Role::Assistant => "model",
            Role::Other(s) => s.as_str(),
        };

        let parts = format_parts(&msg.content)?;
        result.push(json!({"role": role, "parts": parts}));
    }

    Ok(json!(result))
}

fn format_parts(blocks: &[ContentBlock]) -> Result<Value, ProviderError> {
    let parts: Result<Vec<Value>, _> = blocks.iter().map(format_part).collect();
    Ok(json!(parts?))
}

fn format_part(block: &ContentBlock) -> Result<Value, ProviderError> {
    match block {
        ContentBlock::Text(t) => Ok(json!({"text": t.text})),
        ContentBlock::Image(img) => format_image_part(img),
        ContentBlock::Audio(audio) => format_audio_part(audio),
        ContentBlock::Video(video) => format_video_part(video),
        ContentBlock::Document(doc) => format_document_part(doc),
        ContentBlock::ToolUse(tu) => Ok(json!({
            "functionCall": {
                "name": tu.name,
                "args": tu.input,
            }
        })),
        ContentBlock::ToolResult(tr) => {
            // Gemini tool results: functionResponse part
            let content = tr
                .content
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::Text(t) = b {
                        Some(t.text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(json!({
                "functionResponse": {
                    "name": tr.tool_use_id, // Gemini uses function name, not ID
                    "response": {"result": content},
                }
            }))
        }
        ContentBlock::Thinking(th) => Ok(json!({"text": th.text, "thought": true})),
    }
}

fn format_image_part(img: &ImageContent) -> Result<Value, ProviderError> {
    match &img.source {
        MediaSource::Base64(b64) => Ok(json!({
            "inlineData": {
                "mimeType": b64.media_type,
                "data": b64.data,
            }
        })),
        MediaSource::FileUri { uri, media_type } => Ok(json!({
            "fileData": {
                "mimeType": media_type,
                "fileUri": uri,
            }
        })),
        MediaSource::S3(s3) => Ok(json!({
            "fileData": {
                "fileUri": s3.uri,
            }
        })),
        MediaSource::Url(url) => {
            // Vertex AI supports HTTP URLs for images
            Ok(json!({
                "fileData": {
                    "fileUri": url,
                }
            }))
        }
        _ => Err(ProviderError::Unsupported(
            "Unsupported image source type for Gemini".into(),
        )),
    }
}

fn format_audio_part(audio: &AudioContent) -> Result<Value, ProviderError> {
    match &audio.source {
        MediaSource::Base64(b64) => Ok(json!({
            "inlineData": {
                "mimeType": b64.media_type,
                "data": b64.data,
            }
        })),
        MediaSource::FileUri { uri, media_type } => Ok(json!({
            "fileData": {"mimeType": media_type, "fileUri": uri}
        })),
        MediaSource::Url(url) => Ok(json!({"fileData": {"fileUri": url}})),
        _ => Err(ProviderError::Unsupported(
            "Unsupported audio source type for Gemini".into(),
        )),
    }
}

fn format_video_part(video: &VideoContent) -> Result<Value, ProviderError> {
    match &video.source {
        MediaSource::Base64(b64) => Ok(json!({
            "inlineData": {"mimeType": b64.media_type, "data": b64.data}
        })),
        MediaSource::FileUri { uri, media_type } => Ok(json!({
            "fileData": {"mimeType": media_type, "fileUri": uri}
        })),
        MediaSource::Url(url) => Ok(json!({"fileData": {"fileUri": url}})),
        _ => Err(ProviderError::Unsupported(
            "Unsupported video source type for Gemini".into(),
        )),
    }
}

fn format_document_part(doc: &DocumentContent) -> Result<Value, ProviderError> {
    match &doc.source {
        MediaSource::Base64(b64) => Ok(json!({
            "inlineData": {"mimeType": b64.media_type, "data": b64.data}
        })),
        MediaSource::FileUri { uri, media_type } => Ok(json!({
            "fileData": {"mimeType": media_type, "fileUri": uri}
        })),
        _ => Err(ProviderError::Unsupported(
            "Gemini documents require base64 or Files API URI".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Response parsing helpers
// ---------------------------------------------------------------------------

pub(super) fn parse_gemini_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let candidates = json["candidates"].as_array().ok_or_else(|| {
        ProviderError::Serialization("Missing 'candidates' in Gemini response".into())
    })?;

    if candidates.is_empty() {
        return Err(ProviderError::Api {
            status: 0,
            message: "Empty candidates in Gemini response".into(),
        });
    }

    let candidate = &candidates[0];
    // `content.parts` may be absent for SAFETY blocks, MAX_TOKENS hit before any output, or
    // recitation stops. Treat as empty rather than erroring — the caller gets an empty Response
    // with the correct stop_reason (Safety, Length, etc.).
    let empty = vec![];
    let parts = candidate["content"]["parts"].as_array().unwrap_or(&empty);

    let mut content: Vec<ContentBlock> = Vec::new();
    for part in parts {
        if let Some(text) = part["text"].as_str() {
            let is_thought = part["thought"].as_bool().unwrap_or(false);
            if is_thought {
                content.push(ContentBlock::Thinking(crate::types::ThinkingBlock {
                    text: text.to_string(),
                    signature: None,
                }));
            } else {
                content.push(ContentBlock::text(text));
            }
        } else if let Some(func_call) = part.get("functionCall") {
            let name = func_call["name"].as_str().unwrap_or("").to_string();
            let args = func_call["args"].clone();
            let tool_idx = content
                .iter()
                .filter(|b| matches!(b, ContentBlock::ToolUse(_)))
                .count()
                + 1;
            let id = format!("gemini_fc_{}", tool_idx);
            content.push(ContentBlock::ToolUse(ToolUseBlock {
                id,
                name,
                input: args,
            }));
        }
    }

    let finish_reason = candidate["finishReason"].as_str().unwrap_or("STOP");
    let stop_reason = parse_gemini_finish_reason(finish_reason);
    let usage = parse_gemini_usage(&json["usageMetadata"]);
    let model = json["modelVersion"].as_str().map(|s| s.to_string());
    let grounding_metadata = parse_grounding_metadata(&candidate["groundingMetadata"]);

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model,
        id: None,
        container: None,
        logprobs: None,
        grounding_metadata,
        warnings: vec![],
    })
}

fn parse_grounding_metadata(val: &Value) -> Option<GroundingMetadata> {
    if val.is_null() || !val.is_object() {
        return None;
    }
    let chunks = val["groundingChunks"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|c| GroundingChunk {
                    title: c["web"]["title"].as_str().map(|s| s.to_string()),
                    uri: c["web"]["uri"].as_str().map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();
    let search_queries = val["webSearchQueries"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|q| q.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    Some(GroundingMetadata {
        chunks,
        search_queries,
    })
}

pub(super) fn parse_gemini_usage(usage: &Value) -> Usage {
    Usage {
        input_tokens: usage["promptTokenCount"].as_u64().unwrap_or(0),
        output_tokens: usage["candidatesTokenCount"].as_u64().unwrap_or(0),
        cache_read_tokens: usage["cachedContentTokenCount"].as_u64().unwrap_or(0),
        reasoning_tokens: usage["thoughtsTokenCount"].as_u64().unwrap_or(0),
        total_tokens: usage["totalTokenCount"].as_u64().unwrap_or(0),
        ..Default::default()
    }
}

pub(super) fn parse_gemini_finish_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" | "FINISH_REASON_STOP" => StopReason::EndTurn,
        "MAX_TOKENS" | "FINISH_REASON_MAX_TOKENS" => StopReason::MaxTokens,
        "SAFETY" | "FINISH_REASON_SAFETY" => StopReason::ContentFilter,
        "MALFORMED_FUNCTION_CALL" => StopReason::Other("malformed_function_call".into()),
        other => StopReason::Other(other.to_lowercase()),
    }
}
