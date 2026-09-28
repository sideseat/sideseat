use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{
    ContentBlock, Message, ModelCapability, Response, StreamEvent, ToolUseBlock, model_capabilities,
};
use crate::error::ProviderError;

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
        Self {
            input: vec![input.into()],
            model: None,
        }
    }

    pub fn new_batch(inputs: Vec<String>) -> Self {
        Self {
            input: inputs,
            model: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

/// Boolean category flags from a moderation result.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModerationCategories {
    #[serde(default)]
    pub harassment: bool,
    #[serde(default, rename = "harassment/threatening")]
    pub harassment_threatening: bool,
    #[serde(default)]
    pub hate: bool,
    #[serde(default, rename = "hate/threatening")]
    pub hate_threatening: bool,
    #[serde(default)]
    pub illicit: bool,
    #[serde(default, rename = "illicit/violent")]
    pub illicit_violent: bool,
    #[serde(default, rename = "self-harm")]
    pub self_harm: bool,
    #[serde(default, rename = "self-harm/instructions")]
    pub self_harm_instructions: bool,
    #[serde(default, rename = "self-harm/intent")]
    pub self_harm_intent: bool,
    #[serde(default)]
    pub sexual: bool,
    #[serde(default, rename = "sexual/minors")]
    pub sexual_minors: bool,
    #[serde(default)]
    pub violence: bool,
    #[serde(default, rename = "violence/graphic")]
    pub violence_graphic: bool,
}

/// Confidence scores (0–1) for each moderation category.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModerationCategoryScores {
    #[serde(default)]
    pub harassment: f64,
    #[serde(default, rename = "harassment/threatening")]
    pub harassment_threatening: f64,
    #[serde(default)]
    pub hate: f64,
    #[serde(default, rename = "hate/threatening")]
    pub hate_threatening: f64,
    #[serde(default)]
    pub illicit: f64,
    #[serde(default, rename = "illicit/violent")]
    pub illicit_violent: f64,
    #[serde(default, rename = "self-harm")]
    pub self_harm: f64,
    #[serde(default, rename = "self-harm/instructions")]
    pub self_harm_instructions: f64,
    #[serde(default, rename = "self-harm/intent")]
    pub self_harm_intent: f64,
    #[serde(default)]
    pub sexual: f64,
    #[serde(default, rename = "sexual/minors")]
    pub sexual_minors: f64,
    #[serde(default)]
    pub violence: f64,
    #[serde(default, rename = "violence/graphic")]
    pub violence_graphic: f64,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
