use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

use crate::error::ProviderError;

// ---------------------------------------------------------------------------
// Gemini safety settings
// ---------------------------------------------------------------------------

/// Harm category for Gemini safety settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SafetyCategory {
    HarassmentContent,
    HateSpeech,
    SexuallyExplicit,
    DangerousContent,
    CivicIntegrity,
}

impl SafetyCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HarassmentContent => "HARM_CATEGORY_HARASSMENT",
            Self::HateSpeech => "HARM_CATEGORY_HATE_SPEECH",
            Self::SexuallyExplicit => "HARM_CATEGORY_SEXUALLY_EXPLICIT",
            Self::DangerousContent => "HARM_CATEGORY_DANGEROUS_CONTENT",
            Self::CivicIntegrity => "HARM_CATEGORY_CIVIC_INTEGRITY",
        }
    }
}

impl std::fmt::Display for SafetyCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Harm block threshold for Gemini safety settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SafetyThreshold {
    BlockNone,
    BlockLowAndAbove,
    BlockMediumAndAbove,
    BlockOnlyHigh,
}

impl SafetyThreshold {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BlockNone => "BLOCK_NONE",
            Self::BlockLowAndAbove => "BLOCK_LOW_AND_ABOVE",
            Self::BlockMediumAndAbove => "BLOCK_MEDIUM_AND_ABOVE",
            Self::BlockOnlyHigh => "BLOCK_ONLY_HIGH",
        }
    }
}

impl std::fmt::Display for SafetyThreshold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Gemini safety setting — one category + threshold pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetySetting {
    pub category: SafetyCategory,
    pub threshold: SafetyThreshold,
}

// ---------------------------------------------------------------------------
// Logprobs (OpenAI)
// ---------------------------------------------------------------------------

/// Per-token logprob alternative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopLogprob {
    pub token: String,
    pub logprob: f64,
    pub bytes: Option<Vec<u8>>,
}

/// Per-token logprob with alternatives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenLogprob {
    pub token: String,
    pub logprob: f64,
    pub bytes: Option<Vec<u8>>,
    pub top_logprobs: Vec<TopLogprob>,
}

// ---------------------------------------------------------------------------
// Grounding metadata (Gemini web search)
// ---------------------------------------------------------------------------

/// A single grounding source chunk (web search result).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingChunk {
    pub title: Option<String>,
    pub uri: Option<String>,
}

/// Grounding metadata returned when Gemini performs web search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingMetadata {
    pub chunks: Vec<GroundingChunk>,
    pub search_queries: Vec<String>,
}

// ---------------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

// ---------------------------------------------------------------------------
// Media source — shared across image / audio / video / document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Base64Data {
    /// MIME type, e.g. "image/jpeg", "audio/mp3", "application/pdf"
    pub media_type: String,
    /// Base64-encoded bytes
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Location {
    pub uri: String,
    pub bucket_owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MediaSource {
    Base64(Base64Data),
    Url(String),
    S3(S3Location),
    /// Gemini Files API / Cloud Storage URI
    FileUri {
        uri: String,
        media_type: String,
    },
}

impl MediaSource {
    pub fn base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self::Base64(Base64Data {
            media_type: media_type.into(),
            data: data.into(),
        })
    }

    pub fn url(url: impl Into<String>) -> Self {
        Self::Url(url.into())
    }

    /// Build from raw bytes — encodes to base64 automatically.
    pub fn from_bytes(media_type: impl Into<String>, bytes: &[u8]) -> Self {
        use base64::Engine;
        Self::Base64(Base64Data {
            media_type: media_type.into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }
}

// ---------------------------------------------------------------------------
// Format enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Jpeg,
    Png,
    Gif,
    Webp,
    Heic,
    Heif,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Mp3,
    Wav,
    Aac,
    Flac,
    Ogg,
    Webm,
    M4a,
    Opus,
    Aiff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoFormat {
    Mp4,
    Mov,
    Mkv,
    Webm,
    Avi,
    Flv,
    Mpeg,
    Wmv,
    ThreeGp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentFormat {
    Pdf,
    Csv,
    Doc,
    Docx,
    Xls,
    Xlsx,
    Html,
    Txt,
    Md,
}

// ---------------------------------------------------------------------------
// Media content structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    pub source: MediaSource,
    pub format: Option<ImageFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioContent {
    pub source: MediaSource,
    pub format: AudioFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoContent {
    pub source: MediaSource,
    pub format: VideoFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentContent {
    pub source: MediaSource,
    pub format: DocumentFormat,
    /// Optional name / title for the document
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// Tool blocks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseBlock {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultBlock {
    pub tool_use_id: String,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// Thinking / reasoning block
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingBlock {
    pub thinking: String,
    /// Cryptographic signature (Anthropic / Bedrock). Must be passed back
    /// unmodified in multi-turn conversations.
    pub signature: Option<String>,
}

// ---------------------------------------------------------------------------
// Content block — the union of all content types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text(String),
    Image(ImageContent),
    Audio(AudioContent),
    Video(VideoContent),
    Document(DocumentContent),
    ToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
    Thinking(ThinkingBlock),
}

impl ContentBlock {
    pub fn text(t: impl Into<String>) -> Self {
        Self::Text(t.into())
    }

    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self::ToolUse(ToolUseBlock {
            id: id.into(),
            name: name.into(),
            input,
        })
    }

    pub fn tool_result(tool_use_id: impl Into<String>, content: Vec<ContentBlock>) -> Self {
        Self::ToolResult(ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content,
            is_error: false,
        })
    }

    pub fn tool_error(tool_use_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self::ToolResult(ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: vec![ContentBlock::text(error.into())],
            is_error: true,
        })
    }

    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }
    pub fn is_tool_use(&self) -> bool {
        matches!(self, Self::ToolUse(_))
    }
    pub fn is_tool_result(&self) -> bool {
        matches!(self, Self::ToolResult(_))
    }
    pub fn is_thinking(&self) -> bool {
        matches!(self, Self::Thinking(_))
    }

    pub fn as_text(&self) -> Option<&str> {
        if let Self::Text(t) = self {
            Some(t)
        } else {
            None
        }
    }

    pub fn as_tool_use(&self) -> Option<&ToolUseBlock> {
        if let Self::ToolUse(t) = self {
            Some(t)
        } else {
            None
        }
    }

    pub fn as_tool_result(&self) -> Option<&ToolResultBlock> {
        if let Self::ToolResult(t) = self {
            Some(t)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    /// Optional name to distinguish participants with the same role (OpenAI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Anthropic prompt caching: marks this message for caching.
    /// Applied to the last content block of the message in the API request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::text(text)],
            name: None,
            cache_control: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::text(text)],
            name: None,
            cache_control: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentBlock::text(text)],
            name: None,
            cache_control: None,
        }
    }

    pub fn with_content(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            name: None,
            cache_control: None,
        }
    }

    /// Set the participant name (used by OpenAI to distinguish same-role participants).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Mark this message for Anthropic prompt caching.
    pub fn with_cache_control(mut self, cache_control: CacheControl) -> Self {
        self.cache_control = Some(cache_control);
        self
    }

    /// Build a tool result message from (tool_use_id, result_text) pairs.
    pub fn with_tool_results(results: Vec<(String, String)>) -> Self {
        let content = results
            .into_iter()
            .map(|(id, text)| ContentBlock::tool_result(id, vec![ContentBlock::text(text)]))
            .collect();
        Self {
            role: Role::User,
            content,
            name: None,
            cache_control: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// JSON Schema object describing the tool's input parameters.
    pub input_schema: serde_json::Value,
    /// Enable strict schema validation (OpenAI Chat/Responses only).
    /// When true, `additionalProperties: false` is automatically added
    /// and OpenAI validates that all call arguments match the schema exactly.
    /// Ignored by other providers.
    pub strict: bool,
    /// Example inputs for documentation / few-shot prompting (not forwarded to providers).
    #[serde(default)]
    pub input_examples: Vec<serde_json::Value>,
}

impl Tool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            strict: false,
            input_examples: vec![],
        }
    }

    /// Enable strict schema validation for this tool (OpenAI).
    pub fn with_strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Attach example inputs for documentation / few-shot prompting.
    pub fn with_input_examples(mut self, examples: Vec<serde_json::Value>) -> Self {
        self.input_examples = examples;
        self
    }
}

