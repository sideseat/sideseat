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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
    /// Any role string not recognized by the SDK (e.g. "developer", "ipython", "data").
    /// Round-trips through serialization unchanged.
    Other(String),
}

impl Role {
    pub fn as_str(&self) -> &str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for Role {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Role {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "system" => Self::System,
            "user" => Self::User,
            "assistant" => Self::Assistant,
            "tool" => Self::Tool,
            _ => Self::Other(s),
        })
    }
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
    /// Plain text — used as document text source (Bedrock `DocumentSource::Text`)
    Text(String),
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
    /// Raw 16-bit PCM audio — OpenAI audio output only.
    #[serde(rename = "pcm16")]
    Pcm16,
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

/// Image detail level — controls how much processing OpenAI applies to an image.
///
/// Sent as the `detail` field in `image_url` content parts.
/// `None` defaults to `Auto` (provider decides based on image size).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageDetail {
    /// Provider chooses based on image size.
    #[default]
    Auto,
    /// Low-fidelity mode: 85 tokens, image resized to 512×512.
    Low,
    /// High-fidelity mode: full resolution, higher token cost.
    High,
}

// ---------------------------------------------------------------------------
// Media content structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    pub source: MediaSource,
    pub format: Option<ImageFormat>,
    /// Image detail level forwarded to OpenAI-compatible providers. `None` = `Auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<ImageDetail>,
}

/// Request audio output from the model (OpenAI gpt-4o-audio-preview and o-series).
///
/// Setting this on [`ProviderConfig`] causes the Chat Completions request to include
/// `modalities: ["text", "audio"]` and the `audio` config object automatically.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioOutputConfig {
    /// Voice to use for audio output (e.g. `"alloy"`, `"nova"`, `"shimmer"`, `"echo"`,
    /// `"fable"`, `"onyx"`, `"ash"`, `"ballad"`, `"coral"`, `"sage"`, `"verse"`).
    pub voice: String,
    /// Audio encoding format. Defaults to `mp3` if `None`.
    pub format: Option<AudioFormat>,
}

impl AudioOutputConfig {
    pub fn new(voice: impl Into<String>) -> Self {
        Self { voice: voice.into(), format: None }
    }
    pub fn with_format(mut self, format: AudioFormat) -> Self {
        self.format = Some(format);
        self
    }
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

/// A tool-call block emitted by the model.
///
/// `input` is always a JSON object per the LLM tool-calling spec. While the field
/// type is `serde_json::Value` for compatibility with all providers, callers can
/// rely on `input.as_object()` returning `Some` in all well-formed responses.
/// Use [`ToolUseBlock::input_object`] for a checked accessor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseBlock {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

impl ToolUseBlock {
    /// Returns the input as an object map.
    ///
    /// Returns `None` if the provider sent a malformed non-object input.
    pub fn input_object(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.input.as_object()
    }
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
    pub text: String,
    /// Cryptographic signature (Anthropic / Bedrock). Must be passed back
    /// unmodified in multi-turn conversations.
    pub signature: Option<String>,
}

// ---------------------------------------------------------------------------
// Citation types (Anthropic citations API)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Citation {
    CharLocation {
        cited_text: String,
        document_index: u32,
        document_title: Option<String>,
        start_char_index: u32,
        end_char_index: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
    },
    PageLocation {
        cited_text: String,
        document_index: u32,
        document_title: Option<String>,
        start_page_number: u32,
        end_page_number: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
    },
    ContentBlockLocation {
        cited_text: String,
        document_index: u32,
        document_title: Option<String>,
        start_block_index: u32,
        end_block_index: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
    },
    WebSearchResultLocation {
        cited_text: String,
        url: String,
        title: Option<String>,
        encrypted_index: String,
    },
}

/// A text content block, optionally annotated with citations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBlock {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citations: Vec<Citation>,
}

impl TextBlock {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            citations: Vec::new(),
        }
    }
}

impl std::ops::Deref for TextBlock {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl PartialEq<str> for TextBlock {
    fn eq(&self, other: &str) -> bool {
        self.text == other
    }
}

impl PartialEq<String> for TextBlock {
    fn eq(&self, other: &String) -> bool {
        &self.text == other
    }
}

impl PartialEq for TextBlock {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for TextBlock {}

impl std::hash::Hash for TextBlock {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.text.hash(state);
    }
}

impl std::fmt::Display for TextBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<String> for TextBlock {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for TextBlock {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

// ---------------------------------------------------------------------------
// Container info (Anthropic code execution sandboxes)
// ---------------------------------------------------------------------------

/// Information about a code execution container returned by Anthropic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Content block — the union of all content types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text(TextBlock),
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
        ContentBlock::Text(TextBlock::new(t))
    }

    /// Construct a tool-use block representing a model's request to call a tool.
    ///
    /// - `id` — unique call identifier (echo it back in the matching [`Self::tool_result`])
    /// - `name` — name of the tool being called
    /// - `input` — parsed JSON arguments (should be an object matching the tool's schema)
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

    /// Construct an `Image` block from a URL.
    pub fn image_url(url: impl Into<String>) -> Self {
        Self::Image(ImageContent {
            source: MediaSource::Url(url.into()),
            format: None,
            detail: None,
        })
    }

