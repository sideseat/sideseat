use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures::{Stream, StreamExt, pin_mut};

use crate::error::ProviderError;
use crate::types::{
    AgentResult, AgentStep, AudioContent, AudioFormat, ContentBlock, ContentBlockStart,
    ContentDelta, EmbeddingRequest, EmbeddingResponse, FallbackStrategy, ImageContent,
    ImageEditRequest, ImageGenerationRequest, ImageGenerationResponse, MediaSource, Message,
    ModelInfo, ModerationRequest, ModerationResponse, ProviderConfig, Response, Role, SpeechRequest,
    SpeechResponse, StopReason, StreamEvent, StreamRecording, TextBlock, ThinkingBlock, TokenCount,
    ToolUseBlock, TranscriptionRequest, TranscriptionResponse, Usage, VideoGenerationRequest,
    VideoGenerationResponse, should_fallback,
};

/// Boxed stream of provider events.
pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>;

// ---------------------------------------------------------------------------
// Base Provider trait
// ---------------------------------------------------------------------------

/// Base trait shared by all provider capability traits.
///
/// Provides identity (`provider_name`) and model enumeration (`list_models`).
/// All capability-specific traits (`ChatProvider`, `EmbeddingProvider`, etc.) extend this.
#[async_trait]
pub trait Provider: Send + Sync {
    /// GenAI OTel `gen_ai.system` attribute value for this provider. Default: `"unknown"`.
    fn provider_name(&self) -> &'static str {
        "unknown"
    }

    /// List available models for this provider.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Err(ProviderError::Unsupported(
            "list_models not supported by this provider".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// ChatProvider trait
// ---------------------------------------------------------------------------

/// Conversational text generation capability.
///
/// Implement `stream()` — defaults for `complete()` and `count_tokens()` are provided.
#[async_trait]
pub trait ChatProvider: Provider {
    /// Start a streaming conversation. All chat providers must implement this.
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream;

