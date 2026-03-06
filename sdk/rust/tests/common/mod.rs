//! Shared helpers for all integration tests.

#![allow(dead_code)]

#[allow(unused_imports)]
pub use std::sync::Arc;

#[allow(unused_imports)]
pub use sideseat::{
    AudioProvider, ChatProvider, EmbeddingProvider, ImageProvider, Provider, ProviderError,
    VideoProvider, collect_stream,
    providers::{
        AnthropicProvider, BedrockProvider, CohereProvider, GeminiAuth, GeminiInteractionsProvider,
        GeminiProvider, MistralProvider, OpenAIChatProvider, OpenAIResponsesProvider, XAIProvider,
    },
    types::{
        AudioContent, AudioFormat, CacheControl, ContentBlock, DocumentContent, DocumentFormat,
        EmbeddingRequest, ImageContent, ImageFormat, ImageGenerationRequest, ImageSize,
        MediaSource, Message, ProviderConfig, Response, Role, S3Location, SpeechRequest,
        StopReason, Tool, ToolResultBlock, TranscriptionRequest, VideoContent, VideoFormat,
        VideoGenerationRequest,
    },
};

pub use httpmock::prelude::*;
pub use httpmock::Mock;

// ---------------------------------------------------------------------------
// Legacy test helpers (retry, user_msg, etc.)
// ---------------------------------------------------------------------------

/// Retries an async operation up to 5 times when `ProviderError::is_retryable()` is true,
/// or when the response has empty content (Bedrock can return 200 with empty content under load).
pub async fn retry<F, Fut>(mut f: F) -> Result<Response, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Response, ProviderError>>,
{
    let mut last = None;
    for attempt in 0..5u32 {
        if attempt > 0 {
            let wait_ms = match &last {
                Some(Err(ProviderError::TooManyRequests { retry_after_secs: Some(s), .. })) => {
                    s.saturating_mul(1000)
                }
                Some(Err(ProviderError::TooManyRequests { .. })) => 15_000,
                _ => 1_000u64 << (attempt - 1).min(3),
            };
            tokio::time::sleep(tokio::time::Duration::from_millis(wait_ms)).await;
        }
        match f().await {
            Ok(r) if !r.content.is_empty() => return Ok(r),
            Ok(r) => last = Some(Ok(r)),
            Err(e) if e.is_retryable() => last = Some(Err(e)),
            Err(e) => return Err(e),
        }
    }
    last.unwrap()
}

pub fn user_msg(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::text(text.to_string())],
        name: None,
        cache_control: None,
    }
}

pub fn default_config(model: &str) -> ProviderConfig {
    ProviderConfig {
        model: model.to_string(),
        max_tokens: Some(256),
        ..Default::default()
    }
}

pub fn echo_tool() -> Tool {
    Tool::new(
        "echo",
        "Echoes the provided message back.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "Message to echo"}
            },
            "required": ["message"]
        }),
    )
}

/// 64×64 solid-white PNG (base64). Used for vision / multimodal tests.
pub const TINY_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAS0lEQVR42u3PMQ0AAAwDoPo33UrYvQQckD4XAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAYHLAMpT0sIcNbcEAAAAAElFTkSuQmCC";

pub fn bedrock_region_prefix(region: &str) -> &'static str {
    if region.starts_with("eu-") {
        "eu"
    } else if region.starts_with("ap-") {
        "ap"
    } else {
        "us"
    }
}

pub fn bedrock_model_not_available(e: &ProviderError) -> bool {
    match e {
        ProviderError::Api { status: 0, .. } => true,
        ProviderError::Unsupported(_) => true,
        ProviderError::Api { message, .. } => {
            let m = message.to_lowercase();
            m.contains("not found")
                || m.contains("does not exist")
                || m.contains("no access")
                || m.contains("access denied")
                || m.contains("not supported in this region")
                || m.contains("not available")
                || m.contains("invalid model")
                || m.contains("identifier is invalid")
                || m.contains("on-demand throughput")
        }
        _ => false,
    }
}

pub fn bedrock_nova_lite(region: &str) -> String {
    format!("{}.amazon.nova-lite-v1:0", bedrock_region_prefix(region))
}