// ---------------------------------------------------------------------------
// Tool choice
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    /// Model decides whether to use tools (default)
    Auto,
    /// Model must use at least one tool
    Any,
    /// Do not use tools
    None,
    /// Force a specific tool by name
    Tool { name: String },
}

// ---------------------------------------------------------------------------
// Reasoning effort (OpenAI o-series, xAI, DeepSeek)
// ---------------------------------------------------------------------------

/// Reasoning effort level for models with extended thinking.
///
/// - OpenAI o-series: sent as `reasoning_effort`
/// - Anthropic (Opus 4.6, Sonnet 4.6, Opus 4.5): sent as `output_config.effort`
///   `Max` is only valid for Anthropic claude-opus-4-6.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    /// Maximum effort — only available on Anthropic claude-opus-4-6.
    Max,
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Response format (structured output)
// ---------------------------------------------------------------------------

/// Desired output format for the model's response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseFormat {
    /// Plain text (default)
    Text,
    /// Unstructured JSON — model must output valid JSON, no schema enforced
    Json,
    /// Structured JSON output validated against a schema
    JsonSchema {
        /// Schema name (used as the response_format name in the API)
        name: String,
        /// JSON Schema object describing the expected structure
        schema: serde_json::Value,
        /// Strict validation (default: true). Automatically adds
        /// `additionalProperties: false` to the schema.
        strict: bool,
    },
}

impl ResponseFormat {
    /// Shorthand for `JsonSchema` with `strict: true`.
    pub fn json_schema_strict(name: impl Into<String>, schema: serde_json::Value) -> Self {
        Self::JsonSchema {
            name: name.into(),
            schema,
            strict: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Service tier (OpenAI)
// ---------------------------------------------------------------------------

/// Processing tier for OpenAI requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceTier {
    Auto,
    Default,
    /// Flex — lower cost, potentially slower
    Flex,
}

impl ServiceTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Default => "default",
            Self::Flex => "flex",
        }
    }
}

impl std::fmt::Display for ServiceTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Cache control (Anthropic prompt caching)
// ---------------------------------------------------------------------------

/// Anthropic prompt cache control — marks a message or system prompt for caching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CacheControl {
    /// Ephemeral cache (5-minute TTL, 1.25× token cost for cache writes)
    Ephemeral,
}

// ---------------------------------------------------------------------------
// Web search configuration
// ---------------------------------------------------------------------------

/// Built-in web search tool configuration (Anthropic, OpenAI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchConfig {
    /// Maximum number of web searches the model may perform
    pub max_uses: Option<u32>,
    /// Domain whitelist — only these domains are searched (cannot combine with blocked_domains)
    pub allowed_domains: Option<Vec<String>>,
    /// Domain blacklist — these domains are excluded from results
    pub blocked_domains: Option<Vec<String>>,
}

impl WebSearchConfig {
    pub fn new() -> Self {
        Self {
            max_uses: None,
            allowed_domains: None,
            blocked_domains: None,
        }
    }

    pub fn with_max_uses(mut self, max_uses: u32) -> Self {
        self.max_uses = Some(max_uses);
        self
    }

    pub fn with_allowed_domains(mut self, domains: Vec<impl Into<String>>) -> Self {
        self.allowed_domains = Some(domains.into_iter().map(|s| s.into()).collect());
        self
    }