    /// Construct a successful tool-result block.
    ///
    /// `tool_use_id` must match the `id` from the corresponding [`Self::tool_use`] block.
    /// For text-only results, prefer the higher-level [`Message::with_tool_results`].
    pub fn tool_result(tool_use_id: impl Into<String>, content: Vec<ContentBlock>) -> Self {
        Self::ToolResult(ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content,
            is_error: false,
        })
    }

    /// Construct a tool-result block signalling that the tool call failed.
    ///
    /// Sets `is_error: true`; the provider will include this in context as an error outcome.
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
            Some(&t.text)
        } else {
            None
        }
    }

    /// Returns the `TextBlock` (with citations) if this is a text block.
    ///
    /// Use `as_text()` for plain string access; use this when you need citations.
    pub fn as_text_block(&self) -> Option<&TextBlock> {
        if let Self::Text(t) = self { Some(t) } else { None }
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

    pub fn as_thinking(&self) -> Option<&ThinkingBlock> {
        if let Self::Thinking(t) = self {
            Some(t)
        } else {
            None
        }
    }

    /// Returns `true` if this block is an [`ImageContent`].
    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image(_))
    }
    /// Returns `true` if this block is an [`AudioContent`].
    pub fn is_audio(&self) -> bool {
        matches!(self, Self::Audio(_))
    }
    /// Returns `true` if this block is a [`VideoContent`].
    pub fn is_video(&self) -> bool {
        matches!(self, Self::Video(_))
    }
    /// Returns `true` if this block is a [`DocumentContent`].
    pub fn is_document(&self) -> bool {
        matches!(self, Self::Document(_))
    }

    /// Returns the inner [`ImageContent`] if this is an `Image` block, otherwise `None`.
    pub fn as_image(&self) -> Option<&ImageContent> {
        if let Self::Image(i) = self { Some(i) } else { None }
    }
    /// Returns the inner [`AudioContent`] if this is an `Audio` block, otherwise `None`.
    pub fn as_audio(&self) -> Option<&AudioContent> {
        if let Self::Audio(a) = self { Some(a) } else { None }
    }
    /// Returns the inner [`VideoContent`] if this is a `Video` block, otherwise `None`.
    pub fn as_video(&self) -> Option<&VideoContent> {
        if let Self::Video(v) = self { Some(v) } else { None }
    }
    /// Returns the inner [`DocumentContent`] if this is a `Document` block, otherwise `None`.
    pub fn as_document(&self) -> Option<&DocumentContent> {
        if let Self::Document(d) = self { Some(d) } else { None }
    }
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    /// Participant name for multi-agent / multi-user conversations (OpenAI-compatible providers).
    ///
    /// When multiple participants share the same role (e.g. two `user` turns from different
    /// people), set `name` to tell the model who said what. Forwarded verbatim to the API;
    /// ignored by providers that don't support it (Anthropic, Gemini, Bedrock, Cohere).
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

    /// Create a message with any role and an arbitrary list of content blocks.
    pub fn with_content(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            name: None,
            cache_control: None,
        }
    }

    /// Build a tool result message with rich content blocks for a single tool call.
    ///
    /// For multiple tool results in one message use [`Message::with_tool_result_blocks`].
    pub fn tool(tool_use_id: impl Into<String>, content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::Tool,
            content: vec![ContentBlock::tool_result(tool_use_id.into(), content)],
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

    /// Build a tool result message from `(tool_use_id, result_text)` pairs (text only).
    ///
    /// Use this for the common case where each tool returns a plain string.
    /// For a single tool result, prefer [`Message::tool`].
    /// For results with images, documents, or structured data, use [`Message::with_tool_result_blocks`].
    ///
    /// ```rust
    /// # use sideseat::Message;
    /// let msg = Message::with_tool_results(vec![
    ///     ("toolu_01".to_string(), "Paris".to_string()),
    ///     ("toolu_02".to_string(), "22°C".to_string()),
    /// ]);
    /// ```
    pub fn with_tool_results(results: Vec<(String, String)>) -> Self {
        let content = results
            .into_iter()
            .map(|(id, text)| ContentBlock::tool_result(id, vec![ContentBlock::text(text)]))
            .collect();
        Self {
            role: Role::Tool,
            content,
            name: None,
            cache_control: None,
        }
    }

    /// Build a tool result message from `(tool_use_id, content_blocks)` pairs (rich content).
    ///
    /// Use when any tool result contains non-text content (images, documents, etc.).
    /// For text-only results, prefer the simpler [`Message::with_tool_results`].
    ///
    /// ```rust
    /// # use sideseat::{Message, ContentBlock};
    /// let msg = Message::with_tool_result_blocks(vec![
    ///     ("toolu_01".to_string(), vec![
    ///         ContentBlock::text("Here is the chart:"),
    ///         ContentBlock::image_url("https://example.com/chart.png"),
    ///     ]),
    /// ]);
    /// ```
    pub fn with_tool_result_blocks(results: Vec<(String, Vec<ContentBlock>)>) -> Self {
        let content = results
            .into_iter()
            .map(|(id, blocks)| ContentBlock::tool_result(id, blocks))
            .collect();
        Self {
            role: Role::Tool,
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
    /// Create a tool definition.
    ///
    /// - `name` — identifier used by the model to call the tool (no spaces)
    /// - `description` — natural-language description of what the tool does (shown to the model)
    /// - `input_schema` — JSON Schema object describing the expected arguments
    ///
    /// Use [`Self::with_strict`] to enable strict schema validation (OpenAI),
    /// and [`Self::with_input_examples`] to provide few-shot examples.
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
    /// Allow the model to choose from a named subset of the defined tools.
    ///
    /// Maps to OpenAI `{"type": "allowed_tools", "mode": "auto", "tools": [...]}`.
    AllowedTools { tools: Vec<String> },
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
        /// Strict schema validation. Honored by OpenAI Chat, OpenAI Responses, Mistral, and xAI.
        /// Ignored by Anthropic, Gemini, Gemini Interactions, Cohere, and Bedrock.
        /// When `true`, automatically adds `additionalProperties: false` to the schema.
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
// JsonSchema builder
// ---------------------------------------------------------------------------

/// Fluent builder for constructing JSON Schema objects.
///
/// # Example
///
/// ```
/// use sideseat::types::{JsonSchema, Tool};
///
/// let schema = JsonSchema::object()
///     .required_field("city", JsonSchema::string().description("City name"))
///     .field("units", JsonSchema::string().enum_values(["celsius", "fahrenheit"]))
///     .build();
///
/// let _tool = Tool::new("get_weather", "Get current weather", schema);
/// ```
pub struct JsonSchema {
    schema: serde_json::Value,
}

impl JsonSchema {
    pub fn object() -> Self {
        Self {
            schema: serde_json::json!({"type": "object", "properties": {}, "required": []}),
        }
    }

    pub fn string() -> Self {
        Self { schema: serde_json::json!({"type": "string"}) }
    }

    pub fn number() -> Self {
        Self { schema: serde_json::json!({"type": "number"}) }
    }

    pub fn integer() -> Self {
        Self { schema: serde_json::json!({"type": "integer"}) }
    }

    pub fn boolean() -> Self {
        Self { schema: serde_json::json!({"type": "boolean"}) }
    }

    pub fn null() -> Self {
        Self { schema: serde_json::json!({"type": "null"}) }
    }

    pub fn array(items: Self) -> Self {
        Self {
            schema: serde_json::json!({"type": "array", "items": items.schema}),
        }
    }

    pub fn description(mut self, desc: &str) -> Self {
        self.schema["description"] = serde_json::Value::String(desc.to_string());
        self
    }

    /// Add an optional property to an object schema.
    pub fn field(mut self, name: &str, schema: Self) -> Self {
        if let Some(props) = self.schema.get_mut("properties").and_then(|v| v.as_object_mut()) {
            props.insert(name.to_string(), schema.schema);
        }
        self
    }

    /// Add a required property to an object schema.
    pub fn required_field(mut self, name: &str, schema: Self) -> Self {
        if let Some(props) = self.schema.get_mut("properties").and_then(|v| v.as_object_mut()) {
            props.insert(name.to_string(), schema.schema);
        }
        if let Some(req) = self.schema.get_mut("required").and_then(|v| v.as_array_mut()) {
            req.push(serde_json::Value::String(name.to_string()));
        }
        self
    }

    pub fn enum_values<S: AsRef<str>>(mut self, values: impl IntoIterator<Item = S>) -> Self {
        self.schema["enum"] = serde_json::Value::Array(
            values.into_iter().map(|v| serde_json::Value::String(v.as_ref().to_string())).collect(),
        );
        self
    }

    /// Allow null in addition to the current type.
    pub fn nullable(mut self) -> Self {
        if let Some(t) = self.schema.get("type").and_then(|v| v.as_str()).map(|s| s.to_string()) {
            self.schema["type"] = serde_json::json!([t, "null"]);
        }
        self
    }

    pub fn build(self) -> serde_json::Value {
        self.schema
    }
}

impl From<JsonSchema> for serde_json::Value {
    fn from(s: JsonSchema) -> Self {
        s.build()
    }
}

// ---------------------------------------------------------------------------
// Service tier
// ---------------------------------------------------------------------------

/// Processing tier sent to the provider.
///
/// | Variant | OpenAI | Anthropic |
/// |---|---|---|
/// | `Auto` | `"auto"` | `"auto"` |
/// | `Default` | `"default"` | — |
/// | `Flex` | `"flex"` | — |
/// | `StandardOnly` | — | `"standard_only"` |
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    /// Use priority capacity, falling back to standard (both OpenAI and Anthropic).
    Auto,
    /// Standard capacity only (OpenAI).
    Default,
    /// Lower cost, potentially slower (OpenAI).
    Flex,
    /// Standard capacity only, no priority fallback (Anthropic).
    StandardOnly,
}

impl ServiceTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Default => "default",
            Self::Flex => "flex",
            Self::StandardOnly => "standard_only",
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
///
/// Single-variant enum kept as enum for forward compatibility; future variants may add
/// `Persistent` or `Ttl(Duration)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CacheControl {
    /// Ephemeral cache (5-minute TTL, 1.25× token cost for cache writes)
    Ephemeral,
}

// ---------------------------------------------------------------------------
// Web search configuration
// ---------------------------------------------------------------------------

/// Approximate geographic context for web search results (OpenAI).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebSearchUserLocation {
    /// Always `"approximate"`.
    #[serde(rename = "type")]
    pub location_type: String,
    /// ISO 3166-1 alpha-2 country code (e.g. `"US"`, `"GB"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// City name (e.g. `"London"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// State or region name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// IANA timezone (e.g. `"America/Chicago"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl WebSearchUserLocation {
    pub fn new() -> Self {
        Self { location_type: "approximate".into(), ..Default::default() }
    }
    pub fn with_country(mut self, c: impl Into<String>) -> Self {
        self.country = Some(c.into());
        self
    }
    pub fn with_city(mut self, c: impl Into<String>) -> Self {
        self.city = Some(c.into());
        self
    }
    pub fn with_region(mut self, r: impl Into<String>) -> Self {
        self.region = Some(r.into());
        self
    }
    pub fn with_timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone = Some(tz.into());
        self
    }
}