pub fn bedrock_nova_micro(region: &str) -> String {
    format!("{}.amazon.nova-micro-v1:0", bedrock_region_prefix(region))
}

pub fn bedrock_haiku(region: &str) -> String {
    format!("{}.anthropic.claude-haiku-4-5-20251001-v1:0", bedrock_region_prefix(region))
}

pub fn bedrock_region() -> String {
    std::env::var("BEDROCK_REGION")
        .or_else(|_| std::env::var("AWS_REGION"))
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|_| "us-east-1".to_string())
}

#[macro_export]
macro_rules! bedrock_iam_env {
    () => {{
        bedrock_region()
    }};
}

#[macro_export]
macro_rules! bedrock_api_key_env {
    () => {{
        let api_key = match std::env::var("BEDROCK_API_KEY")
            .or_else(|_| std::env::var("AWS_BEARER_TOKEN_BEDROCK"))
        {
            Ok(k) => k,
            Err(_) => return,
        };
        (api_key, bedrock_region())
    }};
}

// ---------------------------------------------------------------------------
// Canned response constants
// ---------------------------------------------------------------------------

// ── Anthropic ──────────────────────────────────────────────────────────────
pub const ANTHROPIC_COMPLETE_JSON: &str = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
pub const ANTHROPIC_TOOL_JSON: &str = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"echo","input":{"message":"hi"}}],"stop_reason":"tool_use","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
pub const ANTHROPIC_LIST_MODELS_JSON: &str = r#"{"data":[{"id":"claude-haiku-4-5-20251001","type":"model","display_name":"Claude Haiku 4.5","created_at":"2025-01-01T00:00:00Z"}],"has_more":false}"#;
pub const ANTHROPIC_COUNT_TOKENS_JSON: &str = r#"{"input_tokens":42}"#;
pub const ANTHROPIC_STREAM_EVENTS: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"test\",\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
    "event: content_block_stop\n",
    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
    "event: message_stop\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);
pub const ANTHROPIC_STREAM_TOOL_EVENTS: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"test\",\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"echo\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"message\\\":\\\"hi\\\"}\"}}\n\n",
    "event: content_block_stop\n",
    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":5}}\n\n",
    "event: message_stop\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

// ── OpenAI / Mistral / xAI (OpenAI-compatible) ────────────────────────────
pub const OPENAI_COMPLETE_JSON: &str = r#"{"id":"chatcmpl-test","object":"chat.completion","model":"gpt-4o-mini","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
pub const OPENAI_TOOL_JSON: &str = r#"{"id":"chatcmpl-test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"echo","arguments":"{\"message\":\"hi\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
pub const OPENAI_STREAM_EVENTS: &str = concat!(
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n",
    "data: [DONE]\n\n",
);
pub const OPENAI_STREAM_TOOL_EVENTS: &str = concat!(
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":null,\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"echo\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"message\\\":\\\"hi\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n",
    "data: [DONE]\n\n",
);
pub const OPENAI_EMBED_JSON: &str = r#"{"object":"list","data":[{"object":"embedding","embedding":[0.1,0.2,0.3],"index":0}],"usage":{"prompt_tokens":5,"total_tokens":5}}"#;
pub const OPENAI_IMAGE_GEN_JSON: &str = r#"{"created":1234567890,"data":[{"url":"https://example.com/image.png"}]}"#;
pub const OPENAI_LIST_MODELS_JSON: &str = r#"{"object":"list","data":[{"id":"gpt-4o-mini","object":"model","created":1234567890,"owned_by":"openai"}]}"#;
pub const OPENAI_LOGPROBS_JSON: &str = r#"{"id":"chatcmpl-test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop","logprobs":{"content":[{"token":"hello","logprob":-0.5,"top_logprobs":[{"token":"hello","logprob":-0.5},{"token":"hi","logprob":-1.2}]}]}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
pub const OPENAI_RESPONSES_JSON: &str = r#"{"id":"resp_test","object":"response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}"#;
pub const OPENAI_RESPONSES_TOOL_JSON: &str = r#"{"id":"resp_test","object":"response","status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"echo","arguments":"{\"message\":\"hi\"}"}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}"#;
pub const OPENAI_RESPONSES_STREAM_EVENTS: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"object\":\"response\",\"status\":\"in_progress\"}}\n\n",
    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"in_progress\",\"role\":\"assistant\",\"content\":[]}}\n\n",
    "data: {\"type\":\"response.content_part.added\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"part\":{\"type\":\"output_text\",\"text\":\"\"}}\n\n",
    "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"hello\"}\n\n",
    "data: {\"type\":\"response.output_text.done\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"hello\"}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"object\":\"response\",\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15},\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}]}}\n\n",
);
pub const OPENAI_RESPONSES_STREAM_TOOL_EVENTS: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"object\":\"response\",\"status\":\"in_progress\"}}\n\n",
    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"call_1\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"\",\"status\":\"in_progress\"}}\n\n",
    "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"call_1\",\"output_index\":0,\"delta\":\"{\\\"message\\\":\\\"hi\\\"}\"}\n\n",
    "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"call_1\",\"output_index\":0,\"arguments\":\"{\\\"message\\\":\\\"hi\\\"}\"}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"call_1\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"{\\\"message\\\":\\\"hi\\\"}\",\"status\":\"completed\"}}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"object\":\"response\",\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15},\"output\":[{\"type\":\"function_call\",\"id\":\"call_1\",\"call_id\":\"call_1\",\"name\":\"echo\",\"arguments\":\"{\\\"message\\\":\\\"hi\\\"}\",\"status\":\"completed\"}]}}\n\n",
);