    /// Run a non-streaming request. Default implementation collects the stream.
    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        let stream = self.stream(messages, config);
        collect_stream(stream).await
    }

    /// Count tokens for a request without generating a response.
    async fn count_tokens(
        &self,
        _messages: Vec<Message>,
        _config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        Err(ProviderError::Unsupported(
            "count_tokens not supported by this provider".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// EmbeddingProvider trait
// ---------------------------------------------------------------------------

/// Text embedding capability. Model is specified inside `EmbeddingRequest`.
#[async_trait]
pub trait EmbeddingProvider: Provider {
    async fn embed(
        &self,
        request: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ProviderError>;
}

// ---------------------------------------------------------------------------
// ImageProvider trait
// ---------------------------------------------------------------------------

/// Image generation and editing capability.
#[async_trait]
pub trait ImageProvider: Provider {
    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError>;

    async fn edit_image(
        &self,
        _request: ImageEditRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        Err(ProviderError::Unsupported(
            "Image editing not supported by this provider".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// VideoProvider trait
// ---------------------------------------------------------------------------

/// Video generation capability.
#[async_trait]
pub trait VideoProvider: Provider {
    async fn generate_video(
        &self,
        request: VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError>;
}

// ---------------------------------------------------------------------------
// AudioProvider trait
// ---------------------------------------------------------------------------

/// Audio generation, transcription, and translation capability.
#[async_trait]
pub trait AudioProvider: Provider {
    async fn generate_speech(
        &self,
        _request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        Err(ProviderError::Unsupported(
            "TTS not supported by this provider".into(),
        ))
    }

    async fn transcribe(
        &self,
        _request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        Err(ProviderError::Unsupported(
            "Transcription not supported by this provider".into(),
        ))
    }

    async fn translate(
        &self,
        _request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        Err(ProviderError::Unsupported(
            "Audio translation not supported by this provider".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// ModerationProvider trait
// ---------------------------------------------------------------------------

/// Content moderation capability.
#[async_trait]
pub trait ModerationProvider: Provider {
    async fn moderate(
        &self,
        request: ModerationRequest,
    ) -> Result<ModerationResponse, ProviderError>;
}

// ---------------------------------------------------------------------------
// StatefulProvider trait
// ---------------------------------------------------------------------------

/// Stateful response management (retrieve/cancel in-progress or stored responses).
#[async_trait]
pub trait StatefulProvider: Provider {
    async fn retrieve_response(&self, id: &str) -> Result<serde_json::Value, ProviderError>;
    async fn cancel_response(&self, id: &str) -> Result<serde_json::Value, ProviderError>;
}

// ---------------------------------------------------------------------------
// collect_stream
// ---------------------------------------------------------------------------

/// Collect a `ProviderStream` into a `Response` by assembling content blocks.
pub async fn collect_stream(stream: ProviderStream) -> Result<Response, ProviderError> {
    pin_mut!(stream);

    let mut usage = Usage::default();
    let mut stop_reason = StopReason::EndTurn;
    let mut model: Option<String> = None;
    let mut response_id: Option<String> = None;

    // Accumulate per-block state
    let mut text_blocks: HashMap<usize, String> = HashMap::new();
    let mut tool_blocks: HashMap<usize, (String, String, String)> = HashMap::new(); // (id, name, partial_json)
    let mut thinking_blocks: HashMap<usize, (String, Option<String>)> = HashMap::new(); // (thinking, signature)
    let mut image_blocks: HashMap<usize, (String, String)> = HashMap::new(); // (media_type, b64_data)
    let mut audio_blocks: HashMap<usize, String> = HashMap::new(); // accumulated base64 chunks

    // Ordered block indices so we preserve output order
    let mut block_order: Vec<usize> = Vec::new();

    while let Some(result) = stream.next().await {
        match result? {
            StreamEvent::ContentBlockStart { index, block } => {
                block_order.push(index);
                match block {
                    ContentBlockStart::Text => {
                        text_blocks.insert(index, String::new());
                    }
                    ContentBlockStart::ToolUse { id, name } => {
                        tool_blocks.insert(index, (id, name, String::new()));
                    }
                    ContentBlockStart::Thinking => {
                        thinking_blocks.insert(index, (String::new(), None));
                    }
                    ContentBlockStart::Audio => {
                        audio_blocks.insert(index, String::new());
                    }
                }
            }
            StreamEvent::ContentBlockDelta { index, delta } => match delta {
                ContentDelta::Text { text } => {
                    // Some providers (e.g. Bedrock converse-stream) omit ContentBlockStart
                    // for text blocks; auto-initialize on first delta.
                    let entry = text_blocks.entry(index).or_insert_with(|| {
                        if !block_order.contains(&index) {
                            block_order.push(index);
                        }
                        String::new()
                    });
                    entry.push_str(&text);
                }
                ContentDelta::ToolInput { partial_json } => {
                    if let Some((_, _, buf)) = tool_blocks.get_mut(&index) {
                        buf.push_str(&partial_json);
                    }
                }
                ContentDelta::Thinking { thinking } => {
                    if let Some((t, _)) = thinking_blocks.get_mut(&index) {
                        t.push_str(&thinking);
                    }
                }
                ContentDelta::Signature { signature } => {
                    if let Some((_, sig)) = thinking_blocks.get_mut(&index) {
                        *sig = Some(signature);
                    }
                }
                ContentDelta::AudioData { b64_data } => {
                    if let Some(buf) = audio_blocks.get_mut(&index) {
                        buf.push_str(&b64_data);
                    }
                }
            },
            StreamEvent::ContentBlockStop { .. } => {
                // Nothing to do — we finalize on MessageStop below
            }
            StreamEvent::MessageStop {
                stop_reason: sr, ..
            } => {
                stop_reason = sr;
            }
            StreamEvent::Metadata {
                usage: u,
                model: m,
                id,
            } => {
                usage += u;
                if m.is_some() {
                    model = m;
                }
                if id.is_some() {
                    response_id = id;
                }
            }
            StreamEvent::MessageStart { .. } => {}
            StreamEvent::InlineData {
                index,
                media_type,
                b64_data,
            } => {
                block_order.push(index);
                image_blocks.insert(index, (media_type, b64_data));
            }
        }
    }

    // Assemble content blocks in original order (dedup by index)
    let mut seen = std::collections::HashSet::new();
    let mut content: Vec<ContentBlock> = Vec::new();
    for index in block_order {
        if !seen.insert(index) {
            continue;
        }
        if let Some(text) = text_blocks.remove(&index) {
            if !text.is_empty() {
                content.push(ContentBlock::Text(TextBlock::new(text)));
            }
        } else if let Some((id, name, json_buf)) = tool_blocks.remove(&index) {
            let input = if json_buf.is_empty() {
                serde_json::Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(&json_buf).map_err(|e| {
                    ProviderError::Stream(format!(
                        "Malformed tool input JSON for '{}': {}",
                        name, e
                    ))
                })?
            };
            content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
        } else if let Some((thinking, signature)) = thinking_blocks.remove(&index) {
            content.push(ContentBlock::Thinking(ThinkingBlock {
                thinking,
                signature,
            }));
        } else if let Some((media_type, b64_data)) = image_blocks.remove(&index) {
            content.push(ContentBlock::Image(ImageContent {
                source: MediaSource::base64(media_type, b64_data),
                format: None,
                detail: None,
            }));
        } else if let Some(b64_data) = audio_blocks.remove(&index)
            && !b64_data.is_empty()
        {
            content.push(ContentBlock::Audio(AudioContent {
                source: MediaSource::base64("audio/mpeg", b64_data),
                format: AudioFormat::Mp3,
            }));
        }
    }

    // Drain any remaining text/tool/thinking that had no explicit start event
    for text in text_blocks.into_values() {
        if !text.is_empty() {
            content.push(ContentBlock::Text(TextBlock::new(text)));
        }
    }
    for (id, name, json_buf) in tool_blocks.into_values() {
        let input = if json_buf.is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(&json_buf).map_err(|e| {
                ProviderError::Stream(format!("Malformed tool input JSON for '{}': {}", name, e))
            })?
        };
        content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
    }
    for (thinking, signature) in thinking_blocks.into_values() {
        content.push(ContentBlock::Thinking(ThinkingBlock {
            thinking,
            signature,
        }));
    }

    Ok(Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model,
        id: response_id,
        container: None,
        logprobs: None,
        grounding_metadata: None,
        warnings: vec![],
        request_body: None,
    })
}

/// Collect a stream into both a `Response` and all raw events.
pub async fn collect_stream_with_events(
    stream: ProviderStream,
) -> Result<(Response, Vec<StreamEvent>), ProviderError> {
    pin_mut!(stream);
    let mut all_events: Vec<StreamEvent> = Vec::new();
    while let Some(result) = stream.next().await {
        all_events.push(result?);
    }
    // The stream replay requires ownership; clone once so we can return `all_events` too.
    let replay: ProviderStream = Box::pin(futures::stream::iter(
        all_events
            .clone()
            .into_iter()
            .map(Ok::<StreamEvent, ProviderError>),
    ));
    let response = collect_stream(replay).await?;
    Ok((response, all_events))
}

// ---------------------------------------------------------------------------
// TextStream — yields only text delta strings
// ---------------------------------------------------------------------------

/// A stream adapter that yields only text delta strings from a provider stream.
pub struct TextStream(ProviderStream);

impl TextStream {
    pub fn new(stream: ProviderStream) -> Self {
        Self(stream)
    }
}

impl Stream for TextStream {
    type Item = Result<String, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match self.0.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(StreamEvent::ContentBlockDelta {
                    delta: crate::types::ContentDelta::Text { text },
                    ..
                }))) => return Poll::Ready(Some(Ok(text))),
                Poll::Ready(Some(Ok(_))) => {}
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ProviderExt — ergonomic single-message helpers
// ---------------------------------------------------------------------------

/// Extension trait that adds convenience methods to any `ChatProvider`.
#[async_trait]
pub trait ProviderExt: ChatProvider {
    /// Stream a single user message.
    fn ask_stream(
        &self,
        content: impl Into<String> + Send,
        config: ProviderConfig,
    ) -> ProviderStream {
        self.stream(vec![Message::user(content)], config)
    }

    /// Complete a single user message.
    async fn ask(
        &self,
        content: impl Into<String> + Send,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        self.complete(vec![Message::user(content)], config).await
    }

    /// Complete a single user message and return the first text block.
    async fn ask_text(
        &self,
        content: impl Into<String> + Send,
        config: ProviderConfig,
    ) -> Result<String, ProviderError> {
        let response = self.ask(content, config).await?;
        response
            .first_text()
            .map(|s| s.to_string())
            .ok_or_else(|| ProviderError::Stream("No text content in response".into()))
    }
}

impl<T: ChatProvider + ?Sized> ProviderExt for T {}

// ---------------------------------------------------------------------------
// RetryConfig
// ---------------------------------------------------------------------------

/// Configuration for exponential backoff with jitter.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (not counting the initial attempt).
    pub max_retries: u32,
    /// Initial delay in milliseconds before the first retry.
    pub base_delay_ms: u64,
    /// Multiplier applied to the delay on each successive attempt.
    pub backoff_multiplier: f64,
    /// Fraction of jitter to apply (e.g. 0.25 = ±25%).
    pub jitter_factor: f64,
    /// Maximum delay cap in milliseconds.
    pub max_delay_ms: u64,
}

impl RetryConfig {
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            base_delay_ms: 1000,
            backoff_multiplier: 2.0,
            jitter_factor: 0.25,
            max_delay_ms: 30_000,
        }
    }

    pub fn with_base_delay_ms(mut self, ms: u64) -> Self {
        self.base_delay_ms = ms;
        self
    }

    pub fn with_jitter_factor(mut self, f: f64) -> Self {
        self.jitter_factor = f;
        self
    }

    pub fn with_max_delay_ms(mut self, ms: u64) -> Self {
        self.max_delay_ms = ms;
        self
    }

    pub(crate) fn delay_for_attempt(&self, attempt: u32) -> u64 {
        let base = self.base_delay_ms as f64;
        let exp = self
            .backoff_multiplier
            .powi(attempt.saturating_sub(1) as i32);
        let delay = (base * exp).min(self.max_delay_ms as f64);
        let jitter = (rand::random::<f64>() * 2.0 - 1.0) * self.jitter_factor;
        ((delay * (1.0 + jitter)) as u64).min(self.max_delay_ms)
    }
}

// ---------------------------------------------------------------------------
// retry_op helper
// ---------------------------------------------------------------------------

async fn retry_op<T, F, Fut>(config: &RetryConfig, f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut last_err: Option<ProviderError> = None;
    for attempt in 0..=config.max_retries {
        if attempt > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(config.delay_for_attempt(attempt))).await;
        }
        match f().await {
            Ok(r) => return Ok(r),
            Err(e) if e.is_retryable() => last_err = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last_err.expect("retry loop ran at least once"))
}

// ---------------------------------------------------------------------------
// RetryProvider
// ---------------------------------------------------------------------------

/// Wraps a provider with automatic retry on transient errors using exponential backoff with jitter.
///
/// Note: `stream()` is NOT retried — partial output cannot be transparently replayed.
pub struct RetryProvider<P> {
    inner: P,
    config: RetryConfig,
}

impl<P> RetryProvider<P> {
    pub fn new(inner: P, max_retries: u32) -> Self {
        Self {
            inner,
            config: RetryConfig::new(max_retries),
        }
    }

    pub fn from_config(inner: P, config: RetryConfig) -> Self {
        Self { inner, config }
    }

    pub fn with_base_delay_ms(mut self, ms: u64) -> Self {
        self.config.base_delay_ms = ms;
        self
    }
}

#[async_trait]
impl<P: Provider + Send + Sync> Provider for RetryProvider<P> {
    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.inner.list_models().await
    }
}

#[async_trait]
impl<P: ChatProvider + Send + Sync> ChatProvider for RetryProvider<P> {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        self.inner.stream(messages, config)
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        retry_op(&self.config, || {
            self.inner.complete(messages.clone(), config.clone())
        })
        .await
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        self.inner.count_tokens(messages, config).await
    }
}

#[async_trait]
impl<P: EmbeddingProvider + Send + Sync> EmbeddingProvider for RetryProvider<P> {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        retry_op(&self.config, || self.inner.embed(request.clone())).await
    }
}

#[async_trait]
impl<P: ImageProvider + Send + Sync> ImageProvider for RetryProvider<P> {
    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        retry_op(&self.config, || self.inner.generate_image(request.clone())).await
    }

    async fn edit_image(
        &self,
        request: ImageEditRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        retry_op(&self.config, || self.inner.edit_image(request.clone())).await
    }
}

#[async_trait]
impl<P: VideoProvider + Send + Sync> VideoProvider for RetryProvider<P> {
    async fn generate_video(
        &self,
        request: VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        retry_op(&self.config, || self.inner.generate_video(request.clone())).await
    }
}

#[async_trait]
impl<P: AudioProvider + Send + Sync> AudioProvider for RetryProvider<P> {
    async fn generate_speech(
        &self,
        request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        retry_op(&self.config, || self.inner.generate_speech(request.clone())).await
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        retry_op(&self.config, || self.inner.transcribe(request.clone())).await
    }

    async fn translate(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        retry_op(&self.config, || self.inner.translate(request.clone())).await
    }
}

#[async_trait]
impl<P: ModerationProvider + Send + Sync> ModerationProvider for RetryProvider<P> {
    async fn moderate(
        &self,
        request: ModerationRequest,
    ) -> Result<ModerationResponse, ProviderError> {
        retry_op(&self.config, || self.inner.moderate(request.clone())).await
    }
}

#[async_trait]
impl<P: StatefulProvider + Send + Sync> StatefulProvider for RetryProvider<P> {
    async fn retrieve_response(&self, id: &str) -> Result<serde_json::Value, ProviderError> {
        self.inner.retrieve_response(id).await
    }

    async fn cancel_response(&self, id: &str) -> Result<serde_json::Value, ProviderError> {
        self.inner.cancel_response(id).await
    }
}

// ---------------------------------------------------------------------------
// FallbackProvider
// ---------------------------------------------------------------------------

/// Tries each chat provider in order, returning the first successful response.
///
/// Note: `stream()` only uses the first provider — partial output cannot be transparently
/// replayed on fallback.
pub struct FallbackProvider {
    providers: Vec<Box<dyn ChatProvider + Send + Sync>>,
    strategy: FallbackStrategy,
}

impl FallbackProvider {
    pub fn new(providers: Vec<Box<dyn ChatProvider + Send + Sync>>) -> Self {
        Self {
            providers,
            strategy: FallbackStrategy::AnyError,
        }
    }

    pub fn with_strategy(
        providers: Vec<Box<dyn ChatProvider + Send + Sync>>,
        strategy: FallbackStrategy,
    ) -> Self {
        Self {
            providers,
            strategy,
        }
    }

    /// Add a provider to the fallback chain.
    pub fn push(&mut self, provider: impl ChatProvider + 'static) {
        self.providers.push(Box::new(provider));
    }
}

#[async_trait]
impl Provider for FallbackProvider {
    fn provider_name(&self) -> &'static str {
        self.providers
            .first()
            .map(|p| p.provider_name())
            .unwrap_or("unknown")
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut all = Vec::new();
        for p in &self.providers {
            if let Ok(models) = p.list_models().await {
                all.extend(models);
            }
        }
        Ok(all)
    }
}

#[async_trait]
impl ChatProvider for FallbackProvider {
    /// Note: only uses first provider (streaming cannot fall back mid-stream).
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        if let Some(p) = self.providers.first() {
            p.stream(messages, config)
        } else {
            Box::pin(futures::stream::once(async {
                Err(ProviderError::Config("No providers configured".into()))
            }))
        }
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        let mut last_err: Option<ProviderError> = None;
        for provider in &self.providers {
            match provider.complete(messages.clone(), config.clone()).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    if should_fallback(&e, &self.strategy) {
                        last_err = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| ProviderError::Config("No providers configured".into())))
    }
}

