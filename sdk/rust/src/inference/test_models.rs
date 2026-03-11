//! Default test models for credential connectivity checks.
//!
//! These models are used by the server's test-connection feature to verify
//! API credentials with a minimal request (`max_tokens=1`).
//!
//! Models are selected for test efficiency (small, fast, widely available).
//! `ModelNotFound` from `complete()` is treated as success (auth worked).

/// Anthropic — Claude Haiku (fastest/cheapest for test)
pub const ANTHROPIC: &str = "claude-haiku-4-5-20251001";

/// OpenAI — latest GPT model (ModelNotFound = success if account has no access)
pub const OPENAI: &str = "gpt-4.1-nano";

/// Google Gemini — latest fast Flash model
pub const GEMINI: &str = "gemini-2.5-flash";

/// Cohere — Command A (latest)
pub const COHERE: &str = "command-a-03-2025";

/// Groq — production-stable Llama
pub const GROQ: &str = "llama-3.3-70b-versatile";

/// DeepSeek — stable alias
pub const DEEPSEEK: &str = "deepseek-chat";

/// xAI — Grok 3 mini (fast)
pub const XAI: &str = "grok-3-mini";

/// Mistral AI — Mistral Small (cost-efficient)
pub const MISTRAL: &str = "mistral-small-latest";

/// Together AI — Llama 4 Maverick
pub const TOGETHER: &str = "meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8";

/// Fireworks AI
pub const FIREWORKS: &str = "accounts/fireworks/models/llama-v3p3-70b-instruct";

/// Cerebras — Llama 3.3
pub const CEREBRAS: &str = "llama-3.3-70b";

/// Perplexity — lightweight sonar
pub const PERPLEXITY: &str = "sonar";

/// OpenRouter — routed to GPT (ModelNotFound = success)
pub const OPENROUTER: &str = "openai/gpt-4.1-nano";

/// Ollama — common default model (best-effort; user may not have it)
pub const OLLAMA: &str = "llama3.2";

/// AWS Bedrock — Claude Haiku 4.5 (cheap, available in most regions)
/// For EU regions, prefix with "eu." before using.
pub const BEDROCK: &str = "anthropic.claude-haiku-4-5-20251001-v1:0";
