//! SideML types.
//!
//! Core types for normalized SideML messages. These types have no
//! application-specific dependencies and can be used as a standalone library.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

// ============================================================================
// STRONGLY TYPED ENUMS
// ============================================================================

/// Standard chat roles
///
/// One `strum` declaration for the canonical spelling, which is also the JSON one. The *aliases* are a
/// different thing and stay in `try_from_str`: they are a semantic folding of many providers' words onto
/// these four roles, not alternative spellings of a role, and which of them outrank a span's own name is
/// decided by the declarations `declared_alias_spellings` is checked against.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Default,
    strum::IntoStaticStr,
    strum::VariantArray,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ChatRole {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

impl ChatRole {
    /// The canonical spelling, which is what a reconstructed message carries.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// Try to parse a role string, returning None for unknown roles.
    ///
    /// Supports role names from multiple providers:
    /// - OpenAI: system, user, assistant, tool, function, developer
    /// - Anthropic: user, assistant
    /// - Google: user, model
    /// - LangChain/LangGraph: human, ai, tool
    /// - Code execution: ipython (Jupyter/code interpreters)
    pub fn try_from_str(s: &str) -> Option<Self> {
        Some(match s.to_lowercase().as_str() {
            // System roles
            "system" | "developer" => Self::System,
            // User roles (including context/data which represent user-provided conversation history)
            "user" | "human" | "data" | "context" => Self::User,
            // Assistant roles (model outputs, tool invocations)
            "assistant" | "ai" | "bot" | "model" | "choice" | "tool_call" => Self::Assistant,
            // Tool roles (function/tool results, code execution output)
            "tool" | "function" | "ipython" => Self::Tool,
            _ => return None,
        })
    }

    /// Every spelling `try_from_str` folds, so a declaration can be required to exist for each.
    ///
    /// The folding table decides which of four canonical roles a spelling means. It used to decide **authority**
    /// as well - whether a stated role outranks the name its reading was tagged with - so adding a spelling here
    /// silently granted it that. `compile_role_authority` requires an explicit declaration per spelling, and this
    /// is the list it checks against, kept beside the match so a new arm is one line from being declared too.
    pub const fn declared_alias_spellings() -> &'static [&'static str] {
        &[
            "system",
            "developer",
            "user",
            "human",
            "data",
            "context",
            "assistant",
            "ai",
            "bot",
            "model",
            "choice",
            "tool_call",
            "tool",
            "function",
            "ipython",
        ]
    }

    /// Check if role string represents tool definitions (not a conversation role).
    pub fn is_tools_definition_role(s: &str) -> bool {
        s.to_lowercase() == "tools"
    }

    /// Normalize role string, defaulting to User for unknown roles.
    pub fn from_str_normalized(s: &str) -> Self {
        Self::try_from_str(s).unwrap_or(Self::User)
    }

    /// Check if role string normalizes to Tool (without String allocation).
    pub fn is_tool_role(s: &str) -> bool {
        matches!(Self::try_from_str(s), Some(Self::Tool))
    }
}

impl std::fmt::Display for ChatRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Why a response ended, in this engine's own categories.
///
/// Which provider word means which category is declared in the assets' `finish_reasons` sections, not here.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::IntoStaticStr,
    strum::VariantArray,
)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum FinishReason {
    /// The model finished its answer.
    Stop,
    /// A token limit cut the answer off.
    Length,
    /// The model asked for a tool.
    ToolUse,
    /// A content or safety filter stopped the answer.
    ContentFilter,
    /// The generation failed.
    Error,
}

impl FinishReason {
    /// This engine's own spelling, which is also the JSON one. Which provider word means which category
    /// is declared in the assets, not here, so there is no alias table to keep.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// The category a producer's finish-reason spelling means, through the declared spellings.
    pub fn from_str_normalized(s: &str) -> Option<Self> {
        crate::rules::ruleset().finish_reasons.lookup(s)
    }
}

impl std::fmt::Display for FinishReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Tool choice setting for controlling tool calling behavior
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    /// Model decides whether to call tools
    Auto,
    /// Model must not call any tools
    None,
    /// Model must call at least one tool
    Required,
    /// Model must call a specific function
    Function { name: String },
}

/// JSON schema details for structured output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchemaDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