// ---------------------------------------------------------------------------
// Agent loop
// ---------------------------------------------------------------------------

/// Hook callbacks for the agent loop.
#[async_trait]
pub trait AgentHooks: Send + Sync {
    /// Called before each step to optionally modify the config.
    async fn prepare_step(&self, _step: usize, _config: &mut ProviderConfig) {}
    /// Return true to block a tool call and replace its result with an approval message.
    async fn needs_approval(&self, _tool: &ToolUseBlock) -> bool {
        false
    }
    /// Called after each step completes (including tool results).
    async fn on_step_finish(&self, _step: &AgentStep) {}
}

/// No-op hooks (default).
pub struct DefaultHooks;

#[async_trait]
impl AgentHooks for DefaultHooks {}

/// Run an enhanced agent loop with hooks, step tracking, and max_steps support.
pub async fn run_agent_loop_with_hooks<P, F, Fut, H>(
    provider: &P,
    mut messages: Vec<Message>,
    mut config: ProviderConfig,
    tool_handler: F,
    hooks: &H,
    max_steps: Option<usize>,
) -> Result<AgentResult, ProviderError>
where
    P: ChatProvider,
    F: Fn(Vec<ToolUseBlock>) -> Fut,
    Fut: Future<Output = Vec<(String, String)>> + Send,
    H: AgentHooks,
{
    let mut steps: Vec<AgentStep> = Vec::new();
    let mut step_n: usize = 0;

    loop {
        if let Some(max) = max_steps
            && step_n >= max
        {
            return Err(ProviderError::Config(format!(
                "max_steps ({max}) exceeded without reaching EndTurn"
            )));
        }

        hooks.prepare_step(step_n, &mut config).await;

        // Apply active_tools filter
        let effective_config = if let Some(ref names) = config.active_tools.clone() {
            let mut c = config.clone();
            c.tools.retain(|t| names.contains(&t.name));
            c
        } else {
            config.clone()
        };

        let response = provider
            .complete(messages.clone(), effective_config)
            .await?;

        if response.stop_reason != StopReason::ToolUse || !response.has_tool_use() {
            return Ok(AgentResult {
                response,
                steps,
                messages,
            });
        }

        let tool_uses: Vec<ToolUseBlock> = response.tool_uses().into_iter().cloned().collect();

        // Apply approval hook
        let approved_tool_uses = tool_uses.clone();
        let mut precomputed_results: Vec<Option<(String, String)>> =
            vec![None; approved_tool_uses.len()];
        for (i, tu) in approved_tool_uses.iter().enumerate() {
            if hooks.needs_approval(tu).await {
                precomputed_results[i] = Some((
                    tu.id.clone(),
                    "Tool call requires human approval".to_string(),
                ));
            }
        }

        // Split tools needing approval from those that don't
        let tools_needing_call: Vec<ToolUseBlock> = approved_tool_uses
            .iter()
            .zip(precomputed_results.iter())
            .filter(|(_, r)| r.is_none())
            .map(|(t, _)| t.clone())
            .collect();

        let handler_results = if tools_needing_call.is_empty() {
            vec![]
        } else {
            tool_handler(tools_needing_call.clone()).await
        };

        // Merge results in original order
        let mut handler_iter = handler_results.into_iter();
        let tool_results: Vec<(String, String)> = approved_tool_uses
            .iter()
            .zip(precomputed_results.iter())
            .map(|(tu, precomputed)| {
                if let Some(r) = precomputed {
                    r.clone()
                } else {
                    handler_iter
                        .next()
                        .unwrap_or_else(|| (tu.id.clone(), String::new()))
                }
            })
            .collect();

        messages.push(Message::with_content(
            Role::Assistant,
            response.content.clone(),
        ));
        messages.push(Message::with_tool_results(tool_results.clone()));

        let step = AgentStep {
            step_number: step_n,
            response,
            tool_uses,
            tool_results,
        };
        hooks.on_step_finish(&step).await;
        steps.push(step);
        step_n += 1;
    }
}

