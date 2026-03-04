use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
            cache_control: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::text(text)],
            cache_control: None,
        }
    }

    pub fn with_content(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            cache_control: None,
        }
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
        }
    }

    /// Enable strict schema validation for this tool (OpenAI).
    pub fn with_strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Alias for `new()` — ergonomic shorthand for schema-first tool definitions.
    pub fn from_schema(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self::new(name, description, input_schema)
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
}

impl Default for ProviderConfig {
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
#[derive(Debug, Clone)]
pub enum ContentBlockStart {
    Text,
    ToolUse { id: String, name: String },
    Thinking,
}

/// An incremental delta within a content block.
#[derive(Debug, Clone)]
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
    pub fn build(self) -> Vec<Message> {
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
#[derive(Debug, Clone)]
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
}