/// Remote MCP server configuration for [`BuiltinTool::mcp`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolConfig {
    /// Always `"mcp"` — set automatically by [`BuiltinTool::mcp`].
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Human-readable label for the server (used in tool names and logs).
    pub server_label: String,
    /// MCP server URL (for remote servers). Mutually exclusive with `connector_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// OpenAI-maintained connector ID (e.g. `"connector_dropbox"`). Mutually exclusive with `server_url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connector_id: Option<String>,
    /// Human-readable description of the server's capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_description: Option<String>,
    /// When to require user approval: `"never"`, `"always"`, or an object specifying individual tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_approval: Option<serde_json::Value>,
    /// Restrict which MCP tools the model may call. `None` means all tools are allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// Bearer token or OAuth access token for server authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization: Option<String>,
}

impl McpToolConfig {
    /// Remote MCP server accessible via URL.
    pub fn new(server_label: impl Into<String>, server_url: impl Into<String>) -> Self {
        Self {
            tool_type: "mcp".into(),
            server_label: server_label.into(),
            server_url: Some(server_url.into()),
            connector_id: None,
            server_description: None,
            require_approval: None,
            allowed_tools: None,
            authorization: None,
        }
    }
    /// OpenAI-maintained connector (e.g. Dropbox, Gmail).
    pub fn connector(server_label: impl Into<String>, connector_id: impl Into<String>) -> Self {
        Self {
            tool_type: "mcp".into(),
            server_label: server_label.into(),
            server_url: None,
            connector_id: Some(connector_id.into()),
            server_description: None,
            require_approval: None,
            allowed_tools: None,
            authorization: None,
        }
    }
    pub fn with_description(mut self, d: impl Into<String>) -> Self {
        self.server_description = Some(d.into());
        self
    }
    /// Set `require_approval` to `"never"` or `"always"`.
    pub fn with_require_approval(mut self, v: impl Into<String>) -> Self {
        self.require_approval = Some(serde_json::json!(v.into()));
        self
    }
    pub fn with_allowed_tools(mut self, tools: Vec<impl Into<String>>) -> Self {
        self.allowed_tools = Some(tools.into_iter().map(Into::into).collect());
        self
    }
    pub fn with_authorization(mut self, token: impl Into<String>) -> Self {
        self.authorization = Some(token.into());
        self
    }
}