/// Response format for structured outputs
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Plain text response (default)
    Text,
    /// JSON object response
    JsonObject,
    /// JSON schema response with strict schema validation
    JsonSchema { json_schema: JsonSchemaDetails },
}

/// Cache control settings (Anthropic prompt caching)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheControl {
    /// Cache type, typically "ephemeral"
    #[serde(rename = "type")]
    pub cache_type: String,
}

/// Unified content block types for multimodal messages.
///
/// Uses custom deserialization to preserve unknown content block types.
/// Known types are deserialized normally; unknown types are captured in
/// `Unknown { raw }` to prevent data loss.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text content
    Text { text: String },

    /// Image content (from image_url, inline_data, etc.)
    Image {
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        /// Source type: "base64" or "url"
        source: String,
        data: String,
        /// Detail level for vision models (auto, low, high)
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },

    /// Audio content (input_audio, audio output)
    Audio {
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        /// Source type: "base64" or "url"
        source: String,
        data: String,
    },

    /// Document content (PDF, etc.)
    Document {
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Source type: "base64" or "url"
        source: String,
        data: String,
    },

    /// Video content
    Video {
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        /// Source type: "base64" or "url"
        source: String,
        data: String,
    },

    /// Generic file content
    File {
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Source type: "base64" or "url"
        source: String,
        data: String,
    },

    /// Tool use request (assistant calling a tool) - Anthropic style
    ToolUse {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        /// Input as structured object (not stringified)
        input: JsonValue,
    },

    /// Tool result (response from tool execution)
    ToolResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        /// Tool name, when the source reports it on the result.
        ///
        /// Gemini and Google ADK identify a result only by function name - they emit no call
        /// id at all - so this is the only thing that can tie such a result back to its call.
        /// Discarding it left the result permanently uncorrelated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        content: JsonValue,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },

    /// Tool definitions (available tools for the model)
    ToolDefinitions {
        /// Normalized tool definitions in OpenAI format
        tools: Vec<JsonValue>,
        /// Tool choice setting (auto, none, required, or specific function)
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_choice: Option<JsonValue>,
    },

    /// Context/conversation history (chat context, conversation history)
    Context {
        /// Context data (may be messages array or other structured data)
        data: JsonValue,
        /// Optional context type (e.g., "conversation_history", "chat_context")
        #[serde(skip_serializing_if = "Option::is_none")]
        context_type: Option<String>,
    },

    /// Safety/content filter refusal
    Refusal { message: String },

    /// Structured JSON output (output_json, json_object)
    Json { data: JsonValue },

    /// Thinking/reasoning content (Claude extended thinking, o1 reasoning)
    Thinking {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// Redacted thinking content (Claude thinking when not exposed to API)
    RedactedThinking { data: String },

    /// Unknown/passthrough content (preserves original structure)
    Unknown {
        /// The original JSON content, preserved for lossless round-tripping
        raw: JsonValue,
    },
}

/// Internal enum for deserializing known content block types.
/// Used by the custom Deserialize implementation for ContentBlock.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum KnownContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: Option<String>,
        source: String,
        data: String,
        detail: Option<String>,
    },
    Audio {
        media_type: Option<String>,
        source: String,
        data: String,
    },
    Document {
        media_type: Option<String>,
        name: Option<String>,
        source: String,
        data: String,
    },
    Video {
        media_type: Option<String>,
        source: String,
        data: String,
    },
    File {
        media_type: Option<String>,
        name: Option<String>,
        source: String,
        data: String,
    },
    ToolUse {
        id: Option<String>,
        name: String,
        input: JsonValue,
    },
    ToolResult {
        tool_use_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        content: JsonValue,
        #[serde(default)]
        is_error: bool,
    },
    ToolDefinitions {
        tools: Vec<JsonValue>,
        tool_choice: Option<JsonValue>,
    },
    Context {
        data: JsonValue,
        context_type: Option<String>,
    },
    Refusal {
        message: String,
    },
    Json {
        data: JsonValue,
    },
    Thinking {
        text: String,
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    Unknown {
        raw: JsonValue,
    },
}

