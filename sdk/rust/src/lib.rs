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
//!
//! All providers implement the same [`Provider`] trait with `stream()` and `complete()` methods.
//!
//! ## Quick Start
//!
//! ```bash
//! npx sideseat
//! ```
//!
//! See <https://sideseat.ai/docs> for full documentation.

pub mod error;
pub mod provider;
pub mod providers;
pub mod types;

// Convenient re-exports
pub use error::ProviderError;
pub use provider::{
    FallbackProvider, Provider, ProviderExt, ProviderStream, RetryProvider, TextStream,
    batch_complete, collect_stream, collect_stream_with_events, run_agent_loop,
};
pub use types::{
    AudioContent, AudioFormat, Base64Data, CacheControl, ContentBlock, ContentBlockStart,
    ContentDelta, ConversationBuilder, CostEstimate, DocumentContent, DocumentFormat,
    EmbeddingRequest, EmbeddingResponse, EmbeddingTaskType, ImageContent, ImageFormat, MediaSource,
    Message, ModelInfo, ProviderConfig, ReasoningEffort, Response, ResponseFormat, Role,
    S3Location, ServiceTier, StopReason, StreamEvent, ThinkingBlock, TokenCount, Tool, ToolChoice,
    ToolResultBlock, ToolUseBlock, Usage, UsageAccumulator, VideoContent, VideoFormat,
    WebSearchConfig, estimate_tokens, truncate_messages,
};
