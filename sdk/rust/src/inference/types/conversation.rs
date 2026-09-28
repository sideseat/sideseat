use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{
    ContentBlock, ContentBlockStart, ContentDelta, Message, ProviderConfig, Role, StopReason, Usage,
};
use crate::error::ProviderError;

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
                    10 + tr
                        .content
                        .iter()
                        .filter_map(|c| c.as_text())
                        .map(|t| t.len())
                        .sum::<usize>()
                        / 4
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
    MessageStart { role: Role },
    /// Opens a new content block at `index`. Must precede `ContentBlockDelta` for that index.
    ContentBlockStart {
        index: usize,
        block: ContentBlockStart,
    },
    /// Incremental content for the block at `index`.
    ContentBlockDelta { index: usize, delta: ContentDelta },
    /// Closes the content block at `index`. No further deltas for that index will follow.
    ContentBlockStop { index: usize },
    /// Signals the end of the response and the reason generation stopped.
    MessageStop { stop_reason: StopReason },
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
        (
            "grok-4",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        // grok-3-mini has reasoning (reasoning_effort parameter)
        (
            "grok-3-mini",
            &[
                FunctionCalling,
                Streaming,
                StructuredOutput,
                ExtendedThinking,
            ],
        ),
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
        (
            "pixtral",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        (
            "mistral-large",
            &[Vision, FunctionCalling, Streaming, StructuredOutput],
        ),
        (
            "mistral-small",
            &[FunctionCalling, Streaming, StructuredOutput],
        ),
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