impl From<KnownContentBlock> for ContentBlock {
    fn from(known: KnownContentBlock) -> Self {
        match known {
            KnownContentBlock::Text { text } => Self::Text { text },
            KnownContentBlock::Image {
                media_type,
                source,
                data,
                detail,
            } => Self::Image {
                media_type,
                source,
                data,
                detail,
            },
            KnownContentBlock::Audio {
                media_type,
                source,
                data,
            } => Self::Audio {
                media_type,
                source,
                data,
            },
            KnownContentBlock::Document {
                media_type,
                name,
                source,
                data,
            } => Self::Document {
                media_type,
                name,
                source,
                data,
            },
            KnownContentBlock::Video {
                media_type,
                source,
                data,
            } => Self::Video {
                media_type,
                source,
                data,
            },
            KnownContentBlock::File {
                media_type,
                name,
                source,
                data,
            } => Self::File {
                media_type,
                name,
                source,
                data,
            },
            KnownContentBlock::ToolUse { id, name, input } => Self::ToolUse { id, name, input },
            KnownContentBlock::ToolResult {
                tool_use_id,
                name,
                content,
                is_error,
            } => Self::ToolResult {
                tool_use_id,
                name,
                content,
                is_error,
            },
            KnownContentBlock::ToolDefinitions { tools, tool_choice } => {
                Self::ToolDefinitions { tools, tool_choice }
            }
            KnownContentBlock::Context { data, context_type } => {
                Self::Context { data, context_type }
            }
            KnownContentBlock::Refusal { message } => Self::Refusal { message },
            KnownContentBlock::Json { data } => Self::Json { data },
            KnownContentBlock::Thinking { text, signature } => Self::Thinking { text, signature },
            KnownContentBlock::RedactedThinking { data } => Self::RedactedThinking { data },
            KnownContentBlock::Unknown { raw } => Self::Unknown { raw },
        }
    }
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // First deserialize as raw JSON
        let value = JsonValue::deserialize(deserializer)?;

        // Try to deserialize as a known content block type
        match serde_json::from_value::<KnownContentBlock>(value.clone()) {
            Ok(known) => Ok(known.into()),
            // If deserialization fails, preserve the raw content in Unknown
            Err(_) => Ok(ContentBlock::Unknown { raw: value }),
        }
    }
}