/// A built-in OpenAI tool for the Responses API (and where noted, Chat Completions).
///
/// Use the typed constructors ([`BuiltinTool::file_search`], [`BuiltinTool::mcp`], etc.)
/// or [`BuiltinTool::raw`] for custom tool configurations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinTool(pub(crate) serde_json::Value);

/// Server-side context compaction configuration for the Responses API.
///
/// When the accumulated context exceeds `compact_threshold` tokens, the server
/// automatically compacts the conversation before generating the next response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextManagementConfig {
    /// Token count that triggers automatic server-side compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compact_threshold: Option<u32>,
}

impl ContextManagementConfig {
    pub fn with_compact_threshold(compact_threshold: u32) -> Self {
        Self { compact_threshold: Some(compact_threshold) }
    }
}

impl BuiltinTool {
    /// Search over uploaded files. Uses OpenAI's vector stores.
    pub fn file_search() -> Self {
        Self(serde_json::json!({"type": "file_search"}))
    }

    /// File search restricted to specific vector store IDs.
    pub fn file_search_with_ids(ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let ids: Vec<String> = ids.into_iter().map(Into::into).collect();
        Self(serde_json::json!({"type": "file_search", "vector_store_ids": ids}))
    }

    /// Code interpreter running in an auto-provisioned OpenAI container.
    pub fn code_interpreter() -> Self {
        Self(serde_json::json!({"type": "code_interpreter", "container": {"type": "auto"}}))
    }

    /// Code interpreter with pre-uploaded file IDs available in the container.
    pub fn code_interpreter_with_files(file_ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let ids: Vec<String> = file_ids.into_iter().map(Into::into).collect();
        Self(serde_json::json!({
            "type": "code_interpreter",
            "container": {"type": "auto", "file_ids": ids}
        }))
    }

    /// GPT Image generation tool. The model generates images inline during the conversation.
    pub fn image_generation() -> Self {
        Self(serde_json::json!({"type": "image_generation"}))
    }

    /// Computer use (browser/desktop automation).
    ///
    /// `environment`: `"browser"`, `"mac"`, `"windows"`, or `"ubuntu"`.
    pub fn computer_use(
        display_width: u32,
        display_height: u32,
        environment: impl Into<String>,
    ) -> Self {
        Self(serde_json::json!({
            "type": "computer_use_preview",
            "display_width": display_width,
            "display_height": display_height,
            "environment": environment.into(),
        }))
    }