/// Run an agentic tool-call loop until the model stops requesting tools.
///
/// Calls `complete`, appends the assistant turn, invokes `tool_handler` with
/// all tool-use blocks, appends the tool results, and repeats until
/// `stop_reason != ToolUse` or no tool-use blocks are present.
pub async fn run_agent_loop<P, F, Fut>(
    provider: &P,
    messages: Vec<Message>,
    config: ProviderConfig,
    tool_handler: F,
) -> Result<Response, ProviderError>
where
    P: ChatProvider,
    F: Fn(Vec<ToolUseBlock>) -> Fut,
    Fut: Future<Output = Vec<(String, String)>> + Send,
{
    Ok(run_agent_loop_with_hooks(
        provider,
        messages,
        config,
        tool_handler,
        &DefaultHooks,
        None,
    )
    .await?
    .response)
}

// ---------------------------------------------------------------------------
// Batch completion
// ---------------------------------------------------------------------------

/// Run multiple completion requests concurrently and collect all results.
pub async fn batch_complete<P: ChatProvider>(
    provider: &P,
    requests: Vec<(Vec<Message>, ProviderConfig)>,
) -> Vec<Result<Response, ProviderError>> {
    futures::future::join_all(
        requests
            .into_iter()
            .map(|(msgs, cfg)| provider.complete(msgs, cfg)),
    )
    .await
}

