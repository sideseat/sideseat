//! # SideSeat
//!
//! **AI Development Workbench** — Debug, trace, and understand your AI agents.
//!
//! SideSeat captures every LLM call, tool call, and agent decision, then
//! displays them in a web UI as they happen. Built on
//! [OpenTelemetry](https://opentelemetry.io/).
//!
//! ## LLM Providers
//!
//! This crate provides a unified [`Provider`] trait over multiple LLM backends:
//!
//! - [`providers::AnthropicProvider`] — Anthropic Messages API (direct, Bedrock, Vertex)
//! - [`providers::BedrockProvider`] — AWS Bedrock Converse API (native SDK)
//! - [`providers::OpenAIChatProvider`] — OpenAI Chat Completions + compatible providers
//!   (Groq, DeepSeek, xAI, Together, Fireworks, Mistral, Cerebras, Perplexity, Ollama, OpenRouter)
//! - [`providers::OpenAIResponsesProvider`] — OpenAI Responses API (stateful)
//! - [`providers::GeminiProvider`] — Google Gemini generateContent API / Vertex AI
//! - [`providers::GeminiInteractionsProvider`] — Google Gemini Interactions API (stateful, v2)
//! - [`providers::CohereProvider`] — Cohere Chat API v2
//! - [`providers::MistralProvider`] — Mistral AI API (direct and via Bedrock)
//! - [`providers::XAIProvider`] — xAI Grok API (vision, reasoning, Live Search)
//!
//! All providers implement [`ChatProvider`] (via [`Provider`]) with `stream()` and `complete()` methods.
//!
//! ## Quick Start
//!
//! ```bash
//! npx sideseat
//! ```
//!
//! See <https://sideseat.ai/docs> for full documentation.

#[path = "inference/env.rs"]
pub mod env;
#[path = "inference/error.rs"]
pub mod error;
#[path = "inference/mcp.rs"]
pub mod mcp;
#[path = "inference/middleware.rs"]
pub mod middleware;
#[path = "inference/mock.rs"]
pub mod mock;
#[path = "inference/provider.rs"]
pub mod provider;
#[path = "inference/providers/mod.rs"]
pub mod providers;
#[path = "inference/registry.rs"]
pub mod registry;
#[path = "inference/telemetry.rs"]
pub mod telemetry;
#[path = "inference/types.rs"]
pub mod types;

// Convenient re-exports
pub use error::ProviderError;
pub use middleware::{
    DefaultSettingsMiddleware, ExtractReasoningMiddleware, ImageModelMiddleware, LoggingMiddleware,
    Middleware, MiddlewareStack, RateLimitMiddleware, SimulateStreamingMiddleware, TimingMiddleware,
    WrappedImageModel, wrap_image_model,
};
pub use mock::{MockProvider, MockResponse};
pub use provider::{
    AgentHooks, AudioProvider, ChatProvider, DefaultHooks, EmbeddingProvider, FallbackProvider,
    ImageProvider, ModerationProvider, Provider, ProviderExt, ProviderHealthStatus, ProviderStream,
    RetryConfig, RetryProvider, StatefulProvider, TextStream, TextStreamWithMeta, VideoProvider,
    batch_complete, batch_embed,
    batch_generate_images, collect_stream, collect_stream_with_config, collect_stream_with_events,
    generate_text, record_stream, response_to_stream, run_agent_loop, run_agent_loop_with_hooks,
    stream_text, with_chunk_timeout, wrap_language_model,
};
pub use registry::ProviderRegistry;
pub use telemetry::{InstrumentedProvider, SideSeat, SideSeatGuard, TelemetryConfig};
pub use types::{
    AgentResult, AgentStep, AudioContent, AudioFormat, AudioOutputConfig, Base64Data, BuiltinTool,
    CacheControl, Citation, ContextManagementConfig, ContainerInfo, ContentBlock,
    ContentBlockStart, ContentDelta, ConversationBuilder, CostEstimate, DocumentContent,
    DocumentFormat, EmbeddingRequest, EmbeddingResponse, EmbeddingTaskType, FallbackStrategy,
    FallbackTrigger, GeneratedImage, GeneratedVideo, GroundingChunk, GroundingMetadata,
    ImageContent, ImageDetail, ImageEditRequest, ImageFormat, ImageGenerationRequest, JsonSchema,
    ImageGenerationResponse, ImageOutputFormat, ImageQuality, ImageSize, ImageStyle, MediaSource,
    McpToolConfig, Message, ModelCapability, ModelInfo, ModerationCategories,
    ModerationCategoryScores, ModerationRequest, ModerationResponse, ModerationResult,
    PartialConfig, PromptTemplate, ProviderConfig, ReasoningEffort, RequestMetadata, Response,
    ResponseFormat, Role, S3Location, SafetyCategory, SafetySetting, SafetyThreshold, ServiceTier,
    SpeechRequest, SpeechResponse, StaticTokenProvider, StopReason, StreamEvent, StreamMeta,
    StreamRecording,
    TextBlock, ThinkingBlock, TimestampGranularity, TokenCount, TokenLogprob, TokenProvider, Tool,
    ToolChoice, ToolResultBlock, ToolUseBlock, TopLogprob, TranscriptionRequest,
    TranscriptionResponse, TranscriptionSegment, TranscriptionWord, Usage, UsageAccumulator,
    VideoAspectRatio, VideoContent, VideoFormat, VideoGenerationRequest, VideoGenerationResponse,
    VideoResolution, WebSearchConfig, WebSearchUserLocation, cosine_similarity, estimate_tokens,
    euclidean_distance, model_capabilities, normalize_embedding, supports_audio_input,
    supports_audio_output, supports_extended_thinking, supports_function_calling, supports_vision,
    truncate_messages, validate_messages,
};