    /// Remote MCP server or OpenAI-maintained connector.
    pub fn mcp(config: McpToolConfig) -> Self {
        Self(serde_json::to_value(config).unwrap_or_else(|e| {
            tracing::debug!("BuiltinTool::mcp: failed to serialize McpToolConfig: {e}");
            serde_json::json!({"type": "mcp"})
        }))
    }

    /// Shell tool in an auto-provisioned OpenAI container.
    pub fn shell_auto() -> Self {
        Self(serde_json::json!({"type": "shell", "environment": {"type": "container_auto"}}))
    }

    /// Shell tool running in your own local runtime.
    pub fn shell_local() -> Self {
        Self(serde_json::json!({"type": "shell", "environment": {"type": "local"}}))
    }

    /// Local shell (`codex-mini-latest` only). Execution runs entirely in your runtime.
    pub fn local_shell() -> Self {
        Self(serde_json::json!({"type": "local_shell"}))
    }

    /// Apply-patch tool for structured file diffs (`gpt-5.1` only).
    pub fn apply_patch() -> Self {
        Self(serde_json::json!({"type": "apply_patch"}))
    }

    /// Escape hatch — provide a raw JSON tool object for custom or future tool types.
    pub fn raw(v: serde_json::Value) -> Self {
        Self(v)
    }

    pub fn as_value(&self) -> &serde_json::Value {
        &self.0
    }
}

/// Built-in web search tool configuration (Anthropic, OpenAI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchConfig {
    /// Maximum number of web searches the model may perform
    pub max_uses: Option<u32>,
    /// Domain whitelist — only these domains are searched (cannot combine with blocked_domains)
    pub allowed_domains: Option<Vec<String>>,
    /// Domain blacklist — these domains are excluded from results
    pub blocked_domains: Option<Vec<String>>,
    /// Approximate user location for geographically relevant results (OpenAI only).
    pub user_location: Option<WebSearchUserLocation>,
    /// Controls how much web context is fetched per search. One of `"low"`, `"medium"`, `"high"`.
    /// Higher values improve accuracy at greater latency and cost (OpenAI only).
    pub search_context_size: Option<String>,
}

