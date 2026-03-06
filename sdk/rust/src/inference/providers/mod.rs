//! Built-in provider implementations for the SideSeat SDK.
//!
//! # Implementing a custom provider
//!
//! Implement [`ChatProvider`] (which requires [`Provider`]) to integrate any LLM API.
//! The only required method is [`ChatProvider::stream`]; `complete` and `count_tokens`
//! have default implementations built on top of it.
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use sideseat::{
//!     ChatProvider, Provider, ProviderConfig, ProviderError, ProviderStream, Message,
//!     collect_stream,
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
//!             // Call your API here and yield StreamEvents.
//!             // Use `async_stream::try_stream!` so `?` works inside the block.
//!             let _ = (api_key, messages, config);
//!             // yield StreamEvent::ContentBlockDelta { ... };
//!         })
//!     }
//! }
//! ```
mod sse;
pub(crate) mod openai_common;

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
pub use gemini::{GeminiAuth, GeminiProvider};
pub use gemini_interactions::GeminiInteractionsProvider;
pub use mistral::MistralProvider;
pub use openai_chat::OpenAIChatProvider;
pub use openai_responses::OpenAIResponsesProvider;
pub use xai::XAIProvider;