// ── Gemini ────────────────────────────────────────────────────────────────
pub const GEMINI_COMPLETE_JSON: &str = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#;
pub const GEMINI_TOOL_JSON: &str = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"echo","args":{"message":"hi"}}}]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#;
pub const GEMINI_STREAM_EVENTS: &str = concat!(
    "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hello\"}]},\"finishReason\":\"STOP\",\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":5}}\n\n",
);
pub const GEMINI_EMBED_JSON: &str = r#"{"embedding":{"values":[0.1,0.2,0.3]}}"#;
pub const GEMINI_COUNT_TOKENS_JSON: &str = r#"{"totalTokens":42}"#;
pub const GEMINI_LIST_MODELS_JSON: &str = r#"{"models":[{"name":"models/gemini-2.5-flash-lite","displayName":"Gemini 2.5 Flash Lite","supportedGenerationMethods":["generateContent"]}]}"#;

// ── Gemini Interactions ──────────────────────────────────────────────────
pub const INTERACTIONS_COMPLETE_JSON: &str = r#"{"id":"interaction-test","status":"completed","outputs":[{"type":"text","text":"hello"}],"usage":{"total_input_tokens":10,"total_output_tokens":5},"model":"models/test"}"#;
pub const INTERACTIONS_TOOL_JSON: &str = r#"{"id":"interaction-test","status":"requires_action","outputs":[{"type":"function_call","id":"call_1","name":"echo","arguments":{"message":"hi"}}],"usage":{"total_input_tokens":10,"total_output_tokens":5}}"#;
pub const INTERACTIONS_STREAM_EVENTS: &str = concat!(
    "data: {\"event_type\":\"interaction.start\",\"interaction\":{\"id\":\"interaction-test\"}}\n\n",
    "data: {\"event_type\":\"content.start\",\"index\":0,\"content\":{\"type\":\"text\"}}\n\n",
    "data: {\"event_type\":\"content.delta\",\"index\":0,\"delta\":{\"type\":\"text\",\"text\":\"hello\"}}\n\n",
    "data: {\"event_type\":\"content.stop\",\"index\":0}\n\n",
    "data: {\"event_type\":\"interaction.complete\",\"interaction\":{\"id\":\"interaction-test\",\"status\":\"completed\",\"usage\":{\"total_input_tokens\":10,\"total_output_tokens\":5}}}\n\n",
);