    pub fn with_blocked_domains(mut self, domains: Vec<impl Into<String>>) -> Self {
        self.blocked_domains = Some(domains.into_iter().map(|s| s.into()).collect());
        self
    }
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Provider configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Model identifier (provider-specific)
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
    /// Stop sequences
    pub stop_sequences: Vec<String>,
    /// Available tools for the model to call
    pub tools: Vec<Tool>,
    /// How the model should choose tools
    pub tool_choice: Option<ToolChoice>,
    /// Extended thinking / reasoning token budget.
    /// Anthropic: minimum 1024. Gemini: `thinkingBudget`. Bedrock: via additionalModelRequestFields.
    pub thinking_budget: Option<u32>,
    /// Whether to return thinking content in the response (Gemini: includeThoughts)
    pub include_thinking: bool,
    /// Reasoning effort for o-series and reasoning-capable models.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Desired response format (plain text, JSON mode, or structured JSON schema)
    pub response_format: Option<ResponseFormat>,
    /// OpenAI service processing tier
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
    /// OpenAI only.
    pub logit_bias: Option<HashMap<String, i32>>,
    /// Whether to allow parallel tool calls in a single response.
    /// OpenAI: `parallel_tool_calls`. Anthropic: inverse as `disable_parallel_tool_use`.
    pub parallel_tool_calls: Option<bool>,
    /// Number of completions to generate. OpenAI only.
    pub n: Option<u32>,
    /// When true, system messages are converted to user messages wrapped in `<system>` tags.
    /// Use for models/providers that don't support a dedicated system role.
    pub inject_system_as_user_message: bool,
    /// Gemini safety settings — one per harm category.
    pub safety_settings: Vec<SafetySetting>,
    /// Application-level metadata — NOT forwarded to providers.
    /// Use `user` for provider-facing user tracking.
    pub metadata: Option<RequestMetadata>,
    /// Subset of tool names to make active for this request.
    /// When `Some`, only tools with names in this list are forwarded to the provider.
    /// Unknown names in this list are silently ignored.
    /// `None` means all tools in `tools` are active (default).
    pub active_tools: Option<Vec<String>>,
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
}

impl Default for ProviderConfig {
    /// Creates a `ProviderConfig` with an empty model string.
    ///
    /// Note: the `model` field must be set before use; an empty model string
    /// will cause providers to return an error. Prefer `ProviderConfig::new("model-id")`.
    fn default() -> Self {
        Self::new("")
    }
}

// ---------------------------------------------------------------------------
// Cost estimation
// ---------------------------------------------------------------------------

/// Estimated cost for a single request.
#[derive(Debug, Clone, Default)]
pub struct CostEstimate {
    /// Cost for input/prompt tokens (USD)
    pub input_cost: f64,
    /// Cost for output/completion tokens (USD)
    pub output_cost: f64,
    /// Total cost (USD)
    pub total: f64,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Tokens served from prompt cache
    pub cache_read_tokens: u64,
    /// Tokens written to prompt cache
    pub cache_write_tokens: u64,
    /// Tokens used for reasoning / thinking
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
}

impl Usage {
    pub fn with_totals(mut self) -> Self {
        if self.total_tokens == 0 {
            self.total_tokens = self.input_tokens + self.output_tokens;
        }
        self
    }

    /// Estimate cost based on per-million-token prices.
    pub fn estimate_cost(&self, input_per_1m: f64, output_per_1m: f64) -> CostEstimate {
        let input_cost = (self.input_tokens as f64 / 1_000_000.0) * input_per_1m;
        let output_cost = (self.output_tokens as f64 / 1_000_000.0) * output_per_1m;
        CostEstimate {
            input_cost,
            output_cost,
            total: input_cost + output_cost,
        }
    }
}

impl std::ops::Add for Usage {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            input_tokens: self.input_tokens + rhs.input_tokens,
            output_tokens: self.output_tokens + rhs.output_tokens,
            cache_read_tokens: self.cache_read_tokens + rhs.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens + rhs.cache_write_tokens,
            reasoning_tokens: self.reasoning_tokens + rhs.reasoning_tokens,
            total_tokens: self.total_tokens + rhs.total_tokens,
        }
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens += rhs.input_tokens;
        self.output_tokens += rhs.output_tokens;
        self.cache_read_tokens += rhs.cache_read_tokens;
        self.cache_write_tokens += rhs.cache_write_tokens;
        self.reasoning_tokens += rhs.reasoning_tokens;
        self.total_tokens += rhs.total_tokens;
    }
}

/// Accumulates usage statistics across multiple requests.
#[derive(Debug, Clone, Default)]
pub struct UsageAccumulator {
    total: Usage,
    count: usize,
}