impl ContentBlock {
    /// Get the type name of this content block.
    pub fn block_type(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Image { .. } => "image",
            Self::Audio { .. } => "audio",
            Self::Document { .. } => "document",
            Self::Video { .. } => "video",
            Self::File { .. } => "file",
            Self::ToolUse { .. } => "tool_use",
            Self::ToolResult { .. } => "tool_result",
            Self::ToolDefinitions { .. } => "tool_definitions",
            Self::Context { .. } => "context",
            Self::Refusal { .. } => "refusal",
            Self::Json { .. } => "json",
            Self::Thinking { .. } => "thinking",
            Self::RedactedThinking { .. } => "redacted_thinking",
            Self::Unknown { .. } => "unknown",
        }
    }

    /// Returns the semantic category of this content block.
    ///
    /// Content blocks are categorized into three types for deduplication:
    ///
    /// ```text
    /// ┌─────────────────────────────────────────────────────────────────────┐
    /// │                    CONTENT BLOCK CATEGORIES                         │
    /// ├─────────────────────────────────────────────────────────────────────┤
    /// │                                                                     │
    /// │  SEMANTIC (Identity)          What the message IS                   │
    /// │  ─────────────────────────────────────────────────────────────────  │
    /// │  Text, ToolUse, ToolResult    Core conversation content             │
    /// │  Image, Audio, Video, etc.    Media content                         │
    /// │  Refusal, Json                Explicit responses/data               │
    /// │  Unknown                      Preserved for safety                  │
    /// │                                                                     │
    /// │  ENRICHMENT (Value-add)       How it was reasoned about             │
    /// │  ─────────────────────────────────────────────────────────────────  │
    /// │  Thinking                     Visible reasoning process             │
    /// │  RedactedThinking             Hidden reasoning process              │
    /// │                                                                     │
    /// │  METADATA (Structural)        Context, not content                  │
    /// │  ─────────────────────────────────────────────────────────────────  │
    /// │  Context                      Conversation history                  │
    /// │  ToolDefinitions              Available tools                       │
    /// │                                                                     │
    /// └─────────────────────────────────────────────────────────────────────┘
    /// ```
    ///
    /// # Deduplication Rules
    ///
    /// When determining if two messages are duplicates:
    /// - **Semantic blocks** define message identity (different semantic = different message)
    /// - **Enrichment blocks** don't affect identity (msg with thinking = msg without thinking)
    /// - **Metadata blocks** don't affect identity (structural context)
    ///
    /// # Quality Scoring
    ///
    /// When choosing between duplicates:
    /// - Prefer messages with more **enrichment blocks** (more complete)
    /// - This ensures the version WITH thinking is kept over the stripped version
    pub fn category(&self) -> ContentCategory {
        match self {
            // Semantic: Core content that defines what the message IS
            Self::Text { .. } => ContentCategory::Semantic,
            Self::Image { .. } => ContentCategory::Semantic,
            Self::Audio { .. } => ContentCategory::Semantic,
            Self::Video { .. } => ContentCategory::Semantic,
            Self::Document { .. } => ContentCategory::Semantic,
            Self::File { .. } => ContentCategory::Semantic,
            Self::ToolUse { .. } => ContentCategory::Semantic,
            Self::ToolResult { .. } => ContentCategory::Semantic,
            Self::Refusal { .. } => ContentCategory::Semantic,
            Self::Json { .. } => ContentCategory::Semantic,
            // Unknown is treated as semantic to preserve unique content
            // (we can't know if it's identity-defining, so assume it is)
            Self::Unknown { .. } => ContentCategory::Semantic,

            // Enrichment: Adds value but doesn't change message identity
            Self::Thinking { .. } => ContentCategory::Enrichment,
            Self::RedactedThinking { .. } => ContentCategory::Enrichment,

            // Metadata: Structural context, not message content
            Self::Context { .. } => ContentCategory::Metadata,
            Self::ToolDefinitions { .. } => ContentCategory::Metadata,
        }
    }

    /// Returns true if this block defines message identity (semantic content).
    ///
    /// Semantic blocks determine whether two messages are considered duplicates.
    /// Messages with the same semantic blocks are duplicates regardless of
    /// enrichment or metadata blocks.
    ///
    /// # Examples
    ///
    /// ```text
    /// Message A: [Text("Hello"), Thinking("...")]
    /// Message B: [Text("Hello")]
    /// → Same semantic content (Text) → DUPLICATES
    ///
    /// Message C: [Text("Hello")]
    /// Message D: [Text("Hello"), ToolUse("search")]
    /// → Different semantic content → NOT duplicates
    /// ```
    #[inline]
    pub fn is_semantic(&self) -> bool {
        self.category() == ContentCategory::Semantic
    }

    /// Returns true if this block is enrichment content.
    ///
    /// Enrichment blocks add value to a message without changing its identity.
    /// When deduplicating, messages with more enrichment blocks are preferred.
    ///
    /// # Example
    ///
    /// Some providers emit responses twice:
    /// - Chat span: `[Thinking("..."), Text("answer")]` (complete)
    /// - Root span: `[Text("answer")]` (stripped)
    ///
    /// Both have the same identity (Text), but the version with Thinking
    /// is preferred because enrichment adds value.
    #[inline]
    pub fn is_enrichment(&self) -> bool {
        self.category() == ContentCategory::Enrichment
    }
}

/// Semantic category for content blocks.
///
/// Used for deduplication and quality scoring. See [`ContentBlock::category`]
/// for detailed documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentCategory {
    /// Content that defines message identity (Text, ToolUse, media, etc.)
    Semantic,
    /// Content that enriches without changing identity (Thinking, RedactedThinking)
    Enrichment,
    /// Structural context (Context, ToolDefinitions)
    Metadata,
}

/// Complete chat message with all normalized fields (SideML format)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatMessage {
    pub role: ChatRole,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Content as array of content blocks (includes tool_use for tool calls)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<ContentBlock>,

    /// Tool use ID this message is responding to (for tool role)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,

    /// Normalized finish reason
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,

    /// Choice index (for multiple completions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<i32>,

    /// Tool choice setting (controls tool calling behavior)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    /// Response format (structured output format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,

    /// Model name (for observability)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Cache control (Anthropic prompt caching)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,

    /// Stop sequences
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,

    /// Parallel tool calls setting (OpenAI)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
}

