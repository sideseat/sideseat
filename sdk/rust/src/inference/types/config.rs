use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{
    AudioOutputConfig, BuiltinTool, ContextManagementConfig, ReasoningEffort, RequestMetadata,
    ResponseFormat, SafetySetting, ServiceTier, Tool, ToolChoice, WebSearchConfig,
};

// ---------------------------------------------------------------------------
// Provider configuration
// ---------------------------------------------------------------------------

/// Per-request configuration: model selection, sampling parameters, tools, and provider-specific options.
///
/// Create with [`ProviderConfig::new(model)`](Self::new) and customize using builder methods:
///
/// ```
/// use sideseat::ProviderConfig;
///
/// let config = ProviderConfig::new("claude-haiku-4-5-20251001")
///     .with_max_tokens(1024)
///     .with_temperature(0.7)
///     .with_system("You are a helpful assistant.");
/// ```
///
/// Not all providers support every field — call [`validate()`](Self::validate) with the
/// provider name to surface unsupported settings. Unrecognized fields are silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Model identifier (provider-specific). Must be set before use.
    /// `ProviderConfig::new(model)` is the preferred constructor.
    /// `Default::default()` yields an empty model and is only intended for
    /// `..Default::default()` struct-update syntax in tests.
    pub model: String,
    /// System prompt (passed separately from conversation messages)
    pub system: Option<String>,
    /// Maximum output tokens
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0.0–1.0 for most providers, 0.0–2.0 for OpenAI)
    pub temperature: Option<f64>,
    /// Nucleus sampling probability
    pub top_p: Option<f64>,
    /// Top-K sampling (Anthropic, Gemini)
    pub top_k: Option<u32>,
    /// Random seed for reproducibility
    pub seed: Option<u64>,
    /// Stop sequences. Maps to `stop` (OpenAI, xAI, Mistral) or `stop_sequences` (Anthropic, Cohere, Gemini).
    pub stop_sequences: Vec<String>,
    /// Available tools for the model to call
    pub tools: Vec<Tool>,
    /// How the model should choose tools
    pub tool_choice: Option<ToolChoice>,
    /// Extended thinking / reasoning token budget.
    /// Anthropic: minimum 1024.
    pub thinking_budget: Option<u32>,
    /// Whether to return thinking content in the response (Gemini: includeThoughts).
    /// Anthropic only.
    pub include_thinking: bool,
    /// Reasoning effort for o-series and reasoning-capable models.
    /// OpenAI o-series: `reasoning_effort`. xAI grok-3-mini: reasoning level.
    /// Gemini: `thinkingBudget` scaling. Bedrock: `additionalModelRequestFields`.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Desired response format (plain text, JSON mode, or structured JSON schema)
    pub response_format: Option<ResponseFormat>,
    /// Service processing tier. Valid variants differ per provider — see [`ServiceTier`].
    pub service_tier: Option<ServiceTier>,
    /// Enable built-in web search (Anthropic, OpenAI)
    pub web_search: Option<WebSearchConfig>,
    /// Arbitrary additional parameters forwarded to the provider as-is
    pub extra: HashMap<String, serde_json::Value>,
    /// End-user identifier forwarded to the provider for rate-limiting / monitoring.
    /// Anthropic: stored in `metadata.user_id`. OpenAI: sent as `user` field.
    pub user: Option<String>,
    /// Per-request HTTP timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Positive values penalize repetition of tokens already present in the text.
    /// Range: -2.0 to 2.0 (OpenAI), supported by Gemini and Cohere as well.
    pub presence_penalty: Option<f64>,
    /// Positive values penalize tokens based on how many times they've appeared so far.
    /// Range: -2.0 to 2.0 (OpenAI), supported by Gemini and Cohere as well.
    pub frequency_penalty: Option<f64>,
    /// Modify the likelihood of specified tokens — map of token ID to bias (-100 to 100).
    /// OpenAI Chat / OpenAI Responses only.
    pub logit_bias: Option<HashMap<String, i32>>,
    /// Whether to allow parallel tool calls in a single response.
    /// OpenAI: `parallel_tool_calls`. Anthropic: inverse as `disable_parallel_tool_use`.
    pub parallel_tool_calls: Option<bool>,
    /// Number of completions to generate.
    /// OpenAI Chat / OpenAI Responses only.
    pub n: Option<u32>,
    /// When true, system messages are converted to user messages wrapped in `<system>` tags.
    /// OpenAI-compatible providers only (OpenAI Chat, xAI, Mistral, etc.).
    /// Converts the system message to a user message for providers without a system role.
    pub inject_system_as_user_message: bool,
    /// Gemini safety settings — one per harm category.
    /// Gemini only.
    pub safety_settings: Vec<SafetySetting>,
    /// Application-level metadata — NOT forwarded to providers.
    /// Use `user` for provider-facing user tracking.
    pub metadata: Option<RequestMetadata>,
    /// Subset of tool names to make active for this request.
    /// When `Some`, only tools with names in this list are forwarded to the provider.
    /// Unknown names in this list are silently ignored.
    /// `None` means all tools in `tools` are active (default).
    pub active_tools: Option<Vec<String>>,
    /// Container ID to reuse a code execution sandbox from a previous response.
    /// Anthropic only.
    pub container_id: Option<String>,
    /// Geographic region hint for inference (e.g. "eu", "us"). For data residency.
    pub inference_geo: Option<String>,
    /// Request spoken audio output from the model.
    /// OpenAI Chat / OpenAI Responses only.
    /// Sets `modalities: ["text", "audio"]` and `audio: {voice, format}` automatically.
    pub audio_output: Option<AudioOutputConfig>,
    /// Built-in OpenAI tools (file_search, code_interpreter, image_generation, mcp, etc.).
    /// OpenAI Chat / OpenAI Responses only.
    /// Appended to the `tools` array in the Responses API request alongside any function tools.
    pub built_in_tools: Vec<BuiltinTool>,
    /// Run this request asynchronously in the background.
    /// OpenAI Chat / OpenAI Responses only. Requires `store: Some(true)`.
    /// Poll the response by ID to check status.
    pub background: Option<bool>,
    /// Server-side context compaction settings.
    /// OpenAI Chat / OpenAI Responses only.
    pub context_management: Option<ContextManagementConfig>,
    /// Input truncation strategy when the context window is exceeded.
    /// OpenAI Chat / OpenAI Responses only. Use `"auto"` (required for computer_use).
    pub truncation: Option<String>,
    /// How long to retain prompt cache entries. `"in_memory"` (5–60 min) or `"24h"`.
    /// OpenAI Chat / OpenAI Responses only.
    pub prompt_cache_retention: Option<String>,
    /// Cache routing key — requests sharing the same key are more likely to hit the same cache.
    /// OpenAI Chat / OpenAI Responses only.
    pub prompt_cache_key: Option<String>,
    /// Request token-level log probabilities in the response.
    /// OpenAI Chat / OpenAI Responses only.
    pub logprobs: Option<bool>,
    /// Number of top log-probability tokens to return per output token (0–20).
    /// Requires `logprobs: Some(true)`.
    /// OpenAI Chat / OpenAI Responses only.
    pub top_logprobs: Option<u8>,
    /// Whether to store this conversation in OpenAI's dashboard for evals / fine-tuning.
    /// Defaults to the project setting when `None`.
    /// OpenAI Chat / OpenAI Responses only.
    pub store: Option<bool>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::new("")
    }
}