impl UsageAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, usage: Usage) {
        self.total += usage;
        self.count += 1;
    }

    pub fn total(&self) -> &Usage {
        &self.total
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StopReason {
    #[default]
    EndTurn,
    MaxTokens,
    StopSequence(String),
    ToolUse,
    ContentFilter,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub content: Vec<ContentBlock>,
    pub usage: Usage,
    pub stop_reason: StopReason,
    /// Model ID returned by the API (may differ from requested model)
    pub model: Option<String>,
    /// Provider-assigned response/interaction ID (Gemini Interactions, OpenAI Responses, etc.)
    pub id: Option<String>,
    /// Token-level log probabilities (OpenAI, when requested via logprobs=true)
    pub logprobs: Option<Vec<TokenLogprob>>,
    /// Grounding metadata from Gemini web search
    pub grounding_metadata: Option<GroundingMetadata>,
    /// Parameters that were silently dropped or truncated by the provider.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Raw request body sent to the provider (debug use; populated by some providers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<serde_json::Value>,
}

impl Response {
    /// Concatenates all `Text` content blocks into a single string.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Text(t) = b {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Returns the first text block's content.
    pub fn first_text(&self) -> Option<&str> {
        self.content.iter().find_map(|b| b.as_text())
    }

    /// Returns all tool use blocks.
    pub fn tool_uses(&self) -> Vec<&ToolUseBlock> {
        self.content
            .iter()
            .filter_map(|b| b.as_tool_use())
            .collect()
    }

    /// Returns true if the response contains at least one tool use block.
    pub fn has_tool_use(&self) -> bool {
        self.content.iter().any(|b| b.is_tool_use())
    }

    /// Returns all thinking blocks.
    pub fn thinking_content(&self) -> Vec<&ThinkingBlock> {
        self.content
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Thinking(t) = b {
                    Some(t)
                } else {
                    None
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Model listing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// Unix timestamp (seconds) when the model was created/published
    pub created_at: Option<u64>,
}

// ---------------------------------------------------------------------------
// Token counting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCount {
    pub input_tokens: u64,
}

// ---------------------------------------------------------------------------
// Embeddings
// ---------------------------------------------------------------------------

/// Task type hint to optimize embeddings for specific use cases (Gemini).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EmbeddingTaskType {
    RetrievalQuery,
    RetrievalDocument,
    SemanticSimilarity,
    Classification,
    Clustering,
    QuestionAnswering,
    FactVerification,
    CodeRetrievalQuery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    /// Texts to embed
    pub inputs: Vec<String>,
    /// Desired output dimension (provider-dependent, optional)
    pub dimensions: Option<u32>,
    /// Task type hint for optimization (Gemini)
    pub task_type: Option<EmbeddingTaskType>,
}

impl EmbeddingRequest {
    pub fn new(inputs: Vec<impl Into<String>>) -> Self {
        Self {
            inputs: inputs.into_iter().map(|s| s.into()).collect(),
            dimensions: None,
            task_type: None,
        }
    }

    pub fn single(input: impl Into<String>) -> Self {
        Self::new(vec![input.into()])
    }

    pub fn with_dimensions(mut self, dimensions: u32) -> Self {
        self.dimensions = Some(dimensions);
        self
    }

    pub fn with_task_type(mut self, task_type: EmbeddingTaskType) -> Self {
        self.task_type = Some(task_type);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    /// One vector per input string, in the same order
    pub embeddings: Vec<Vec<f32>>,
    pub model: Option<String>,
    pub usage: Usage,
}

// ---------------------------------------------------------------------------
// Streaming event types
// ---------------------------------------------------------------------------

/// Indicates the type of content block that is starting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockStart {
    Text,
    ToolUse {
        id: String,
        name: String,
    },
    Thinking,
    /// Audio output block (OpenAI audio models).
    Audio,
}

/// An incremental delta within a content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentDelta {
    Text {
        text: String,
    },
    /// Partial JSON string for a tool input; accumulate and parse at stop.
    ToolInput {
        partial_json: String,
    },
    Thinking {
        thinking: String,
    },
    /// Cryptographic signature emitted at the end of a thinking block.
    Signature {
        signature: String,
    },
    /// Base64-encoded audio chunk (OpenAI audio output streaming).
    AudioData {
        b64_data: String,
    },
}

// ---------------------------------------------------------------------------
// Token estimation and message truncation
// ---------------------------------------------------------------------------

/// Heuristic token count estimate: approximately 4 characters per token.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Remove oldest non-system messages until estimated token count is under `max_tokens`.
/// System messages are never removed.
pub fn truncate_messages(mut messages: Vec<Message>, max_tokens: usize) -> Vec<Message> {
    loop {
        let total: usize = messages
            .iter()
            .flat_map(|m| &m.content)
            .map(|b| estimate_tokens(b.as_text().unwrap_or("")))
            .sum();
        if total <= max_tokens {
            break;
        }
        let pos = messages.iter().position(|m| m.role != Role::System);
        match pos {
            Some(i) => {
                messages.remove(i);
            }
            None => break,
        }
    }
    messages
}

// ---------------------------------------------------------------------------
// Message validation
// ---------------------------------------------------------------------------

/// Validate a message list and return a list of warning strings.
///
/// Checks for:
/// - Consecutive messages with the same role
/// - Assistant messages with no content blocks
/// - Tool result blocks without a preceding tool use block
pub fn validate_messages(messages: &[Message]) -> Vec<String> {
    let mut warnings = Vec::new();

    // Collect all tool use IDs from assistant messages
    let mut tool_use_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for msg in messages {
        if msg.role == Role::Assistant {
            for block in &msg.content {
                if let ContentBlock::ToolUse(tu) = block {
                    tool_use_ids.insert(&tu.id);
                }
            }
        }
    }

    for (i, msg) in messages.iter().enumerate() {
        // Consecutive same-role
        if i > 0 && messages[i - 1].role == msg.role {
            warnings.push(format!(
                "Message {} and {} both have role {:?}",
                i - 1,
                i,
                msg.role
            ));
        }

        // Assistant with no content
        if msg.role == Role::Assistant && msg.content.is_empty() {
            warnings.push(format!("Message {} (assistant) has no content blocks", i));
        }

        // Tool result without matching tool use
        for block in &msg.content {
            if let ContentBlock::ToolResult(tr) = block
                && !tool_use_ids.contains(tr.tool_use_id.as_str())
            {
                warnings.push(format!(
                    "Message {}: tool result references unknown tool_use_id '{}'",
                    i, tr.tool_use_id
                ));
            }
        }
    }

    warnings
}

// ---------------------------------------------------------------------------
// Conversation builder
// ---------------------------------------------------------------------------

/// Fluent builder for assembling conversation message lists.
#[derive(Debug, Default)]
pub struct ConversationBuilder {
    messages: Vec<Message>,
    system: Option<String>,
}

impl ConversationBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the system prompt (stored separately, not as a message).
    pub fn system(mut self, s: impl Into<String>) -> Self {
        self.system = Some(s.into());
        self
    }

    pub fn user(mut self, s: impl Into<String>) -> Self {
        self.messages.push(Message::user(s));
        self
    }

    pub fn assistant(mut self, s: impl Into<String>) -> Self {
        self.messages.push(Message::assistant(s));
        self
    }

    pub fn message(mut self, m: Message) -> Self {
        self.messages.push(m);
        self
    }

    /// Returns only the message list.
    ///
    /// Note: the system prompt set via `.system()` is NOT included in the returned list.
    /// Use `build_with_config()` to inject the system prompt into a `ProviderConfig`.
    pub fn build_messages(self) -> Vec<Message> {
        self.messages
    }

    /// Returns `(messages, config)` with the system prompt injected into config.
    pub fn build_with_config(self, config: ProviderConfig) -> (Vec<Message>, ProviderConfig) {
        let config = if let Some(system) = self.system {
            config.with_system(system)
        } else {
            config
        };
        (self.messages, config)
    }
}