// ---------------------------------------------------------------------------
// Top-level convenience functions
// ---------------------------------------------------------------------------

/// Complete a conversation and return the first text block.
pub async fn generate_text<P: ChatProvider>(
    provider: &P,
    messages: Vec<Message>,
    config: ProviderConfig,
) -> Result<String, ProviderError> {
    let response = provider.complete(messages, config).await?;
    response
        .first_text()
        .map(str::to_string)
        .ok_or_else(|| ProviderError::Stream("No text content in response".into()))
}

/// Stream a conversation, yielding only text delta strings.
pub fn stream_text<P: ChatProvider>(
    provider: &P,
    messages: Vec<Message>,
    config: ProviderConfig,
) -> TextStream {
    TextStream::new(provider.stream(messages, config))
}

// ---------------------------------------------------------------------------
// Stream recording
// ---------------------------------------------------------------------------

/// Wrap a stream and simultaneously record all events.
///
/// Returns `(recorded_stream, recording)`. Consuming `recorded_stream` drives
/// both the original stream and the recording.
pub fn record_stream(stream: ProviderStream) -> (ProviderStream, StreamRecording) {
    let recording = StreamRecording::new();
    let shared = recording.events.clone();
    let recorded = Box::pin(async_stream::try_stream! {
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            let event = item?;
            shared.lock().push(event.clone());
            yield event;
        }
    });
    (recorded, recording)
}

// ---------------------------------------------------------------------------
// Batch image generation
// ---------------------------------------------------------------------------

/// Generate multiple images concurrently. Returns one Result per request (in order).
pub async fn batch_generate_images<P: ImageProvider>(
    provider: &P,
    requests: Vec<ImageGenerationRequest>,
) -> Vec<Result<ImageGenerationResponse, ProviderError>> {
    futures::future::join_all(requests.into_iter().map(|r| provider.generate_image(r))).await
}

// ---------------------------------------------------------------------------
// wrap_language_model
// ---------------------------------------------------------------------------

/// Wrap a provider in a MiddlewareStack for ergonomic `.with()` chaining.
pub fn wrap_language_model<P: ChatProvider + Send + Sync + 'static>(
    provider: P,
) -> crate::middleware::MiddlewareStack<P> {
    crate::middleware::MiddlewareStack::new(provider)
}