impl WebSearchConfig {
    pub fn new() -> Self {
        Self {
            max_uses: None,
            allowed_domains: None,
            blocked_domains: None,
            user_location: None,
            search_context_size: None,
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

    /// Set approximate user location for geographically relevant results (OpenAI only).
    pub fn with_user_location(mut self, loc: WebSearchUserLocation) -> Self {
        self.user_location = Some(loc);
        self
    }

    /// Set search context size: `"low"`, `"medium"`, or `"high"` (OpenAI only).
    pub fn with_search_context_size(mut self, size: impl Into<String>) -> Self {
        self.search_context_size = Some(size.into());
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

/// The reason a provider stopped generating tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StopReason {
    /// The model finished naturally (most common outcome).
    #[default]
    EndTurn,
    /// Generation was cut short by the `max_tokens` limit.
    MaxTokens,
    /// The model emitted one of the requested `stop_sequences`.
    StopSequence(String),
    /// The model requested one or more tool calls.
    ToolUse,
    /// Output was blocked by the provider's content filter.
    ContentFilter,
    /// A provider-specific stop reason not covered by the variants above.
    Other(String),
}

/// A completed provider response.
///
/// Core payload is in [`content`](Self::content) — a `Vec` of [`ContentBlock`]s (text, tool calls,
/// thinking, audio, etc.). Use the helper methods [`first_text()`](Self::first_text),
/// [`tool_uses()`](Self::tool_uses), and [`thinking_content()`](Self::thinking_content)
/// for common access patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub content: Vec<ContentBlock>,
    pub usage: Usage,
    pub stop_reason: StopReason,
    /// Model ID returned by the API (may differ from requested model)
    pub model: Option<String>,
    /// Provider-assigned response/interaction ID (Gemini Interactions, OpenAI Responses, etc.)
    pub id: Option<String>,
    /// Container info for code execution sandbox reuse (Anthropic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<ContainerInfo>,
    /// Token-level log probabilities (OpenAI, when requested via logprobs=true)
    pub logprobs: Option<Vec<TokenLogprob>>,
    /// Grounding metadata from Gemini web search
    pub grounding_metadata: Option<GroundingMetadata>,
    /// Parameters that were silently dropped or truncated by the provider.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl Response {
    /// Concatenates all `Text` content blocks into a single string.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Text(t) = b {
                    Some(t.text.as_str())
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

    /// Returns the first tool use block with the given name, if present.
    pub fn find_tool_use(&self, name: &str) -> Option<&ToolUseBlock> {
        self.content.iter().find_map(|b| match b {
            ContentBlock::ToolUse(tu) if tu.name == name => Some(tu),
            _ => None,
        })
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

    /// Append a warning (e.g. a silently truncated parameter).
    ///
    /// Used internally by providers and middleware. Callers can inspect `response.warnings`
    /// to surface non-fatal issues to the user.
    pub fn add_warning(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// Estimate cost based on per-million-token prices.
    ///
    /// Shorthand for `response.usage.estimate_cost(input_per_1m, output_per_1m)`.
    pub fn estimate_cost(&self, input_per_1m: f64, output_per_1m: f64) -> CostEstimate {
        self.usage.estimate_cost(input_per_1m, output_per_1m)
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
    /// Model to use for embedding
    pub model: String,
    /// Texts to embed
    pub inputs: Vec<String>,
    /// Desired output dimension (provider-dependent, optional)
    pub dimensions: Option<u32>,
    /// Task type hint for optimization (Gemini, Cohere)
    pub task_type: Option<EmbeddingTaskType>,
    /// Embedding output types to return (Cohere: `"float"`, `"int8"`, `"uint8"`, `"binary"`, `"ubinary"`)
    pub embedding_types: Option<Vec<String>>,
    /// Truncation strategy when input exceeds model context (Cohere: `"NONE"`, `"START"`, `"END"`)
    pub truncate: Option<String>,
}

impl EmbeddingRequest {
    pub fn new(model: impl Into<String>, inputs: Vec<impl Into<String>>) -> Self {
        Self {
            model: model.into(),
            inputs: inputs.into_iter().map(|s| s.into()).collect(),
            dimensions: None,
            task_type: None,
            embedding_types: None,
            truncate: None,
        }
    }

    pub fn single(model: impl Into<String>, input: impl Into<String>) -> Self {
        Self::new(model, vec![input.into()])
    }

    pub fn with_dimensions(mut self, dimensions: u32) -> Self {
        self.dimensions = Some(dimensions);
        self
    }

    pub fn with_task_type(mut self, task_type: EmbeddingTaskType) -> Self {
        self.task_type = Some(task_type);
        self
    }

    pub fn with_embedding_types(mut self, types: Vec<impl Into<String>>) -> Self {
        self.embedding_types = Some(types.into_iter().map(|s| s.into()).collect());
        self
    }

    pub fn with_truncate(mut self, truncate: impl Into<String>) -> Self {
        self.truncate = Some(truncate.into());
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
        text: String,
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
            .map(|b| match b {
                ContentBlock::Text(t) => estimate_tokens(&t.text),
                ContentBlock::ToolUse(tu) => 15 + tu.input.to_string().len() / 4,
                ContentBlock::ToolResult(tr) => {
                    10 + tr.content.iter().filter_map(|c| c.as_text()).map(|t| t.len()).sum::<usize>() / 4
                }
                ContentBlock::Thinking(t) => 5 + t.text.len() / 4,
                _ => 5,
            })
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
///
/// Three ways to finalize a conversation:
/// - [`build()`](ConversationBuilder::build) — prepends system as `Message::system()`, returns `Vec<Message>`
/// - [`build_messages()`](ConversationBuilder::build_messages) — raw messages only (system prompt omitted)
/// - [`build_with_config()`](ConversationBuilder::build_with_config) — injects system into `ProviderConfig.system`
///
/// `build_with_config()` is the most portable option — it works with all providers and avoids
/// the need to choose between per-message and config-level system prompt handling.
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

    /// Returns only the message list, without the system prompt.
    ///
    /// Note: the system prompt set via `.system()` is NOT included in the returned list.
    ///
    /// Use when passing messages to a provider that accepts a separate `system` field
    /// (e.g. via `config.with_system()`), or when you want system-less raw messages.
    pub fn build_messages(self) -> Vec<Message> {
        self.messages
    }

    /// Returns all messages with the system prompt prepended as `Message::system(...)` when set.
    ///
    /// Unlike `build_messages()`, the system prompt is included as the first message.
    /// Unlike `build_with_config()`, the system prompt stays in the message list (not in config).
    ///
    /// Use when the provider reads system from the message list (Anthropic, Gemini).
    /// Avoids setting `config.system` separately.
    pub fn build(self) -> Vec<Message> {
        if let Some(system) = self.system {
            let mut msgs = Vec::with_capacity(self.messages.len() + 1);
            msgs.push(Message::system(system));
            msgs.extend(self.messages);
            msgs
        } else {
            self.messages
        }
    }

    /// Returns `(messages, config)` with the system prompt injected into config.
    ///
    /// Use when you want system in `ProviderConfig.system` without touching the message list.
    /// Compatible with all providers.
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
///
/// A well-formed stream emits them in this order:
/// `MessageStart` → (`ContentBlockStart` → `ContentBlockDelta`* → `ContentBlockStop`)* →
/// `Metadata` → `MessageStop`.
///
/// `InlineData` may appear in place of a `ContentBlock*` group for non-incremental media.
/// `collect_stream` tolerates missing `ContentBlockStart` events (auto-initializes the block).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Signals the start of a new assistant response.
    MessageStart {
        role: Role,
    },
    /// Opens a new content block at `index`. Must precede `ContentBlockDelta` for that index.
    ContentBlockStart {
        index: usize,
        block: ContentBlockStart,
    },
    /// Incremental content for the block at `index`.
    ContentBlockDelta {
        index: usize,
        delta: ContentDelta,
    },
    /// Closes the content block at `index`. No further deltas for that index will follow.
    ContentBlockStop {
        index: usize,
    },
    /// Signals the end of the response and the reason generation stopped.
    MessageStop {
        stop_reason: StopReason,
    },
    /// Token usage, model name, and response ID. May arrive before or after `MessageStop`;
    /// some providers (Anthropic) send it split across two events which are merged by `collect_stream`.
    Metadata {
        usage: Usage,
        model: Option<String>,
        /// Provider-assigned response ID (Gemini Interactions, OpenAI Responses, etc.)
        id: Option<String>,
    },
    /// Complete inline media block emitted as a single event (e.g. Gemini image output in chat).
    /// Not streamed incrementally — the full base64 payload arrives at once.
    InlineData {
        index: usize,
        media_type: String,
        b64_data: String,
    },
}

// ---------------------------------------------------------------------------
// StreamMeta — metadata captured after a stream completes
// ---------------------------------------------------------------------------

/// Metadata available after a provider stream completes.
#[derive(Debug, Clone, Default)]
pub struct StreamMeta {
    pub usage: Usage,
    pub model: Option<String>,
    pub id: Option<String>,
    pub stop_reason: StopReason,
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
                .ok_or_else(|| ProviderError::InvalidRequest("Unclosed '{{' in template".into()))?;
            let close = open + close_rel;
            let key = result[open + 2..close].to_string();
            let value = vars.get(key.as_str()).ok_or_else(|| {
                ProviderError::InvalidRequest(format!("Template variable '{}' not provided", key))
            })?;
            result = format!("{}{}{}", &result[..open], value, &result[close + 2..]);
            pos = open + value.len();
        }
        Ok(result)
    }

    /// Convenience: replaces `{{input}}` with the given value using simple string substitution.
    ///
    /// This is a plain `str::replace("{{input}}", ...)` — no error is returned for unclosed
    /// braces or missing variables. For strict multi-variable templates use [`render`](Self::render).
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
        // c4ai-aya variants
        ("c4ai-aya-vision", &[Vision, Streaming]),
        ("c4ai-aya", &[Streaming]),
        // ── xAI Grok ─────────────────────────────────────────────────────────
        // grok-4 supports vision
        ("grok-4", &[Vision, FunctionCalling, Streaming, StructuredOutput]),
        // grok-3-mini has reasoning (reasoning_effort parameter)
        ("grok-3-mini", &[FunctionCalling, Streaming, StructuredOutput, ExtendedThinking]),
        ("grok-3", &[FunctionCalling, Streaming, StructuredOutput]),
        // grok-2-vision models support image input
        ("grok-2-vision", &[Vision, FunctionCalling, Streaming]),
        ("grok-2", &[FunctionCalling, Streaming]),
        // Legacy grok-1 (open-weights)
        ("grok-1", &[Streaming]),
        // xAI embedding models
        ("grok-embed", &[Embeddings]),
        // ── Mistral ──────────────────────────────────────────────────────────
        // pixtral has vision; must be before mistral-large
        ("pixtral", &[Vision, FunctionCalling, Streaming, StructuredOutput]),
        ("mistral-large", &[Vision, FunctionCalling, Streaming, StructuredOutput]),
        ("mistral-small", &[FunctionCalling, Streaming, StructuredOutput]),
        ("mistral-medium", &[FunctionCalling, Streaming]),
        ("mistral-saba", &[FunctionCalling, Streaming]),
        ("codestral", &[FunctionCalling, Streaming]),
        ("open-mistral-nemo", &[FunctionCalling, Streaming]),
        ("open-mistral", &[FunctionCalling, Streaming]),
        ("open-mixtral", &[FunctionCalling, Streaming]),
        ("mixtral", &[FunctionCalling, Streaming]),
        ("mistral-embed", &[Embeddings]),
        ("mistral-moderation", &[]),
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

/// Request to edit an existing image (DALL-E 2, gpt-image-1 inpainting).
#[derive(Debug, Clone)]
pub struct ImageEditRequest {
    pub model: String,
    /// Image to edit as raw bytes.
    pub image: Vec<u8>,
    /// Format of the image file (used for the multipart filename).
    pub image_format: ImageFormat,
    /// Optional mask (transparent areas indicate what to edit). PNG only. DALL-E 2 only.
    pub mask: Option<Vec<u8>>,
    pub prompt: String,
    pub n: Option<u32>,
    pub size: Option<ImageSize>,
    pub output_format: ImageOutputFormat,
    pub user: Option<String>,
}

impl ImageEditRequest {
    pub fn new(
        model: impl Into<String>,
        image: Vec<u8>,
        image_format: ImageFormat,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            model: model.into(),
            image,
            image_format,
            mask: None,
            prompt: prompt.into(),
            n: None,
            size: None,
            output_format: ImageOutputFormat::Url,
            user: None,
        }
    }

    pub fn with_mask(mut self, mask: Vec<u8>) -> Self {
        self.mask = Some(mask);
        self
    }

    pub fn with_n(mut self, n: u32) -> Self {
        self.n = Some(n);
        self
    }

    pub fn with_size(mut self, size: ImageSize) -> Self {
        self.size = Some(size);
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
    /// Random seed for reproducibility (not supported by all providers).
    pub seed: Option<u64>,
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
            seed: None,
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

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
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
    /// Speaking style instructions (supported by `gpt-4o-mini-tts`).
    pub instructions: Option<String>,
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
            instructions: None,
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

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
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

/// Granularity level for transcription timestamps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimestampGranularity {
    /// Per-word timestamps in the response `words` array.
    Word,
    /// Per-segment timestamps in the response `segments` array.
    Segment,
}

impl TimestampGranularity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Segment => "segment",
        }
    }
}

/// Word-level timestamp from a transcription.
#[derive(Debug, Clone)]
pub struct TranscriptionWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
}

/// Segment-level detail from a transcription (Whisper verbose_json).
#[derive(Debug, Clone)]
pub struct TranscriptionSegment {
    pub id: u32,
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub temperature: f64,
    pub avg_logprob: f64,
    pub no_speech_prob: f64,
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
    /// Request word- and/or segment-level timestamps. Requires `verbose_json` response format.
    pub timestamp_granularities: Option<Vec<TimestampGranularity>>,
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
            timestamp_granularities: None,
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

