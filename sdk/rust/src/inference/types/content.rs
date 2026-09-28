use serde::{Deserialize, Serialize};

use super::CacheControl;

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioOutputConfig {
    /// Voice to use for audio output (e.g. `"alloy"`, `"nova"`, `"shimmer"`, `"echo"`,
    /// `"fable"`, `"onyx"`, `"ash"`, `"ballad"`, `"coral"`, `"sage"`, `"verse"`).
    pub voice: String,
    /// Audio encoding format. Defaults to `mp3` if `None`.
    pub format: Option<AudioFormat>,
}

impl AudioOutputConfig {
    pub fn new(voice: impl Into<String>) -> Self {
        Self {
            voice: voice.into(),
            format: None,
        }
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
    SourceLocation {
        cited_text: String,
        source_id: String,
        source_name: String,
        chunk_index: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        start_char: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        end_char: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        page: Option<u32>,
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
        if let Self::Image(i) = self {
            Some(i)
        } else {
            None
        }
    }
    /// Returns the inner [`AudioContent`] if this is an `Audio` block, otherwise `None`.
    pub fn as_audio(&self) -> Option<&AudioContent> {
        if let Self::Audio(a) = self {
            Some(a)
        } else {
            None
        }
    }
    /// Returns the inner [`VideoContent`] if this is a `Video` block, otherwise `None`.
    pub fn as_video(&self) -> Option<&VideoContent> {
        if let Self::Video(v) = self {
            Some(v)
        } else {
            None
        }
    }
    /// Returns the inner [`DocumentContent`] if this is a `Document` block, otherwise `None`.
    pub fn as_document(&self) -> Option<&DocumentContent> {
        if let Self::Document(d) = self {
            Some(d)
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