impl ProviderConfig {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            seed: None,
            stop_sequences: Vec::new(),
            tools: Vec::new(),
            tool_choice: None,
            thinking_budget: None,
            include_thinking: false,
            reasoning_effort: None,
            response_format: None,
            service_tier: None,
            web_search: None,
            extra: HashMap::new(),
            user: None,
            timeout_ms: None,
            presence_penalty: None,
            frequency_penalty: None,
            logit_bias: None,
            parallel_tool_calls: None,
            n: None,
            inject_system_as_user_message: false,
            safety_settings: Vec::new(),
            metadata: None,
            active_tools: None,
            container_id: None,
            inference_geo: None,
            audio_output: None,
            logprobs: None,
            top_logprobs: None,
            store: None,
            built_in_tools: Vec::new(),
            background: None,
            context_management: None,
            truncation: None,
            prompt_cache_retention: None,
            prompt_cache_key: None,
        }
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = Some(tool_choice);
        self
    }

    pub fn with_thinking(mut self, budget_tokens: u32) -> Self {
        self.thinking_budget = Some(budget_tokens);
        self
    }

    pub fn with_reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }

    pub fn with_response_format(mut self, format: ResponseFormat) -> Self {
        self.response_format = Some(format);
        self
    }

    pub fn with_service_tier(mut self, tier: ServiceTier) -> Self {
        self.service_tier = Some(tier);
        self
    }

    pub fn with_web_search(mut self, config: WebSearchConfig) -> Self {
        self.web_search = Some(config);
        self
    }

    pub fn with_top_p(mut self, top_p: f64) -> Self {
        self.top_p = Some(top_p);
        self
    }

    pub fn with_top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn with_stop_sequences(mut self, stop_sequences: Vec<String>) -> Self {
        self.stop_sequences = stop_sequences;
        self
    }

    /// Insert a single extra parameter forwarded to the provider as-is.
    pub fn with_extra(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }

    /// Set the end-user identifier forwarded to the provider for monitoring.
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Set a per-request HTTP timeout.
    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms);
        self
    }

    /// Set presence penalty (discourages repeating topics already mentioned).
    pub fn with_presence_penalty(mut self, penalty: f64) -> Self {
        self.presence_penalty = Some(penalty);
        self
    }

    /// Set frequency penalty (discourages repeating the same tokens).
    pub fn with_frequency_penalty(mut self, penalty: f64) -> Self {
        self.frequency_penalty = Some(penalty);
        self
    }

    /// Set logit bias for specific tokens (OpenAI only).
    pub fn with_logit_bias(mut self, logit_bias: HashMap<String, i32>) -> Self {
        self.logit_bias = Some(logit_bias);
        self
    }

    /// Set whether parallel tool calls are allowed.
    pub fn with_parallel_tool_calls(mut self, parallel: bool) -> Self {
        self.parallel_tool_calls = Some(parallel);
        self
    }

    /// Set number of completions to generate (OpenAI only).
    pub fn with_n(mut self, n: u32) -> Self {
        self.n = Some(n);
        self
    }

    /// Convert system messages to user messages (for models without system role).
    pub fn with_inject_system_as_user_message(mut self) -> Self {
        self.inject_system_as_user_message = true;
        self
    }

    /// Set Gemini safety settings.
    pub fn with_safety_settings(mut self, settings: Vec<SafetySetting>) -> Self {
        self.safety_settings = settings;
        self
    }

    /// Attach application-level metadata (not forwarded to providers).
    pub fn with_metadata(mut self, m: RequestMetadata) -> Self {
        self.metadata = Some(m);
        self
    }

    /// Restrict which tools are forwarded to the provider for this request.
    ///
    /// Only tools whose names appear in `names` are included. Unknown names are silently ignored.
    /// Use `None` (the default) to forward all tools.
    pub fn with_active_tools(mut self, names: Vec<String>) -> Self {
        self.active_tools = Some(names);
        self
    }

    /// Reuse an existing code execution container from a previous response (Anthropic).
    pub fn with_container_id(mut self, id: impl Into<String>) -> Self {
        self.container_id = Some(id.into());
        self
    }

    /// Set a geographic region hint for inference (e.g. "eu", "us"). For data residency.
    pub fn with_inference_geo(mut self, geo: impl Into<String>) -> Self {
        self.inference_geo = Some(geo.into());
        self
    }

    /// Request spoken audio output (OpenAI). Automatically sets `modalities: ["text", "audio"]`.
    pub fn with_audio_output(mut self, config: AudioOutputConfig) -> Self {
        self.audio_output = Some(config);
        self
    }

    /// Request token-level log probabilities in the response (OpenAI Chat only).
    pub fn with_logprobs(mut self, enabled: bool) -> Self {
        self.logprobs = Some(enabled);
        self
    }

    /// Number of top log-probability tokens per output token (0–20). Implies `logprobs: true`.
    pub fn with_top_logprobs(mut self, n: u8) -> Self {
        self.top_logprobs = Some(n);
        self
    }

    /// Control whether this conversation is stored in OpenAI's dashboard (OpenAI only).
    pub fn with_store(mut self, store: bool) -> Self {
        self.store = Some(store);
        self
    }

    /// Add a single built-in tool (Responses API).
    pub fn with_built_in_tool(mut self, tool: BuiltinTool) -> Self {
        self.built_in_tools.push(tool);
        self
    }

    /// Replace the entire built-in tools list (Responses API).
    pub fn with_built_in_tools(mut self, tools: Vec<BuiltinTool>) -> Self {
        self.built_in_tools = tools;
        self
    }

    /// Run this request asynchronously in the background (Responses API only). Requires `store`.
    pub fn with_background(mut self, background: bool) -> Self {
        self.background = Some(background);
        self
    }

    /// Enable server-side context compaction (Responses API only).
    pub fn with_context_management(mut self, config: ContextManagementConfig) -> Self {
        self.context_management = Some(config);
        self
    }

    /// Set input truncation strategy, e.g. `"auto"` (Responses API only).
    pub fn with_truncation(mut self, strategy: impl Into<String>) -> Self {
        self.truncation = Some(strategy.into());
        self
    }

    /// Set prompt cache retention policy: `"in_memory"` (default, 5–60 min) or `"24h"`.
    pub fn with_prompt_cache_retention(mut self, retention: impl Into<String>) -> Self {
        self.prompt_cache_retention = Some(retention.into());
        self
    }

    /// Set a cache routing key to improve cache hit rates for requests sharing a common prefix.
    pub fn with_prompt_cache_key(mut self, key: impl Into<String>) -> Self {
        self.prompt_cache_key = Some(key.into());
        self
    }

    /// Return warnings for config fields that are not supported by `provider_name`.
    ///
    /// Provider name values: `"anthropic"`, `"openai"`, `"openai-responses"`, `"bedrock"`,
    /// `"gemini"`, `"cohere"`, `"mistral"`, `"xai"`, `"registry"`, `"mock"`, `"unknown"`.
    ///
    /// Does not validate the `model` field — the provider is responsible for that.
    pub fn validate(&self, provider_name: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        let openai_only = matches!(provider_name, "openai" | "openai-responses");
        let gemini_only = provider_name == "gemini";
        let anthropic_only = provider_name == "anthropic";

        if !openai_only {
            if !self.built_in_tools.is_empty() {
                warnings.push(format!(
                    "built_in_tools is only supported by openai/openai-responses (provider: {provider_name})"
                ));
            }
            if self.background.is_some() {
                warnings.push(format!(
                    "background is only supported by openai/openai-responses (provider: {provider_name})"
                ));
            }
            if self.context_management.is_some() {
                warnings.push(format!(
                    "context_management is only supported by openai/openai-responses (provider: {provider_name})"
                ));
            }
            if self.truncation.is_some() {
                warnings.push(format!(
                    "truncation is only supported by openai/openai-responses (provider: {provider_name})"
                ));
            }
            if self.logprobs.is_some() {
                warnings.push(format!(
                    "logprobs is only supported by openai/openai-responses (provider: {provider_name})"
                ));
            }
            if self.top_logprobs.is_some() {
                warnings.push(format!(
                    "top_logprobs is only supported by openai/openai-responses (provider: {provider_name})"
                ));
            }
            if self.n.is_some_and(|n| n > 1) {
                warnings.push(format!(
                    "n > 1 is only supported by openai/openai-responses (provider: {provider_name})"
                ));
            }
            if self.inject_system_as_user_message {
                warnings.push(format!(
                    "inject_system_as_user_message is only supported by openai/openai-responses (provider: {provider_name})"
                ));
            }
            if self.audio_output.is_some() {
                warnings.push(format!(
                    "audio_output is only supported by openai/openai-responses (provider: {provider_name})"
                ));
            }
            if self.logit_bias.as_ref().is_some_and(|b| !b.is_empty()) {
                warnings.push(format!(
                    "logit_bias is only supported by openai/openai-responses (provider: {provider_name})"
                ));
            }
        }
        if !gemini_only && !self.safety_settings.is_empty() {
            warnings.push(format!(
                "safety_settings is only supported by gemini (provider: {provider_name})"
            ));
        }
        if !anthropic_only && self.container_id.is_some() {
            warnings.push(format!(
                "container_id is only supported by anthropic (provider: {provider_name})"
            ));
        }
        if self.thinking_budget.is_some() && self.reasoning_effort.is_some() {
            warnings.push(
                "thinking_budget and reasoning_effort are mutually exclusive; \
                 only one should be set"
                    .into(),
            );
        }
        warnings
    }
}