    pub fn with_timestamp_granularities(mut self, g: Vec<TimestampGranularity>) -> Self {
        self.timestamp_granularities = Some(g);
        self
    }
}

/// Speech-to-text transcription response.
#[derive(Debug, Clone)]
pub struct TranscriptionResponse {
    pub text: String,
    pub language: Option<String>,
    pub duration_secs: Option<f64>,
    /// Per-word timestamps (populated when `TimestampGranularity::Word` is requested).
    pub words: Vec<TranscriptionWord>,
    /// Per-segment details (populated when `TimestampGranularity::Segment` is requested).
    pub segments: Vec<TranscriptionSegment>,
}

// ---------------------------------------------------------------------------
// Content moderation
// ---------------------------------------------------------------------------

/// Request to check whether text or images violate OpenAI usage policies.
#[derive(Debug, Clone)]
pub struct ModerationRequest {
    /// One or more text strings to classify.
    pub input: Vec<String>,
    /// Model to use. Default (`None`) → `"omni-moderation-latest"`.
    pub model: Option<String>,
}

impl ModerationRequest {
    pub fn new(input: impl Into<String>) -> Self {
        Self { input: vec![input.into()], model: None }
    }

    pub fn new_batch(inputs: Vec<String>) -> Self {
        Self { input: inputs, model: None }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

/// Boolean category flags from a moderation result.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModerationCategories {
    #[serde(default)] pub harassment: bool,
    #[serde(default, rename = "harassment/threatening")] pub harassment_threatening: bool,
    #[serde(default)] pub hate: bool,
    #[serde(default, rename = "hate/threatening")] pub hate_threatening: bool,
    #[serde(default)] pub illicit: bool,
    #[serde(default, rename = "illicit/violent")] pub illicit_violent: bool,
    #[serde(default, rename = "self-harm")] pub self_harm: bool,
    #[serde(default, rename = "self-harm/instructions")] pub self_harm_instructions: bool,
    #[serde(default, rename = "self-harm/intent")] pub self_harm_intent: bool,
    #[serde(default)] pub sexual: bool,
    #[serde(default, rename = "sexual/minors")] pub sexual_minors: bool,
    #[serde(default)] pub violence: bool,
    #[serde(default, rename = "violence/graphic")] pub violence_graphic: bool,
}

/// Confidence scores (0–1) for each moderation category.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModerationCategoryScores {
    #[serde(default)] pub harassment: f64,
    #[serde(default, rename = "harassment/threatening")] pub harassment_threatening: f64,
    #[serde(default)] pub hate: f64,
    #[serde(default, rename = "hate/threatening")] pub hate_threatening: f64,
    #[serde(default)] pub illicit: f64,
    #[serde(default, rename = "illicit/violent")] pub illicit_violent: f64,
    #[serde(default, rename = "self-harm")] pub self_harm: f64,
    #[serde(default, rename = "self-harm/instructions")] pub self_harm_instructions: f64,
    #[serde(default, rename = "self-harm/intent")] pub self_harm_intent: f64,
    #[serde(default)] pub sexual: f64,
    #[serde(default, rename = "sexual/minors")] pub sexual_minors: f64,
    #[serde(default)] pub violence: f64,
    #[serde(default, rename = "violence/graphic")] pub violence_graphic: f64,
}

/// Moderation result for a single input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerationResult {
    pub flagged: bool,
    pub categories: ModerationCategories,
    pub category_scores: ModerationCategoryScores,
}