// ---------------------------------------------------------------------------
// Streaming event types
// ---------------------------------------------------------------------------

/// Events emitted by the streaming provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        role: Role,
    },
    ContentBlockStart {
        index: usize,
        block: ContentBlockStart,
    },
    ContentBlockDelta {
        index: usize,
        delta: ContentDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageStop {
        stop_reason: StopReason,
    },
    Metadata {
        usage: Usage,
        model: Option<String>,
        /// Provider-assigned response ID (Gemini Interactions, OpenAI Responses, etc.)
        id: Option<String>,
    },
    /// Complete inline media block (e.g. Gemini image output in chat). Not streamed incrementally.
    InlineData {
        index: usize,
        media_type: String,
        b64_data: String,
    },
}

// ---------------------------------------------------------------------------
// RequestMetadata — application-level tracking, not forwarded to providers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestMetadata {
    pub request_id: Option<String>,
    pub user_id: Option<String>,
    pub tags: HashMap<String, String>,
}

impl RequestMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }

    pub fn with_user_id(mut self, id: impl Into<String>) -> Self {
        self.user_id = Some(id.into());
        self
    }

    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.tags.insert(key.into(), value.into());
        self
    }
}

// ---------------------------------------------------------------------------
// PromptTemplate — {{variable}} substitution
// ---------------------------------------------------------------------------

/// Simple template with `{{variable}}` placeholders.
pub struct PromptTemplate {
    pub template: String,
}

impl PromptTemplate {
    pub fn new(template: impl Into<String>) -> Self {
        Self {
            template: template.into(),
        }
    }

    /// Render the template by substituting `{{key}}` with values from `vars`.
    /// Returns `Err(Config)` if any placeholder is missing or unclosed.
    pub fn render(&self, vars: &HashMap<&str, &str>) -> Result<String, ProviderError> {
        let mut result = self.template.clone();
        let mut pos = 0;
        while let Some(open_rel) = result[pos..].find("{{") {
            let open = pos + open_rel;
            let close_rel = result[open..]
                .find("}}")
                .ok_or_else(|| ProviderError::Config("Unclosed '{{' in template".into()))?;
            let close = open + close_rel;
            let key = result[open + 2..close].to_string();
            let value = vars.get(key.as_str()).ok_or_else(|| {
                ProviderError::Config(format!("Template variable '{}' not provided", key))
            })?;
            result = format!("{}{}{}", &result[..open], value, &result[close + 2..]);
            pos = open + value.len();
        }
        Ok(result)
    }

    /// Convenience: replaces `{{input}}` with the given value.
    pub fn render_input(&self, input: &str) -> String {
        self.template.replace("{{input}}", input)
    }
}

// ---------------------------------------------------------------------------
// ModelCapability — static capability table
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCapability {
    /// Image / screenshot understanding (input)
    Vision,
    /// Audio understanding / input (e.g. speech-to-text capable)
    AudioInput,
    /// Audio output / generation (e.g. TTS capable)
    AudioOutput,
    /// Video understanding (input)
    Video,
    FunctionCalling,
    StructuredOutput,
    Streaming,
    ExtendedThinking,
    Embeddings,
    ImageGeneration,
    /// Video generation output (e.g. Veo)
    VideoGeneration,
    WebSearch,
}

