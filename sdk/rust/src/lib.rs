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
//! ## Composition
//!
//! Providers are plain values — compose them with wrappers for resilience and observability:
//!
//! ```rust,no_run
//! # use sideseat::{providers::AnthropicProvider, RetryProvider, FallbackProvider,
//! #     middleware::MiddlewareStack, LoggingMiddleware, TimingMiddleware};
//! # async fn example() {
//! // Retry transient errors up to 3 times with exponential backoff
//! let retrying = RetryProvider::new(
//!     AnthropicProvider::from_env().unwrap(),
//!     3,
//! );
//!
//! // Fall back to a second provider if the first fails
//! let with_fallback = FallbackProvider::new(vec![
//!     Box::new(AnthropicProvider::from_env().unwrap()),
//!     Box::new(AnthropicProvider::from_env().unwrap()), // e.g. different model/region
//! ]);
//!
//! // Intercept calls with middleware (logging, timing, rate limiting, custom hooks)
//! let instrumented = MiddlewareStack::new(AnthropicProvider::from_env().unwrap())
//!     .with(LoggingMiddleware)
//!     .with(TimingMiddleware::new());
//! # }
//! ```
//!
//! Recommended stacking order (outermost first):
//! 1. [`MiddlewareStack`] — logging, timing, rate limiting
//! 2. [`RetryProvider`] — retry transient errors
//! 3. [`FallbackProvider`] — switch providers on persistent failure
//!
//! ## Observability
//!
//! Wrap any provider with [`InstrumentedProvider`] to emit OpenTelemetry spans and metrics
//! automatically. Use [`SideSeat::new()`] to initialize the OTel pipeline pointed at
//! the SideSeat server:
//!
//! ```rust,no_run
//! # use sideseat::{providers::AnthropicProvider, telemetry::{InstrumentedProvider, SideSeat}};
//! # async fn example() {
//! let _guard = SideSeat::new().init();
//! let provider = InstrumentedProvider::new(AnthropicProvider::from_env().unwrap());
//! # }
//! ```
//!
//! ## Quick Start
//!
//! ```bash
//! npx sideseat
//! ```
//!
//! See <https://sideseat.ai/docs> for full documentation.

#[path = "inference/context/mod.rs"]
pub mod context;
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
/// Convenience re-exports for glob imports: `use sideseat::prelude::*`.
///
/// Includes the most frequently used traits, types, and builders. Import specific
/// items from their respective modules when you need something not listed here.
pub mod prelude {
    pub use crate::error::ProviderError;
    pub use crate::provider::{
        ChatProvider, EmbeddingProvider, ImageProvider, AudioProvider, VideoProvider,
        ModerationProvider, StatefulProvider, Provider, ProviderExt, ProviderStream,
        RetryProvider, FallbackProvider, TextStream, run_agent_loop, run_agent_loop_with_hooks,
        collect_stream, stream_text, generate_text,
    };
    pub use crate::middleware::{Middleware, MiddlewareStack, LoggingMiddleware, TimingMiddleware};
    pub use crate::types::{
        Message, Role, Response, ContentBlock, ProviderConfig, Tool, ToolUseBlock, ToolResultBlock,
        ConversationBuilder, StreamEvent, StopReason, Usage, TokenCount,
    };
}

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