// ── Cohere ────────────────────────────────────────────────────────────────
pub const COHERE_COMPLETE_JSON: &str = r#"{"id":"chat-test","finish_reason":"COMPLETE","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]},"usage":{"billed_units":{"input_tokens":10,"output_tokens":5},"tokens":{"input_tokens":10,"output_tokens":5}}}"#;
pub const COHERE_TOOL_JSON: &str = r#"{"id":"chat-test","finish_reason":"TOOL_CALL","message":{"role":"assistant","content":[],"tool_calls":[{"id":"call_1","type":"function","function":{"name":"echo","arguments":"{\"message\":\"hi\"}"}}]},"usage":{"billed_units":{"input_tokens":10,"output_tokens":5},"tokens":{"input_tokens":10,"output_tokens":5}}}"#;
pub const COHERE_EMBED_JSON: &str = r#"{"id":"emb-test","embeddings":{"float":[[0.1,0.2,0.3]]},"texts":["hello"],"meta":{}}"#;
pub const COHERE_TOKENIZE_JSON: &str = r#"{"tokens":[1,2,3,4,5,6],"token_strings":["hi","there"],"meta":{}}"#;
pub const COHERE_LIST_MODELS_JSON: &str = r#"{"models":[{"name":"command-r-plus","endpoints":["chat"],"context_length":128000}]}"#;

// ── Mistral ────────────────────────────────────────────────────────────────
pub const MISTRAL_EMBED_JSON: &str = r#"{"id":"emb-test","object":"list","data":[{"object":"embedding","embedding":[0.1,0.2,0.3],"index":0}],"usage":{"prompt_tokens":5,"total_tokens":5}}"#;
pub const MISTRAL_LIST_MODELS_JSON: &str = r#"{"data":[{"id":"mistral-small-latest","object":"model","created":1234567890,"owned_by":"mistralai"}],"object":"list"}"#;

// ── xAI ────────────────────────────────────────────────────────────────────
pub const XAI_EMBED_JSON: &str = r#"{"object":"list","data":[{"object":"embedding","embedding":[0.1,0.2,0.3],"index":0}],"usage":{"prompt_tokens":5,"total_tokens":5}}"#;
pub const XAI_LIST_MODELS_JSON: &str = r#"{"data":[{"id":"grok-3-mini","object":"model","created":1234567890,"owned_by":"xai"}]}"#;

// ── Bedrock ────────────────────────────────────────────────────────────────
pub const BEDROCK_COMPLETE_JSON: &str = r#"{"output":{"message":{"role":"assistant","content":[{"text":"hello"}]}},"stopReason":"end_turn","usage":{"inputTokens":10,"outputTokens":5,"totalTokens":15}}"#;
pub const BEDROCK_COMPLETE_MAX_TOKENS_JSON: &str = r#"{"output":{"message":{"role":"assistant","content":[{"text":"hello"}]}},"stopReason":"max_tokens","usage":{"inputTokens":10,"outputTokens":5,"totalTokens":15}}"#;
pub const BEDROCK_TOOL_JSON: &str = r#"{"output":{"message":{"role":"assistant","content":[{"toolUse":{"toolUseId":"tu_1","name":"echo","input":{"message":"hi"}}}]}},"stopReason":"tool_use","usage":{"inputTokens":10,"outputTokens":5,"totalTokens":15}}"#;
pub const BEDROCK_EMBED_TITAN_JSON: &str = r#"{"embedding":[0.1,0.2,0.3],"inputTextTokenCount":5}"#;
pub const BEDROCK_EMBED_TITAN_MULTIMODAL_JSON: &str = r#"{"embedding":[0.1,0.2,0.3],"inputTextTokenCount":5,"inputImageTokenCount":5}"#;
pub const BEDROCK_EMBED_COHERE_JSON: &str = r#"{"embeddings":{"float":[[0.1,0.2,0.3]]},"texts":["test"]}"#;
pub const BEDROCK_IMAGE_GEN_JSON: &str = r#"{"images":["iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="],"error":""}"#;
pub const BEDROCK_ASYNC_INVOKE_JSON: &str = r#"{"invocationArn":"arn:aws:bedrock:us-east-1:123456789012:async-invoke/test-id"}"#;
pub const BEDROCK_ASYNC_STATUS_JSON: &str = r#"{"invocationArn":"arn:aws:bedrock:us-east-1:123456789012:async-invoke/test-id","status":"Completed","outputDataConfig":{"s3OutputDataConfig":{"s3Uri":"s3://test-bucket/output/"}}}"#;
pub const BEDROCK_COUNT_TOKENS_JSON: &str = r#"{"inputTokens":42}"#;
pub const BEDROCK_LIST_MODELS_JSON: &str = r#"{"modelSummaries":[{"modelId":"amazon.nova-lite-v1:0","modelName":"Amazon Nova Lite","providerName":"Amazon","responseStreamingSupported":true}]}"#;
pub const BEDROCK_VALIDATION_ERROR_JSON: &str = r#"{"__type":"ValidationException","message":"image not supported"}"#;
pub const BEDROCK_ASYNC_INVOKE_ERROR_JSON: &str = r#"{"__type":"ValidationException","message":"S3 bucket does not exist"}"#;

