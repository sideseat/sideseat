use serde::{Deserialize, Serialize};

use super::{
    ContainerInfo, ContentBlock, GroundingMetadata, ThinkingBlock, TokenLogprob, ToolUseBlock,
};

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