/// Response from a moderation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerationResponse {
    pub id: String,
    pub model: String,
    pub results: Vec<ModerationResult>,
}

// ---------------------------------------------------------------------------
// TokenProvider — dynamic bearer token supply
// ---------------------------------------------------------------------------

use async_trait::async_trait;

/// Dynamic bearer token provider (e.g. for Vertex AI rotating credentials).
#[async_trait]
pub trait TokenProvider: Send + Sync {
    async fn get_token(&self) -> Result<String, crate::error::ProviderError>;
}

/// A `TokenProvider` backed by a static string.
pub struct StaticTokenProvider(String);

impl StaticTokenProvider {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }
}

#[async_trait]
impl TokenProvider for StaticTokenProvider {
    async fn get_token(&self) -> Result<String, crate::error::ProviderError> {
        Ok(self.0.clone())
    }
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
    Network,
}

impl FallbackTrigger {
    pub fn matches(&self, err: &PE) -> bool {
        match self {
            Self::ContextWindowExceeded => matches!(err, PE::ContextWindowExceeded(_)),
            Self::ContentFilterViolation => matches!(err, PE::ContentFilterViolation(_)),
            Self::Timeout => matches!(err, PE::Timeout { .. }),
            Self::TooManyRequests => matches!(err, PE::TooManyRequests { .. }),
            Self::Auth => err.is_auth_error(),
            Self::Network => matches!(err, PE::Network(_)),
        }
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
    /// (tool_use_id, content_blocks) pairs returned by the tool handler.
    pub tool_results: Vec<(String, Vec<ContentBlock>)>,
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