// ---------------------------------------------------------------------------
// EventStream binary encoding helpers (for Bedrock streaming)
// ---------------------------------------------------------------------------

/// Encode a single AWS EventStream frame.
/// Format: total_len(4) + headers_len(4) + prelude_crc(4) + headers + payload + msg_crc(4)
pub fn encode_eventstream_frame(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut headers_bytes: Vec<u8> = Vec::new();
    for (name, value) in headers {
        headers_bytes.push(name.len() as u8);
        headers_bytes.extend_from_slice(name.as_bytes());
        headers_bytes.push(7u8); // string type
        headers_bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
        headers_bytes.extend_from_slice(value.as_bytes());
    }
    let total_len = 16u32 + headers_bytes.len() as u32 + payload.len() as u32;
    let headers_len = headers_bytes.len() as u32;
    let mut prelude = Vec::new();
    prelude.extend_from_slice(&total_len.to_be_bytes());
    prelude.extend_from_slice(&headers_len.to_be_bytes());
    let prelude_crc = crc32fast::hash(&prelude);
    let mut msg = prelude;
    msg.extend_from_slice(&prelude_crc.to_be_bytes());
    msg.extend_from_slice(&headers_bytes);
    msg.extend_from_slice(payload);
    let msg_crc = crc32fast::hash(&msg);
    msg.extend_from_slice(&msg_crc.to_be_bytes());
    msg
}

fn eventstream_event(event_type: &str, payload: &[u8]) -> Vec<u8> {
    encode_eventstream_frame(
        &[
            (":event-type", event_type),
            (":content-type", "application/json"),
            (":message-type", "event"),
        ],
        payload,
    )
}

/// Binary body for Bedrock converse-stream (contentBlockDelta + messageStop + metadata).
pub fn bedrock_converse_stream_body() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(eventstream_event(
        "contentBlockDelta",
        &serde_json::to_vec(&serde_json::json!({"contentBlockIndex":0,"delta":{"text":"hello"}})).unwrap(),
    ));
    body.extend(eventstream_event(
        "messageStop",
        &serde_json::to_vec(&serde_json::json!({"stopReason":"end_turn"})).unwrap(),
    ));
    body.extend(eventstream_event(
        "metadata",
        &serde_json::to_vec(&serde_json::json!({"usage":{"inputTokens":10,"outputTokens":5,"totalTokens":15}})).unwrap(),
    ));
    body
}

/// Binary body for Bedrock converse-stream with tool_use.
pub fn bedrock_converse_stream_tool_body() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(eventstream_event(
        "contentBlockStart",
        &serde_json::to_vec(&serde_json::json!({"contentBlockIndex":0,"start":{"toolUse":{"toolUseId":"tu_1","name":"echo"}}})).unwrap(),
    ));
    body.extend(eventstream_event(
        "contentBlockDelta",
        &serde_json::to_vec(&serde_json::json!({"contentBlockIndex":0,"delta":{"toolUse":{"input":"{\"message\":\"hi\"}"}}})).unwrap(),
    ));
    body.extend(eventstream_event(
        "contentBlockStop",
        &serde_json::to_vec(&serde_json::json!({"contentBlockIndex":0})).unwrap(),
    ));
    body.extend(eventstream_event(
        "messageStop",
        &serde_json::to_vec(&serde_json::json!({"stopReason":"tool_use"})).unwrap(),
    ));
    body.extend(eventstream_event(
        "metadata",
        &serde_json::to_vec(&serde_json::json!({"usage":{"inputTokens":10,"outputTokens":5,"totalTokens":15}})).unwrap(),
    ));
    body
}