impl ChatMessage {
    #[must_use]
    pub fn new(role: ChatRole) -> Self {
        Self {
            role,
            ..Default::default()
        }
    }

    #[must_use]
    pub fn with_content(mut self, content: Vec<ContentBlock>) -> Self {
        self.content = content;
        self
    }

    #[must_use]
    pub fn with_text(self, text: &str) -> Self {
        self.with_content(vec![ContentBlock::Text {
            text: text.to_string(),
        }])
    }

    #[must_use]
    pub fn with_finish_reason(mut self, reason: FinishReason) -> Self {
        self.finish_reason = Some(reason);
        self
    }
}

#[cfg(test)]
mod spelling_tests {
    use strum::VariantArray;

    use super::{ChatRole, FinishReason};

    /// The canonical spellings a reconstructed message and its finish reason carry.
    ///
    /// These reach the goldens, the truth comparison and every API answer, so they are the one set of
    /// strings in this crate that a reader can see changing without a compile error. `VariantArray`
    /// makes a new variant without a spelling here fail.
    #[test]
    fn every_canonical_spelling_is_unchanged_and_matches_its_json_form() {
        let roles = [
            (ChatRole::System, "system"),
            (ChatRole::User, "user"),
            (ChatRole::Assistant, "assistant"),
            (ChatRole::Tool, "tool"),
        ];
        for (role, spelling) in roles {
            assert_eq!(role.as_str(), spelling);
            assert_eq!(role.to_string(), spelling);
            assert_eq!(
                serde_json::to_string(&role).unwrap(),
                format!("\"{spelling}\"")
            );
            assert_eq!(
                serde_json::from_str::<ChatRole>(&format!("\"{spelling}\"")).unwrap(),
                role
            );
            // The canonical spelling is also an accepted alias, which is what keeps a round trip through
            // a stored message idempotent.
            assert_eq!(ChatRole::try_from_str(spelling), Some(role));
        }
        assert_eq!(ChatRole::VARIANTS.len(), roles.len());

        let reasons = [
            (FinishReason::Stop, "stop"),
            (FinishReason::Length, "length"),
            (FinishReason::ToolUse, "tool_use"),
            (FinishReason::ContentFilter, "content_filter"),
            (FinishReason::Error, "error"),
        ];
        for (reason, spelling) in reasons {
            assert_eq!(reason.as_str(), spelling);
            assert_eq!(reason.to_string(), spelling);
            assert_eq!(
                serde_json::to_string(&reason).unwrap(),
                format!("\"{spelling}\"")
            );
            assert_eq!(
                serde_json::from_str::<FinishReason>(&format!("\"{spelling}\"")).unwrap(),
                reason
            );
        }
        assert_eq!(FinishReason::VARIANTS.len(), reasons.len());
    }

    /// Every alias `try_from_str` folds still folds, to the same role.
    ///
    /// The folding table is not a spelling table and did not move to `strum`: it maps many providers'
    /// words onto four roles. This asserts the mapping rather than just the declaration list that
    /// `declared_alias_spellings` already guards.
    #[test]
    fn every_declared_alias_still_folds_to_the_same_role() {
        let folding = [
            ("system", ChatRole::System),
            ("developer", ChatRole::System),
            ("user", ChatRole::User),
            ("human", ChatRole::User),
            ("data", ChatRole::User),
            ("context", ChatRole::User),
            ("assistant", ChatRole::Assistant),
            ("ai", ChatRole::Assistant),
            ("bot", ChatRole::Assistant),
            ("model", ChatRole::Assistant),
            ("choice", ChatRole::Assistant),
            ("tool_call", ChatRole::Assistant),
            ("tool", ChatRole::Tool),
            ("function", ChatRole::Tool),
            ("ipython", ChatRole::Tool),
        ];
        for (spelling, role) in folding {
            assert_eq!(ChatRole::try_from_str(spelling), Some(role), "{spelling}");
            assert_eq!(
                ChatRole::try_from_str(&spelling.to_uppercase()),
                Some(role),
                "folding was case-insensitive: {spelling}"
            );
        }
        assert_eq!(ChatRole::declared_alias_spellings().len(), folding.len());
        assert!(ChatRole::try_from_str("not-a-role").is_none());
    }
}
