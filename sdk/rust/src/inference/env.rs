//! Environment variable names and read helpers used by the SDK.
//!
//! All provider `from_env()` constructors and [`crate::telemetry::SideSeat::new`] read
//! credentials and configuration using the helpers in this module.
//!
//! # Supported variables
//!
//! | Constant | Used by |
//! |---|---|
//! | [`keys::ANTHROPIC_API_KEY`] | [`crate::providers::AnthropicProvider::from_env`] |
//! | [`keys::OPENAI_API_KEY`] | [`crate::providers::OpenAIChatProvider::from_env`], [`crate::providers::OpenAIResponsesProvider::from_env`] |
//! | [`keys::GEMINI_API_KEY`] / [`keys::GOOGLE_API_KEY`] | [`crate::providers::GeminiProvider::from_env`], [`crate::providers::GeminiInteractionsProvider::from_env`] |
//! | [`keys::COHERE_API_KEY`] | [`crate::providers::CohereProvider::from_env`] |
//! | [`keys::GROQ_API_KEY`] | [`crate::providers::OpenAIChatProvider::for_groq_from_env`] |
//! | [`keys::DEEPSEEK_API_KEY`] | [`crate::providers::OpenAIChatProvider::for_deepseek_from_env`] |
//! | [`keys::XAI_API_KEY`] | [`crate::providers::XAIProvider::from_env`], [`crate::providers::OpenAIChatProvider::for_xai_from_env`] |
//! | [`keys::MISTRAL_API_KEY`] | [`crate::providers::MistralProvider::from_env`], [`crate::providers::OpenAIChatProvider::for_mistral_from_env`] |
//! | [`keys::TOGETHER_API_KEY`] | [`crate::providers::OpenAIChatProvider::for_together_from_env`] |
//! | [`keys::BEDROCK_API_KEY`] | [`crate::providers::OpenAIChatProvider::for_bedrock_openai_from_env`], [`crate::providers::OpenAIResponsesProvider::for_bedrock_openai_from_env`] |
//! | [`keys::SIDESEAT_ENDPOINT`] | [`crate::telemetry::SideSeat::new`] — default `http://localhost:5388` |
//! | [`keys::SIDESEAT_PROJECT_ID`] | [`crate::telemetry::SideSeat::new`] — default `default` |
//! | [`keys::SIDESEAT_API_KEY`] | [`crate::telemetry::SideSeat::new`] — optional |

use crate::error::ProviderError;

/// Environment variable name constants used by the SDK.
pub mod keys {
    /// Anthropic API key.
    pub const ANTHROPIC_API_KEY: &str = "ANTHROPIC_API_KEY";

    /// OpenAI API key. Also used by [`crate::providers::OpenAIResponsesProvider`].
    pub const OPENAI_API_KEY: &str = "OPENAI_API_KEY";

    /// Gemini / Google AI API key. Falls back to [`GOOGLE_API_KEY`].
    pub const GEMINI_API_KEY: &str = "GEMINI_API_KEY";
    /// Google Cloud API key. Fallback when [`GEMINI_API_KEY`] is not set.
    pub const GOOGLE_API_KEY: &str = "GOOGLE_API_KEY";

    /// Cohere API key.
    pub const COHERE_API_KEY: &str = "COHERE_API_KEY";

    /// Groq API key.
    pub const GROQ_API_KEY: &str = "GROQ_API_KEY";
    /// DeepSeek API key.
    pub const DEEPSEEK_API_KEY: &str = "DEEPSEEK_API_KEY";
    /// xAI / Grok API key.
    pub const XAI_API_KEY: &str = "XAI_API_KEY";
    /// Mistral API key.
    pub const MISTRAL_API_KEY: &str = "MISTRAL_API_KEY";
    /// Together AI API key.
    pub const TOGETHER_API_KEY: &str = "TOGETHER_API_KEY";

    /// AWS Bedrock API key (bearer token). Also accepted as `AWS_BEARER_TOKEN_BEDROCK`.
    /// Used by [`crate::providers::OpenAIChatProvider::for_bedrock_openai_from_env`] and
    /// [`crate::providers::OpenAIResponsesProvider::for_bedrock_openai_from_env`].
    pub const BEDROCK_API_KEY: &str = "BEDROCK_API_KEY";

    /// SideSeat server base URL. Default: `http://localhost:5388`.
    pub const SIDESEAT_ENDPOINT: &str = "SIDESEAT_ENDPOINT";
    /// SideSeat project ID. Default: `default`.
    pub const SIDESEAT_PROJECT_ID: &str = "SIDESEAT_PROJECT_ID";
    /// SideSeat API key. Optional.
    pub const SIDESEAT_API_KEY: &str = "SIDESEAT_API_KEY";
}

/// Read a required env var, returning [`ProviderError::MissingConfig`] if absent.
pub fn require(name: &str) -> Result<String, ProviderError> {
    std::env::var(name).map_err(|_| ProviderError::MissingConfig(format!("{name} not set")))
}

/// Read the first set env var from `names`, returning [`ProviderError::MissingConfig`] if none are set.
///
/// Used for providers that accept alternative credential env vars
/// (e.g. `GEMINI_API_KEY` / `GOOGLE_API_KEY`).
pub fn require_any(names: &[&str]) -> Result<String, ProviderError> {
    for &name in names {
        if let Ok(v) = std::env::var(name) {
            return Ok(v);
        }
    }
    Err(ProviderError::MissingConfig(format!(
        "{} not set",
        names.join(" or ")
    )))
}

/// Read an optional env var, returning `None` if absent.
pub fn optional(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Read an optional env var, returning `default` if absent.
pub fn optional_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}