/// Wrap raw bytes in the `{"bytes": "base64"}` JSON envelope that Bedrock's
/// `invoke_model_with_response_stream` SDK uses for `ResponseStream::Chunk` events.
fn bedrock_invoke_chunk_payload(raw: &[u8]) -> Vec<u8> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
    serde_json::to_vec(&serde_json::json!({"bytes": b64})).unwrap()
}

/// Binary body for invoke-with-response-stream (AnthropicProvider via Bedrock).
pub fn bedrock_anthropic_stream_body() -> Vec<u8> {
    let events = [
        serde_json::json!({"type":"message_start","message":{"id":"msg_test","type":"message","role":"assistant","content":[],"model":"test","usage":{"input_tokens":10,"output_tokens":0}}}),
        serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}),
        serde_json::json!({"type":"content_block_stop","index":0}),
        serde_json::json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}),
        serde_json::json!({"type":"message_stop"}),
    ];
    let mut body = Vec::new();
    for ev in &events {
        let payload = bedrock_invoke_chunk_payload(&serde_json::to_vec(ev).unwrap());
        body.extend(eventstream_event("chunk", &payload));
    }
    body
}

/// Binary body for invoke-with-response-stream (Anthropic tool_use via Bedrock).
pub fn bedrock_anthropic_stream_tool_body() -> Vec<u8> {
    let events = [
        serde_json::json!({"type":"message_start","message":{"id":"msg_test","type":"message","role":"assistant","content":[],"model":"test","usage":{"input_tokens":10,"output_tokens":0}}}),
        serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu_1","name":"echo"}}),
        serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"message\":\"hi\"}"}}),
        serde_json::json!({"type":"content_block_stop","index":0}),
        serde_json::json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
        serde_json::json!({"type":"message_stop"}),
    ];
    let mut body = Vec::new();
    for ev in &events {
        let payload = bedrock_invoke_chunk_payload(&serde_json::to_vec(ev).unwrap());
        body.extend(eventstream_event("chunk", &payload));
    }
    body
}

// ---------------------------------------------------------------------------
// AWS SDK client helpers for mock Bedrock servers
// ---------------------------------------------------------------------------

/// Create a fake Bedrock runtime client pointing at a mock server.
pub fn fake_bedrock_runtime_client(base_url: &str) -> aws_sdk_bedrockruntime::Client {
    use aws_credential_types::Credentials;
    use aws_sdk_bedrockruntime::config::{BehaviorVersion, Region};
    let creds = Credentials::new("fake", "fake", None, None, "test");
    let conf = aws_sdk_bedrockruntime::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(creds)
        .endpoint_url(base_url)
        .build();
    aws_sdk_bedrockruntime::Client::from_conf(conf)
}

/// Create a fake Bedrock management client pointing at a mock server.
pub fn fake_bedrock_mgmt_client(base_url: &str) -> aws_sdk_bedrock::Client {
    use aws_credential_types::Credentials;
    use aws_sdk_bedrock::config::{BehaviorVersion, Region};
    let creds = Credentials::new("fake", "fake", None, None, "test");
    let conf = aws_sdk_bedrock::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(creds)
        .endpoint_url(base_url)
        .build();
    aws_sdk_bedrock::Client::from_conf(conf)
}

// ---------------------------------------------------------------------------
// Provider factory helpers
// ---------------------------------------------------------------------------

pub fn mock_anthropic() -> (MockServer, AnthropicProvider) {
    let server = MockServer::start();
    let p = AnthropicProvider::new("test-key").with_base_url(server.base_url());
    (server, p)
}

pub fn mock_openai_chat() -> (MockServer, OpenAIChatProvider) {
    let server = MockServer::start();
    let p = OpenAIChatProvider::new("test-key").with_api_base(server.base_url());
    (server, p)
}

pub fn mock_openai_responses() -> (MockServer, OpenAIResponsesProvider) {
    let server = MockServer::start();
    let p = OpenAIResponsesProvider::new("test-key").with_api_base(server.base_url());
    (server, p)
}

pub fn mock_gemini() -> (MockServer, GeminiProvider) {
    let server = MockServer::start();
    let p = GeminiProvider::new(GeminiAuth::ApiKey("test-key".to_string()))
        .with_api_base(server.base_url());
    (server, p)
}

