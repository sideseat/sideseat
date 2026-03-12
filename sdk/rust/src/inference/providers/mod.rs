//! Built-in provider implementations for the SideSeat SDK.
//!
//! # Implementing a custom provider
//!
//! Implement [`crate::ChatProvider`] (which requires [`crate::provider::Provider`]) to integrate any LLM API.
//! The only required method is [`crate::ChatProvider::stream`]; `complete` and `count_tokens`
//! have default implementations built on top of it.
//!
//! ## Stream event lifecycle
//!
//! A well-formed stream emits events in this order:
//! 1. [`crate::types::StreamEvent::MessageStart`] — signals the start of a response
//! 2. [`crate::types::StreamEvent::ContentBlockStart`] + [`crate::types::StreamEvent::ContentBlockDelta`] × N + [`crate::types::StreamEvent::ContentBlockStop`] — one group per content block
//! 3. [`crate::types::StreamEvent::Metadata`] — usage (token counts), model ID, and response ID
//! 4. [`crate::types::StreamEvent::MessageStop`] — stop reason
//!
//! Providers that send usage only at the end (not in a separate event) should emit
//! `Metadata` just before `MessageStop`. If the API doesn't return usage, omit the
//! `Metadata` event — `collect_stream` will log a debug warning and use zero counts.
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use sideseat::{
//!     ChatProvider, Provider, ProviderConfig, ProviderError, ProviderStream, Message,
//!     types::{ContentBlockStart, ContentDelta, Role, StopReason, StreamEvent, Usage},
//! };
//!
//! struct MyProvider { api_key: String }
//!
//! #[async_trait]
//! impl Provider for MyProvider {
//!     fn provider_name(&self) -> &'static str { "my-provider" }
//! }
//!
//! #[async_trait]
//! impl ChatProvider for MyProvider {
//!     fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
//!         let api_key = self.api_key.clone();
//!         Box::pin(async_stream::try_stream! {
//!             // 1. Signal start
//!             yield StreamEvent::MessageStart { role: Role::Assistant };
//!
//!             // 2. Stream text content
//!             yield StreamEvent::ContentBlockStart { index: 0, block: ContentBlockStart::Text };
//!             yield StreamEvent::ContentBlockDelta {
//!                 index: 0,
//!                 delta: ContentDelta::Text { text: "Hello!".into() },
//!             };
//!             yield StreamEvent::ContentBlockStop { index: 0 };
//!
//!             // 3. Emit token usage and model info
//!             yield StreamEvent::Metadata {
//!                 usage: Usage { input_tokens: 10, output_tokens: 5, ..Usage::default() },
//!                 model: Some(config.model.clone()),
//!                 id: None,
//!             };
//!
//!             // 4. Signal completion
//!             yield StreamEvent::MessageStop { stop_reason: StopReason::EndTurn };
//!         })
//!     }
//! }
//! ```
pub(crate) mod openai_common;
mod sse;

pub mod anthropic;
pub mod bedrock;
pub mod cohere;
pub mod gemini;
pub mod gemini_interactions;
pub mod mistral;
pub mod openai_chat;
pub mod openai_responses;
pub mod xai;

pub use anthropic::AnthropicProvider;
pub use bedrock::BedrockProvider;
pub use cohere::CohereProvider;
pub use gemini::{GcpAdcTokenProvider, GeminiAuth, GeminiProvider};
pub use gemini_interactions::GeminiInteractionsProvider;
pub use mistral::MistralProvider;
pub use openai_chat::OpenAIChatProvider;
pub use openai_responses::OpenAIResponsesProvider;
pub use xai::XAIProvider;