/// Returns the known capabilities of a model by `starts_with` prefix matching.
/// More specific prefixes must appear before broader ones in the table.
pub fn model_capabilities(model: &str) -> Vec<ModelCapability> {
    use ModelCapability::*;

    // Each entry: (prefix, capabilities)
    // Order: most-specific first within each family.
    let table: &[(&str, &[ModelCapability])] = &[
        // ── Anthropic Claude ────────────────────────────────────────────────
        // Claude 4 Opus / Sonnet / Haiku — all have vision + extended thinking
        (
            "claude-opus-4",
            &[
                Vision,
                FunctionCalling,
                Streaming,
                ExtendedThinking,
                StructuredOutput,
            ],
        ),
        (
            "claude-sonnet-4",
            &[
                Vision,
                FunctionCalling,
                Streaming,
                ExtendedThinking,
                StructuredOutput,
            ],
        ),
        (
            "claude-haiku-4",
            &[
                Vision,
                FunctionCalling,
                Streaming,
                ExtendedThinking,
                StructuredOutput,
            ],
        ),
        // Claude 3.7 Sonnet — extended thinking (hybrid reasoning)
        (
            "claude-3-7-sonnet",
            &[
                Vision,
                FunctionCalling,
                Streaming,
                ExtendedThinking,
                StructuredOutput,
            ],
        ),
        // Claude 3.5
        (
            "claude-3-5",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        // Claude 3 named models (before catch-all)
        (
            "claude-3-opus",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        (
            "claude-3-sonnet",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        (
            "claude-3-haiku",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        ("claude-3", &[Vision, FunctionCalling, Streaming]),
        // Claude 2 / Instant
        ("claude-2", &[FunctionCalling, Streaming]),
        ("claude-instant", &[FunctionCalling, Streaming]),
        // ── OpenAI GPT ──────────────────────────────────────────────────────
        // gpt-4o-audio-preview (most specific before gpt-4o-mini and gpt-4o)
        (
            "gpt-4o-audio-preview",
            &[
                AudioInput,
                AudioOutput,
                Vision,
                FunctionCalling,
                Streaming,
                StructuredOutput,
            ],
        ),
        // GPT-4o Mini (before gpt-4o to avoid false match)
        (
            "gpt-4o-mini",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        // GPT-4o — adds AudioInput + AudioOutput + WebSearch
        (
            "gpt-4o",
            &[
                Vision,
                AudioInput,
                AudioOutput,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                WebSearch,
            ],
        ),
        // GPT-4.1 variants (nano/mini before base)
        (
            "gpt-4.1-nano",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        (
            "gpt-4.1-mini",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        (
            "gpt-4.1",
            &[
                Vision,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                WebSearch,
            ],
        ),
        // GPT-4 Turbo (before gpt-4)
        (
            "gpt-4-turbo",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        (
            "gpt-4",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        ("gpt-3.5", &[FunctionCalling, Streaming]),
        // ── OpenAI o-series (reasoning) ─────────────────────────────────────
        // o1-mini / o1-preview lack vision; full o1 added vision later
        (
            "o1-mini",
            &[
                FunctionCalling,
                Streaming,
                StructuredOutput,
                ExtendedThinking,
            ],
        ),
        (
            "o1-preview",
            &[
                FunctionCalling,
                Streaming,
                StructuredOutput,
                ExtendedThinking,
            ],
        ),
        (
            "o1",
            &[
                Vision,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                ExtendedThinking,
            ],
        ),
        (
            "o3-mini",
            &[
                FunctionCalling,
                Streaming,
                StructuredOutput,
                ExtendedThinking,
            ],
        ),
        (
            "o3",
            &[
                Vision,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                ExtendedThinking,
            ],
        ),
        (
            "o4-mini",
            &[
                Vision,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                ExtendedThinking,
            ],
        ),
        (
            "o4",
            &[
                Vision,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                ExtendedThinking,
            ],
        ),
        // ── OpenAI TTS / Transcription ───────────────────────────────────────
        ("tts-1-hd", &[AudioOutput]),
        ("tts-1", &[AudioOutput]),
        ("whisper-", &[]),
        // ── Google Gemini ────────────────────────────────────────────────────
        // Gemini 3.x — adds ExtendedThinking over 2.x
        (
            "gemini-3",
            &[
                Vision,
                AudioInput,
                Video,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                ExtendedThinking,
                WebSearch,
            ],
        ),
        // Gemini 2.5 — thinking-capable
        (
            "gemini-2.5",
            &[
                Vision,
                AudioInput,
                Video,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                ExtendedThinking,
                WebSearch,
            ],
        ),
        // Gemini 2.0
        (
            "gemini-2.0",
            &[
                Vision,
                AudioInput,
                Video,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                WebSearch,
            ],
        ),
        // Gemini 2.x catch-all
        (
            "gemini-2",
            &[
                Vision,
                AudioInput,
                Video,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                WebSearch,
            ],
        ),
        // Gemini 1.5
        (
            "gemini-1.5",
            &[
                Vision,
                AudioInput,
                Video,
                FunctionCalling,
                Streaming,
                StructuredOutput,
                WebSearch,
            ],
        ),
        // Gemini 1.0
        ("gemini-1.0", &[Vision, FunctionCalling, Streaming]),
        // Gemini embedding models
        ("gemini-embedding", &[Embeddings]),
        // ── Cohere ──────────────────────────────────────────────────────────
        // command-a variants (more specific before base)
        (
            "command-a-reasoning",
            &[FunctionCalling, Streaming, ExtendedThinking, WebSearch],
        ),
        ("command-a-vision", &[Vision, FunctionCalling, Streaming]),
        ("command-a", &[FunctionCalling, Streaming, WebSearch]),
        // command-r variants
        ("command-r7b", &[FunctionCalling, Streaming, WebSearch]),
        ("command-r-plus", &[FunctionCalling, Streaming, WebSearch]),
        ("command-r", &[FunctionCalling, Streaming, WebSearch]),
        ("command", &[Streaming]),
        // ── Embedding models ─────────────────────────────────────────────────
        ("text-embedding-gecko", &[Embeddings]), // Vertex AI legacy
        ("text-embedding-", &[Embeddings]),
        ("embed-", &[Embeddings]),
        // ── Generation models ────────────────────────────────────────────────
        ("dall-e", &[ImageGeneration]),
        ("imagen", &[ImageGeneration]),
        ("veo", &[VideoGeneration]),
    ];

    for (prefix, caps) in table {
        if model.starts_with(prefix) {
            return caps.to_vec();
        }
    }

    vec![Streaming]
}

// ---------------------------------------------------------------------------
// Image generation types
// ---------------------------------------------------------------------------

/// Canonical image sizes understood by DALL-E and Imagen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSize {
    S256x256,
    S512x512,
    S1024x1024,
    /// Portrait (DALL-E 3, Imagen 9:16)
    S1024x1792,
    /// Landscape (DALL-E 3, Imagen 16:9)
    S1792x1024,
    /// Provider-specific size string (e.g. "768x768")
    Custom(String),
}

impl ImageSize {
    pub fn as_str(&self) -> &str {
        match self {
            Self::S256x256 => "256x256",
            Self::S512x512 => "512x512",
            Self::S1024x1024 => "1024x1024",
            Self::S1024x1792 => "1024x1792",
            Self::S1792x1024 => "1792x1024",
            Self::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for ImageSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ImageSize {
    /// Map to Imagen/Veo aspect ratio string.
    pub fn as_aspect_ratio(&self) -> &str {
        match self {
            Self::S1792x1024 => "16:9",
            Self::S1024x1792 => "9:16",
            _ => "1:1",
        }
    }
}

/// Image quality hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageQuality {
    Standard,
    /// HD quality (DALL-E 3)
    Hd,
    Custom(String),
}

impl ImageQuality {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Standard => "standard",
            Self::Hd => "hd",
            Self::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for ImageQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Image style hint (DALL-E 3 only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageStyle {
    /// Bold, saturated, stylized look.
    Vivid,
    /// More natural, less hyper-real look.
    Natural,
}

impl ImageStyle {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Vivid => "vivid",
            Self::Natural => "natural",
        }
    }
}

impl std::fmt::Display for ImageStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Desired output format for generated images.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ImageOutputFormat {
    /// Return a URL pointing to the image (default).
    #[default]
    Url,
    /// Return base64-encoded image data.
    B64Json,
}

impl ImageOutputFormat {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Url => "url",
            Self::B64Json => "b64_json",
        }
    }
}

impl std::fmt::Display for ImageOutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Request to generate one or more images from a text prompt.
#[derive(Debug, Clone)]
pub struct ImageGenerationRequest {
    pub model: String,
    pub prompt: String,
    /// Number of images to generate (default 1).
    pub n: Option<u32>,
    pub size: Option<ImageSize>,
    pub quality: Option<ImageQuality>,
    pub style: Option<ImageStyle>,
    pub output_format: ImageOutputFormat,
    pub user: Option<String>,
    /// Random seed for reproducibility (DALL-E, Imagen; not supported by all providers).
    pub seed: Option<u64>,
}

impl ImageGenerationRequest {
    pub fn new(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            n: None,
            size: None,
            quality: None,
            style: None,
            output_format: ImageOutputFormat::Url,
            user: None,
            seed: None,
        }
    }

    pub fn with_n(mut self, n: u32) -> Self {
        self.n = Some(n);
        self
    }

    pub fn with_size(mut self, size: ImageSize) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_quality(mut self, quality: ImageQuality) -> Self {
        self.quality = Some(quality);
        self
    }

    pub fn with_style(mut self, style: ImageStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub fn with_output_format(mut self, format: ImageOutputFormat) -> Self {
        self.output_format = format;
        self
    }

    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }
}

/// A single generated image.
#[derive(Debug, Clone)]
pub struct GeneratedImage {
    /// URL to the image (output_format = Url).
    pub url: Option<String>,
    /// Base64-encoded image data (output_format = B64Json or Imagen).
    pub b64_json: Option<String>,
    /// DALL-E 3 may return an enhanced version of the original prompt.
    pub revised_prompt: Option<String>,
}

/// Response from an image generation request.
#[derive(Debug, Clone)]
pub struct ImageGenerationResponse {
    pub images: Vec<GeneratedImage>,
}

// ---------------------------------------------------------------------------
// Video generation types
// ---------------------------------------------------------------------------

/// Aspect ratio for generated videos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoAspectRatio {
    Landscape16x9,
    Portrait9x16,
    Square1x1,
    Custom(String),
}

impl VideoAspectRatio {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Landscape16x9 => "16:9",
            Self::Portrait9x16 => "9:16",
            Self::Square1x1 => "1:1",
            Self::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for VideoAspectRatio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resolution for generated videos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoResolution {
    P720,
    P1080,
    Custom(String),
}

impl VideoResolution {
    pub fn as_str(&self) -> &str {
        match self {
            Self::P720 => "720p",
            Self::P1080 => "1080p",
            Self::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for VideoResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Request to generate one or more videos from a text prompt.
#[derive(Debug, Clone)]
pub struct VideoGenerationRequest {
    pub model: String,
    pub prompt: String,
    /// Number of videos to generate (default 1).
    pub n: Option<u32>,
    /// Video duration in seconds.
    pub duration_secs: Option<u32>,
    pub aspect_ratio: Option<VideoAspectRatio>,
    pub resolution: Option<VideoResolution>,
    /// S3 URI for async output (required for Bedrock Nova Reel: `s3://bucket/prefix`).
    pub output_storage_uri: Option<String>,
}

impl VideoGenerationRequest {
    pub fn new(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            n: None,
            duration_secs: None,
            aspect_ratio: None,
            resolution: None,
            output_storage_uri: None,
        }
    }

    pub fn with_n(mut self, n: u32) -> Self {
        self.n = Some(n);
        self
    }

    pub fn with_duration_secs(mut self, secs: u32) -> Self {
        self.duration_secs = Some(secs);
        self
    }

    pub fn with_aspect_ratio(mut self, ar: VideoAspectRatio) -> Self {
        self.aspect_ratio = Some(ar);
        self
    }

    pub fn with_resolution(mut self, res: VideoResolution) -> Self {
        self.resolution = Some(res);
        self
    }

    /// Set the S3 output URI (required for Bedrock Nova Reel).
    pub fn with_output_storage_uri(mut self, uri: impl Into<String>) -> Self {
        self.output_storage_uri = Some(uri.into());
        self
    }
}

/// A single generated video.
#[derive(Debug, Clone)]
pub struct GeneratedVideo {
    /// Download URI (GCS URL for Veo, CDN URL for others).
    pub uri: Option<String>,
    /// Base64-encoded video data (if returned inline).
    pub b64_json: Option<String>,
    pub duration_secs: Option<f64>,
}

/// Response from a video generation request.
#[derive(Debug, Clone)]
pub struct VideoGenerationResponse {
    pub videos: Vec<GeneratedVideo>,
}

// ---------------------------------------------------------------------------
// TTS / Transcription types
// ---------------------------------------------------------------------------

/// Text-to-speech request.
#[derive(Debug, Clone)]
pub struct SpeechRequest {
    pub model: String,
    pub input: String,
    pub voice: String,
    /// Output audio format. None = provider default (mp3).
    pub response_format: Option<AudioFormat>,
    /// Playback speed multiplier (0.25–4.0).
    pub speed: Option<f64>,
}

impl SpeechRequest {
    pub fn new(
        model: impl Into<String>,
        input: impl Into<String>,
        voice: impl Into<String>,
    ) -> Self {
        Self {
            model: model.into(),
            input: input.into(),
            voice: voice.into(),
            response_format: None,
            speed: None,
        }
    }

    pub fn with_format(mut self, f: AudioFormat) -> Self {
        self.response_format = Some(f);
        self
    }

    pub fn with_speed(mut self, s: f64) -> Self {
        self.speed = Some(s);
        self
    }
}

/// Text-to-speech response.
#[derive(Debug, Clone)]
pub struct SpeechResponse {
    pub audio: Vec<u8>,
    /// Actual format returned (mp3 if None was passed in request).
    pub format: AudioFormat,
}

/// Speech-to-text transcription request.
#[derive(Debug, Clone)]
pub struct TranscriptionRequest {
    pub model: String,
    pub audio: Vec<u8>,
    pub format: AudioFormat,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub temperature: Option<f64>,
}

impl TranscriptionRequest {
    pub fn new(model: impl Into<String>, audio: Vec<u8>, format: AudioFormat) -> Self {
        Self {
            model: model.into(),
            audio,
            format,
            language: None,
            prompt: None,
            temperature: None,
        }
    }

    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = Some(lang.into());
        self
    }

    pub fn with_prompt(mut self, p: impl Into<String>) -> Self {
        self.prompt = Some(p.into());
        self
    }

    pub fn with_temperature(mut self, t: f64) -> Self {
        self.temperature = Some(t);
        self
    }
}

/// Speech-to-text transcription response.
#[derive(Debug, Clone)]
pub struct TranscriptionResponse {
    pub text: String,
    pub language: Option<String>,
    pub duration_secs: Option<f64>,
}

// ---------------------------------------------------------------------------
// FallbackStrategy / FallbackTrigger
// ---------------------------------------------------------------------------

use crate::error::ProviderError as PE;

/// When to trigger fallback to the next provider.
#[derive(Debug, Clone, Default)]
pub enum FallbackStrategy {
    /// Fall back on any error (default).
    #[default]
    AnyError,
    /// Fall back only when the error matches one of the listed triggers.
    OnTriggers(Vec<FallbackTrigger>),
}

/// Error condition that can trigger a fallback.
///
/// Use `FallbackStrategy::AnyError` to fall back on all errors.
/// Use `FallbackStrategy::OnTriggers(vec![...])` to fall back only on specific errors.
#[derive(Debug, Clone, PartialEq)]
pub enum FallbackTrigger {
    ContextWindowExceeded,
    ContentFilterViolation,
    Timeout,
    TooManyRequests,
    Auth,
}

impl FallbackTrigger {
    pub fn matches(&self, err: &PE) -> bool {
        match self {
            Self::ContextWindowExceeded => matches!(err, PE::ContextWindowExceeded(_)),
            Self::ContentFilterViolation => matches!(err, PE::ContentFilterViolation(_)),
            Self::Timeout => matches!(err, PE::Timeout { .. }),
            Self::TooManyRequests => matches!(err, PE::TooManyRequests { .. }),
            Self::Auth => err.is_auth_error(),
        }
    }
}

/// Returns true if the error should trigger a fallback given `strategy`.
pub fn should_fallback(err: &PE, strategy: &FallbackStrategy) -> bool {
    match strategy {
        FallbackStrategy::AnyError => true,
        FallbackStrategy::OnTriggers(triggers) => triggers.iter().any(|t| t.matches(err)),
    }
}

// ---------------------------------------------------------------------------
// Agent loop types
// ---------------------------------------------------------------------------

/// A single step in an agent loop execution.
#[derive(Debug, Clone)]
pub struct AgentStep {
    pub step_number: usize,
    pub response: Response,
    pub tool_uses: Vec<ToolUseBlock>,
    /// (tool_use_id, result_text) pairs returned by the tool handler.
    pub tool_results: Vec<(String, String)>,
}

/// Result of running the agent loop to completion.
#[derive(Debug, Clone)]
pub struct AgentResult {
    /// Final response (no tool use).
    pub response: Response,
    /// All intermediate steps (tool calls + results).
    pub steps: Vec<AgentStep>,
    /// Full conversation messages including all tool calls and results.
    pub messages: Vec<Message>,
}

// ---------------------------------------------------------------------------
// StreamRecording
// ---------------------------------------------------------------------------

/// Records all events from a provider stream for later inspection or replay.
pub struct StreamRecording {
    pub(crate) events: Arc<Mutex<Vec<StreamEvent>>>,
}

impl StreamRecording {
    pub(crate) fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(vec![])),
        }
    }

    pub fn len(&self) -> usize {
        self.events.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot all recorded events (cloned).
    pub fn snapshot(&self) -> Vec<StreamEvent> {
        self.events.lock().clone()
    }

    /// Replay all recorded events starting from `index` as a new ProviderStream.
    pub fn replay_from(&self, index: usize) -> crate::provider::ProviderStream {
        let events = self.events.lock().clone();
        Box::pin(futures::stream::iter(
            events.into_iter().skip(index).map(Ok::<_, ProviderError>),
        ))
    }
}

// ---------------------------------------------------------------------------
// PartialConfig — for DefaultSettingsMiddleware
// ---------------------------------------------------------------------------

/// Subset of ProviderConfig fields used to supply defaults.
/// Only `Some` fields are applied; `None` means "don't override".
#[derive(Debug, Clone, Default)]
pub struct PartialConfig {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub seed: Option<u64>,
    /// Applied only if `config.stop_sequences` is empty.
    pub stop_sequences: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Vector utilities
// ---------------------------------------------------------------------------

/// Cosine similarity in [−1, 1]. Returns 0.0 if either vector has zero magnitude.
///
/// # Panics
/// Panics if `a.len() != b.len()`.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "vectors must have equal length");
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        0.0
    } else {
        (dot / (mag_a * mag_b)).clamp(-1.0, 1.0)
    }
}

/// L2-normalize a vector in place. No-op if magnitude is zero.
pub fn normalize_embedding(v: &mut [f32]) {
    let mag: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag > 0.0 {
        v.iter_mut().for_each(|x| *x /= mag);
    }
}

/// Euclidean distance between two equal-length vectors.
///
/// # Panics
/// Panics if `a.len() != b.len()`.
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "vectors must have equal length");
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

// ---------------------------------------------------------------------------
// ModelCapability helper free functions
// ---------------------------------------------------------------------------

/// Returns true if the model supports vision/image input.
pub fn supports_vision(model: &str) -> bool {
    model_capabilities(model).contains(&ModelCapability::Vision)
}

/// Returns true if the model supports audio input (speech understanding).
pub fn supports_audio_input(model: &str) -> bool {
    model_capabilities(model).contains(&ModelCapability::AudioInput)
}

/// Returns true if the model can generate audio output (TTS).
pub fn supports_audio_output(model: &str) -> bool {
    model_capabilities(model).contains(&ModelCapability::AudioOutput)
}

/// Returns true if the model supports function / tool calling.
pub fn supports_function_calling(model: &str) -> bool {
    model_capabilities(model).contains(&ModelCapability::FunctionCalling)
}

/// Returns true if the model supports extended thinking / reasoning.
pub fn supports_extended_thinking(model: &str) -> bool {
    model_capabilities(model).contains(&ModelCapability::ExtendedThinking)
}