pub fn mock_gemini_interactions() -> (MockServer, GeminiInteractionsProvider) {
    let server = MockServer::start();
    let p = GeminiInteractionsProvider::new("test-key").with_api_base(server.base_url());
    (server, p)
}

pub fn mock_cohere() -> (MockServer, CohereProvider) {
    let server = MockServer::start();
    let p = CohereProvider::new("test-key").with_api_base(server.base_url());
    (server, p)
}

pub fn mock_mistral() -> (MockServer, MistralProvider) {
    let server = MockServer::start();
    let p = MistralProvider::new("test-key").with_api_base(server.base_url());
    (server, p)
}

pub fn mock_xai() -> (MockServer, XAIProvider) {
    let server = MockServer::start();
    let p = XAIProvider::new("test-key").with_api_base(server.base_url());
    (server, p)
}

pub fn mock_bedrock_openai_chat() -> (MockServer, OpenAIChatProvider) {
    let server = MockServer::start();
    let p = OpenAIChatProvider::for_bedrock_openai("us-east-1", "test-key")
        .with_api_base(server.base_url());
    (server, p)
}

pub fn mock_bedrock_openai_responses() -> (MockServer, OpenAIResponsesProvider) {
    let server = MockServer::start();
    let p = OpenAIResponsesProvider::for_bedrock_openai("us-east-1", "test-key")
        .with_api_base(server.base_url());
    (server, p)
}

/// BedrockProvider pointed at a mock server (converse, invoke, async-invoke, management).
pub fn mock_bedrock() -> (MockServer, BedrockProvider) {
    let server = MockServer::start();
    let client = fake_bedrock_runtime_client(&server.base_url());
    let mgmt = fake_bedrock_mgmt_client(&server.base_url());
    let p = BedrockProvider::from_clients(client, mgmt);
    (server, p)
}

/// AnthropicProvider via Bedrock (invoke_model / invoke_model_with_response_stream).
pub fn mock_anthropic_bedrock() -> (MockServer, AnthropicProvider) {
    let server = MockServer::start();
    let client = Arc::new(fake_bedrock_runtime_client(&server.base_url()));
    let p = AnthropicProvider::from_bedrock(client);
    (server, p)
}

// ---------------------------------------------------------------------------
// Mock registration helpers
// ---------------------------------------------------------------------------

/// Register a JSON response mock.
pub fn mock_json<'a>(server: &'a MockServer, method: Method, path_pattern: &'a str, body: &'static str) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(method).path_includes(path_pattern);
        then.status(200).header("content-type", "application/json").body(body);
    })
}

/// Register an SSE response mock.
pub fn mock_sse<'a>(server: &'a MockServer, method: Method, path_pattern: &'a str, body: &'static str) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(method).path_includes(path_pattern);
        then.status(200).header("content-type", "text/event-stream").body(body);
    })
}

/// Register an EventStream (binary) response mock for Bedrock streaming.
pub fn mock_eventstream<'a>(server: &'a MockServer, path_pattern: &'a str, body: Vec<u8>) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(POST).path_includes(path_pattern);
        then.status(200)
            .header("content-type", "application/vnd.amazon.eventstream")
            .body(body);
    })
}

/// Register a JSON error response mock.
pub fn mock_json_error<'a>(server: &'a MockServer, method: Method, path_pattern: &'a str, status: u16, body: &'static str) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(method).path_includes(path_pattern);
        then.status(status).header("content-type", "application/json").body(body);
    })
}

/// Register two sequential JSON mocks at the same path (always POST).
/// First call returns `body1`, second call returns `body2`.
pub fn mock_json_seq2<'a>(
    server: &'a MockServer,
    path_pattern: &'a str,
    body1: &'static str,
    body2: &'static str,
) -> (Mock<'a>, Mock<'a>) {
    let m1 = server.mock(|when, then| {
        when.method(POST).path_includes(path_pattern);
        then.status(200).header("content-type", "application/json").body(body1);
    });
    let m2 = server.mock(|when, then| {
        when.method(POST).path_includes(path_pattern);
        then.status(200).header("content-type", "application/json").body(body2);
    });
    (m1, m2)
}
