//! Shared helpers for all integration tests.

#![allow(dead_code)]

#[allow(unused_imports)]
pub use std::sync::Arc;

#[allow(unused_imports)]
pub use sideseat::{
    AudioProvider, ChatProvider, EmbeddingProvider, ImageProvider, Provider, ProviderError,
    VideoProvider, collect_stream,
    providers::{
        AnthropicProvider, BedrockProvider, CohereProvider, GeminiAuth, GeminiProvider,
        MistralProvider, OpenAIChatProvider, OpenAIResponsesProvider, XAIProvider,
    },
    types::{
        AudioContent, AudioFormat, CacheControl, ContentBlock, DocumentContent, DocumentFormat,
        EmbeddingRequest, ImageContent, ImageFormat, ImageGenerationRequest, ImageSize,
        MediaSource, Message, ProviderConfig, Response, Role, S3Location, SpeechRequest,
        StopReason, Tool, ToolResultBlock, TranscriptionRequest, VideoContent, VideoFormat,
        VideoGenerationRequest,
    },
};

/// Retries an async operation up to 5 times when `ProviderError::is_retryable()` is true,
/// or when the response has empty content (Bedrock can return 200 with empty content under load).
///
/// Rate-limit errors (`TooManyRequests`) wait 15 s by default (or `retry_after_secs` if present)
/// to respect free-tier quotas (e.g. Gemini 5 RPM). Other retryable errors use 1 s / 2 s
/// exponential backoff.
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
            Ok(r) => last = Some(Ok(r)), // empty content — retry
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
///
/// Bedrock's Nova models reject images that are too small (1×1 is not accepted).
/// This 64×64 image is the smallest that passes Nova's format validation.
pub const TINY_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAS0lEQVR42u3PMQ0AAAwDoPo33UrYvQQckD4XAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAYHLAMpT0sIcNbcEAAAAAElFTkSuQmCC";

/// Returns the cross-region inference prefix for a Bedrock region.
/// e.g. "eu-west-1" → "eu", "us-east-1" → "us", "ap-southeast-1" → "ap"
pub fn bedrock_region_prefix(region: &str) -> &'static str {
    if region.starts_with("eu-") {
        "eu"
    } else if region.starts_with("ap-") {
        "ap"
    } else {
        "us"
    }
}

/// Returns true if the error indicates the model isn't available in this region/auth context.
/// Used to skip tests gracefully when a model has limited availability, regional restrictions,
/// or isn't supported by the current auth mechanism (e.g. bearer token).
pub fn bedrock_model_not_available(e: &ProviderError) -> bool {
    match e {
        ProviderError::Api { status: 0, .. } => true, // SDK-level error (not HTTP); model/op unavailable
        ProviderError::Unsupported(_) => true,         // explicitly unsupported feature/model
        ProviderError::Api { message, .. } => {
            let m = message.to_lowercase();
            m.contains("not found")
                || m.contains("does not exist")
                || m.contains("no access")
                || m.contains("access denied")
                || m.contains("not supported in this region")
                || m.contains("not available")
                || m.contains("invalid model")
                || m.contains("identifier is invalid")   // "model identifier is invalid"
                || m.contains("on-demand throughput")    // requires provisioned throughput
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

/// Reads the Bedrock region from the first env var that is set, falling back to `"us-east-1"`.
///
/// Priority: `BEDROCK_REGION` → `AWS_REGION` → `AWS_DEFAULT_REGION` → `"us-east-1"`
pub fn bedrock_region() -> String {
    std::env::var("BEDROCK_REGION")
        .or_else(|_| std::env::var("AWS_REGION"))
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|_| "us-east-1".to_string())
}

/// Returns the Bedrock region for IAM-credential tests.
#[macro_export]
macro_rules! bedrock_iam_env {
    () => {{
        bedrock_region()
    }};
}

/// Returns `(api_key, region)` for bearer-token tests, skipping if no API key is set.
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
