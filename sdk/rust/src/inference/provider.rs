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
    VideoGenerationResponse,
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
    /// Short identifier used as the OTel `gen_ai.system` attribute and in debug logs.
    ///
    /// Appears in every span emitted by [`InstrumentedProvider`] — override this in
    /// custom providers so traces are labeled correctly. Defaults to `"unknown"`.
    ///
    /// [`InstrumentedProvider`]: crate::telemetry::InstrumentedProvider
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
    /// Start a streaming conversation. **All providers must implement this method.**
    ///
    /// Returns a [`ProviderStream`] emitting [`StreamEvent`]s. See the [`crate::providers`]
    /// module doc for the expected event ordering and `collect_stream` for how to consume it.
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream;

    /// Run a non-streaming request. Default implementation collects the stream.
    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        let audio_fmt = config.audio_output.as_ref().and_then(|a| a.format.clone());
        let stream = self.stream(messages, config);
        collect_stream_with_audio_format(stream, audio_fmt).await
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
    async fn retrieve_response(&self, _id: &str) -> Result<Response, ProviderError> {
        Err(ProviderError::Unsupported(
            "Response retrieval not supported by this provider".into(),
        ))
    }

    async fn cancel_response(&self, _id: &str) -> Result<Response, ProviderError> {
        Err(ProviderError::Unsupported(
            "Response cancellation not supported by this provider".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Streaming Utilities
// ---------------------------------------------------------------------------

/// Collect a `ProviderStream` into a `Response`, using the audio format from config when available.
///
/// See also [`collect_stream_with_events`] to capture raw events alongside the response.
pub async fn collect_stream_with_config(
    stream: ProviderStream,
    config: Option<&ProviderConfig>,
) -> Result<Response, ProviderError> {
    let audio_fmt = config
        .and_then(|c| c.audio_output.as_ref())
        .and_then(|a| a.format.clone());
    collect_stream_with_audio_format(stream, audio_fmt).await
}

/// Collect a `ProviderStream` into a `Response` by assembling content blocks.
///
/// See also [`collect_stream_with_events`] to capture raw events alongside the response.
pub async fn collect_stream(stream: ProviderStream) -> Result<Response, ProviderError> {
    collect_stream_with_audio_format(stream, None).await
}

async fn collect_stream_with_audio_format(
    stream: ProviderStream,
    audio_format: Option<AudioFormat>,
) -> Result<Response, ProviderError> {
    pin_mut!(stream);

    let mut usage = Usage::default();
    let mut stop_reason = StopReason::EndTurn;
    let mut model: Option<String> = None;
    let mut response_id: Option<String> = None;
    let mut received_metadata = false;

    // Accumulate per-block state
    let mut text_blocks: HashMap<usize, String> = HashMap::new();
    let mut tool_blocks: HashMap<usize, (String, String, String)> = HashMap::new(); // (id, name, partial_json)
    let mut thinking_blocks: HashMap<usize, (String, Option<String>)> = HashMap::new(); // (thinking, signature)
    let mut image_blocks: HashMap<usize, (String, String)> = HashMap::new(); // (media_type, b64_data)
    let mut audio_blocks: HashMap<usize, String> = HashMap::new(); // accumulated base64 chunks

    // Ordered block indices so we preserve output order
    let mut block_order: Vec<usize> = Vec::new();
    // O(1) membership test for orphan delta auto-init (avoids O(n²) with many blocks)
    let mut block_seen: std::collections::HashSet<usize> = std::collections::HashSet::new();

    while let Some(result) = stream.next().await {
        match result? {
            StreamEvent::ContentBlockStart { index, block } => {
                block_seen.insert(index);
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
                        if block_seen.insert(index) {
                            block_order.push(index);
                        }
                        String::new()
                    });
                    entry.push_str(&text);
                }
                ContentDelta::ToolInput { partial_json } => {
                    // Auto-initialize on orphan delta (id/name unknown, use empty strings).
                    let entry = tool_blocks.entry(index).or_insert_with(|| {
                        if block_seen.insert(index) {
                            block_order.push(index);
                        }
                        (String::new(), String::new(), String::new())
                    });
                    entry.2.push_str(&partial_json);
                }
                ContentDelta::Thinking { thinking } => {
                    let entry = thinking_blocks.entry(index).or_insert_with(|| {
                        if block_seen.insert(index) {
                            block_order.push(index);
                        }
                        (String::new(), None)
                    });
                    entry.0.push_str(&thinking);
                }
                ContentDelta::Signature { signature } => {
                    let entry = thinking_blocks.entry(index).or_insert_with(|| {
                        if block_seen.insert(index) {
                            block_order.push(index);
                        }
                        (String::new(), None)
                    });
                    entry.1 = Some(signature);
                }
                ContentDelta::AudioData { b64_data } => {
                    let entry = audio_blocks.entry(index).or_insert_with(|| {
                        if block_seen.insert(index) {
                            block_order.push(index);
                        }
                        String::new()
                    });
                    entry.push_str(&b64_data);
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
                received_metadata = true;
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
                if block_seen.insert(index) {
                    block_order.push(index);
                }
                image_blocks.insert(index, (media_type, b64_data));
            }
        }
    }

    if !received_metadata {
        tracing::debug!("collect_stream: no Metadata event received — usage will be zero");
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
            let fmt = audio_format.clone().unwrap_or(AudioFormat::Mp3);
            let mime = match &fmt {
                AudioFormat::Mp3 => "audio/mpeg",
                AudioFormat::Wav => "audio/wav",
                AudioFormat::Aac => "audio/aac",
                AudioFormat::Flac => "audio/flac",
                AudioFormat::Ogg => "audio/ogg",
                AudioFormat::Webm => "audio/webm",
                AudioFormat::M4a => "audio/mp4",
                AudioFormat::Opus => "audio/opus",
                AudioFormat::Aiff => "audio/aiff",
                AudioFormat::Pcm16 => "audio/pcm",
            };
            content.push(ContentBlock::Audio(AudioContent {
                source: MediaSource::base64(mime, b64_data),
                format: fmt,
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

/// Convert a `Response` into a `ProviderStream` emitting the corresponding events.
///
/// Emits: MessageStart → ContentBlockStart × N → ContentBlockDelta × N →
/// ContentBlockStop × N → Metadata → MessageStop.
pub fn response_to_stream(response: Response) -> ProviderStream {
    Box::pin(futures::stream::once(async move {
        Ok::<StreamEvent, ProviderError>(StreamEvent::MessageStart {
            role: Role::Assistant,
        })
    }).chain(futures::stream::iter({
        let mut events: Vec<Result<StreamEvent, ProviderError>> = Vec::new();
        for (i, block) in response.content.iter().enumerate() {
            match block {
                ContentBlock::Text(t) => {
                    events.push(Ok(StreamEvent::ContentBlockStart { index: i, block: crate::types::ContentBlockStart::Text }));
                    events.push(Ok(StreamEvent::ContentBlockDelta { index: i, delta: crate::types::ContentDelta::Text { text: t.text.clone() } }));
                    events.push(Ok(StreamEvent::ContentBlockStop { index: i }));
                }
                ContentBlock::ToolUse(tu) => {
                    events.push(Ok(StreamEvent::ContentBlockStart { index: i, block: crate::types::ContentBlockStart::ToolUse { id: tu.id.clone(), name: tu.name.clone() } }));
                    events.push(Ok(StreamEvent::ContentBlockDelta { index: i, delta: crate::types::ContentDelta::ToolInput { partial_json: tu.input.to_string() } }));
                    events.push(Ok(StreamEvent::ContentBlockStop { index: i }));
                }
                ContentBlock::Thinking(t) => {
                    events.push(Ok(StreamEvent::ContentBlockStart { index: i, block: crate::types::ContentBlockStart::Thinking }));
                    events.push(Ok(StreamEvent::ContentBlockDelta { index: i, delta: crate::types::ContentDelta::Thinking { thinking: t.thinking.clone() } }));
                    events.push(Ok(StreamEvent::ContentBlockStop { index: i }));
                }
                ContentBlock::Audio(audio) => {
                    if let MediaSource::Base64(b64) = &audio.source {
                        events.push(Ok(StreamEvent::ContentBlockStart { index: i, block: crate::types::ContentBlockStart::Audio }));
                        events.push(Ok(StreamEvent::ContentBlockDelta { index: i, delta: crate::types::ContentDelta::AudioData { b64_data: b64.data.clone() } }));
                        events.push(Ok(StreamEvent::ContentBlockStop { index: i }));
                    }
                    // URL-sourced audio has no inline payload to stream
                }
                ContentBlock::Image(img) => {
                    if let MediaSource::Base64(b64) = &img.source {
                        events.push(Ok(StreamEvent::InlineData { index: i, media_type: b64.media_type.clone(), b64_data: b64.data.clone() }));
                    }
                    // URL-sourced images have no inline payload to stream
                }
                // Document, Video, ToolResult are inputs, not outputs
                _ => {}
            }
        }
        events.push(Ok(StreamEvent::Metadata {
            usage: response.usage.clone(),
            model: response.model.clone(),
            id: response.id.clone(),
        }));
        events.push(Ok(StreamEvent::MessageStop { stop_reason: response.stop_reason.clone() }));
        events
    })))
}

/// Wraps a `ProviderStream` with a per-chunk timeout.
///
/// Returns `ProviderError::Timeout { ms: Some(timeout_ms) }` if no event arrives
/// within `timeout_ms` milliseconds between chunks.
///
/// Unlike `ProviderConfig::timeout_ms` (which governs the initial HTTP connection),
/// this timeout fires if the server stalls after the stream has started.
pub fn with_chunk_timeout(stream: ProviderStream, timeout_ms: u64) -> ProviderStream {
    let duration = tokio::time::Duration::from_millis(timeout_ms);
    Box::pin(async_stream::try_stream! {
        futures::pin_mut!(stream);
        loop {
            match tokio::time::timeout(duration, stream.next()).await {
                Ok(Some(item)) => yield item?,
                Ok(None) => break,
                Err(_elapsed) => {
                    Err(ProviderError::Timeout { ms: Some(timeout_ms) })?;
                    break;
                }
            }
        }
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
    /// Create a [`TextStream`] from a raw provider stream.
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
// TextStreamWithMeta — text stream that captures response metadata
// ---------------------------------------------------------------------------

/// A streaming text adapter that also captures response metadata.
///
/// Implements [`Stream<Item = Result<String, ProviderError>>`] yielding text deltas.
/// After the stream is exhausted, call [`meta`](Self::meta) to retrieve usage,
/// model, stop reason, and response ID.
///
/// # Example
///
/// ```no_run
/// use futures::StreamExt;
/// use sideseat::{ChatProvider, ProviderConfig, Message};
///
/// # async fn example(provider: impl ChatProvider) -> Result<(), sideseat::ProviderError> {
/// let stream = provider.stream(vec![Message::user("hi")], ProviderConfig::new("model"));
/// let mut meta_stream = sideseat::TextStreamWithMeta::new(stream);
/// while let Some(chunk) = meta_stream.next().await {
///     print!("{}", chunk?);
/// }
/// let meta = meta_stream.meta().unwrap();
/// println!("\n[{} tokens]", meta.usage.total_tokens);
/// # Ok(())
/// # }
/// ```
pub struct TextStreamWithMeta {
    inner: ProviderStream,
    meta: Option<crate::types::StreamMeta>,
    usage: crate::types::Usage,
    model: Option<String>,
    id: Option<String>,
    stop_reason: StopReason,
}

impl TextStreamWithMeta {
    pub fn new(stream: ProviderStream) -> Self {
        Self {
            inner: stream,
            meta: None,
            usage: crate::types::Usage::default(),
            model: None,
            id: None,
            stop_reason: StopReason::EndTurn,
        }
    }

    /// Returns response metadata once the stream has been fully consumed, `None` otherwise.
    pub fn meta(&self) -> Option<&crate::types::StreamMeta> {
        self.meta.as_ref()
    }
}

impl Stream for TextStreamWithMeta {
    type Item = Result<String, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(StreamEvent::ContentBlockDelta {
                    delta: crate::types::ContentDelta::Text { text },
                    ..
                }))) => return Poll::Ready(Some(Ok(text))),
                Poll::Ready(Some(Ok(StreamEvent::Metadata { usage, model, id }))) => {
                    self.usage += usage;
                    if model.is_some() {
                        self.model = model;
                    }
                    if id.is_some() {
                        self.id = id;
                    }
                }
                Poll::Ready(Some(Ok(StreamEvent::MessageStop { stop_reason }))) => {
                    self.stop_reason = stop_reason;
                }
                Poll::Ready(Some(Ok(_))) => {}
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => {
                    let meta = crate::types::StreamMeta {
                        usage: std::mem::take(&mut self.usage).with_totals(),
                        model: self.model.clone(),
                        id: self.id.clone(),
                        stop_reason: self.stop_reason.clone(),
                    };
                    self.meta = Some(meta);
                    return Poll::Ready(None);
                }
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
            .ok_or_else(|| ProviderError::EmptyResponse("No text content in response".into()))
    }

    /// Stream a single user message and capture metadata after completion.
    fn ask_stream_with_meta(
        &self,
        content: impl Into<String> + Send,
        config: ProviderConfig,
    ) -> TextStreamWithMeta {
        TextStreamWithMeta::new(self.stream(vec![Message::user(content)], config))
    }

    /// Wrap this provider's stream with a per-chunk timeout.
    ///
    /// Returns `ProviderError::Timeout { ms: Some(timeout_ms) }` if no event
    /// arrives within `timeout_ms` milliseconds between chunks.
    fn stream_with_chunk_timeout(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
        timeout_ms: u64,
    ) -> ProviderStream {
        with_chunk_timeout(self.stream(messages, config), timeout_ms)
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
    /// When true, `stream()` calls are retried by collecting into a full response
    /// and re-emitting as a single-pass stream. Default: false.
    pub retry_stream: bool,
}

impl RetryConfig {
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            base_delay_ms: 1000,
            backoff_multiplier: 2.0,
            jitter_factor: 0.25,
            max_delay_ms: 30_000,
            retry_stream: false,
        }
    }

    /// Enable stream retry (collects response then re-emits as events).
    pub fn with_stream_retry(mut self) -> Self {
        self.retry_stream = true;
        self
    }

    /// Set the initial delay before the first retry. Default: 1000 ms.
    pub fn with_base_delay_ms(mut self, ms: u64) -> Self {
        self.base_delay_ms = ms;
        self
    }

    /// Set the random jitter factor applied to each delay (0.0–1.0). Default: 0.25.
    ///
    /// A factor of `0.25` means each delay is ±25% of the calculated backoff value.
    /// Set to `0.0` to disable jitter.
    pub fn with_jitter_factor(mut self, f: f64) -> Self {
        self.jitter_factor = f;
        self
    }

    /// Cap the maximum delay between retries. Default: 30 000 ms (30 s).
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
// Retry
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
    Err(last_err.unwrap_or_else(|| ProviderError::Stream("All retry attempts exhausted".into())))
}

// ---------------------------------------------------------------------------
// RetryProvider
// ---------------------------------------------------------------------------

/// Wraps a provider with automatic retry on transient errors using exponential backoff with jitter.
///
/// `stream()` is not retried by default. Enable via `RetryConfig::with_stream_retry()` —
/// this buffers the full response before yielding events (no partial-output replay).
pub struct RetryProvider<P> {
    inner: std::sync::Arc<P>,
    config: RetryConfig,
}

impl<P> RetryProvider<P> {
    pub fn new(inner: P, max_retries: u32) -> Self {
        Self {
            inner: std::sync::Arc::new(inner),
            config: RetryConfig::new(max_retries),
        }
    }

    /// Create a [`RetryProvider`] with a fully-customized [`RetryConfig`].
    pub fn from_config(inner: P, config: RetryConfig) -> Self {
        Self { inner: std::sync::Arc::new(inner), config }
    }

    /// Set the initial delay before the first retry. Default: 1000 ms.
    pub fn with_base_delay_ms(mut self, ms: u64) -> Self {
        self.config.base_delay_ms = ms;
        self
    }

    pub fn inner(&self) -> &P {
        &self.inner
    }
}

#[async_trait]
impl<P: Provider + Send + Sync> Provider for RetryProvider<P> {
    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        retry_op(&self.config, || self.inner.list_models()).await
    }
}

#[async_trait]
impl<P: ChatProvider + Send + Sync + 'static> ChatProvider for RetryProvider<P> {
    /// When `retry_stream` is false (default): delegates directly to the inner provider.
    ///
    /// When `retry_stream` is true: calls `complete()` with retry logic, then converts
    /// the full response to a stream. Partial output cannot be transparently replayed,
    /// so `retry_stream` buffers the entire response before yielding any events.
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        if !self.config.retry_stream {
            return self.inner.stream(messages, config);
        }
        let inner = self.inner.clone();
        let rc = self.config.clone();
        Box::pin(async_stream::try_stream! {
            let resp = retry_op(&rc, || inner.complete(messages.clone(), config.clone())).await?;
            let s = response_to_stream(resp);
            futures::pin_mut!(s);
            while let Some(item) = s.next().await {
                yield item?;
            }
        })
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
    async fn retrieve_response(&self, id: &str) -> Result<Response, ProviderError> {
        self.inner.retrieve_response(id).await
    }

    async fn cancel_response(&self, id: &str) -> Result<Response, ProviderError> {
        self.inner.cancel_response(id).await
    }
}

// ---------------------------------------------------------------------------
// Fallback helpers
// ---------------------------------------------------------------------------

fn should_fallback(err: &ProviderError, strategy: &FallbackStrategy) -> bool {
    match strategy {
        // Unsupported is a programming error (wrong provider for the task), not a
        // transient failure — never fall back on it regardless of strategy.
        FallbackStrategy::AnyError => !matches!(err, ProviderError::Unsupported(_)),
        FallbackStrategy::OnTriggers(triggers) => triggers.iter().any(|t| t.matches(err)),
    }
}

// ---------------------------------------------------------------------------
// FallbackProvider
// ---------------------------------------------------------------------------

/// Observable health snapshot for a single provider in a [`FallbackProvider`] chain.
#[derive(Debug, Clone)]
pub struct ProviderHealthStatus {
    /// The `provider_name()` of the provider at this position.
    pub provider_name: String,
    /// Number of consecutive errors since the last success.
    pub consecutive_errors: u32,
    /// `false` when the provider is in a circuit-breaker cooldown.
    pub is_healthy: bool,
    /// Seconds remaining in the cooldown window, or `None` if the provider is healthy.
    pub cooldown_remaining_secs: Option<u64>,
}

/// Per-provider health state for circuit-breaker style skipping.
#[derive(Default)]
struct ProviderHealth {
    consecutive_errors: u32,
    unhealthy_until: Option<std::time::Instant>,
}

/// Tries each chat provider in order, returning the first successful response.
///
/// Note: `stream()` only uses the first provider — partial output cannot be transparently
/// replayed on fallback.
pub struct FallbackProvider {
    providers: Vec<Box<dyn ChatProvider + Send + Sync>>,
    strategy: FallbackStrategy,
    health: parking_lot::Mutex<Vec<ProviderHealth>>,
}

impl FallbackProvider {
    pub fn new(providers: Vec<Box<dyn ChatProvider + Send + Sync>>) -> Self {
        let n = providers.len();
        Self {
            providers,
            strategy: FallbackStrategy::AnyError,
            health: parking_lot::Mutex::new((0..n).map(|_| ProviderHealth::default()).collect()),
        }
    }

    pub fn with_strategy(
        providers: Vec<Box<dyn ChatProvider + Send + Sync>>,
        strategy: FallbackStrategy,
    ) -> Self {
        let n = providers.len();
        Self {
            providers,
            strategy,
            health: parking_lot::Mutex::new((0..n).map(|_| ProviderHealth::default()).collect()),
        }
    }

    /// Add a provider to the fallback chain.
    pub fn push(&mut self, provider: impl ChatProvider + 'static) {
        self.providers.push(Box::new(provider));
        self.health.lock().push(ProviderHealth::default());
    }

    /// Returns a slice of all providers in the fallback chain.
    pub fn providers(&self) -> &[Box<dyn ChatProvider + Send + Sync>] {
        &self.providers
    }

    /// Return the current health state for each provider in the fallback chain.
    pub fn health_status(&self) -> Vec<ProviderHealthStatus> {
        let health = self.health.lock();
        self.providers
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let h = health.get(i);
                let now = std::time::Instant::now();
                let unhealthy_until = h.and_then(|h| h.unhealthy_until);
                let cooldown_remaining_secs = unhealthy_until
                    .and_then(|t| t.checked_duration_since(now))
                    .map(|d| d.as_secs());
                let is_healthy = cooldown_remaining_secs.is_none();
                ProviderHealthStatus {
                    provider_name: p.provider_name().to_string(),
                    consecutive_errors: h.map(|h| h.consecutive_errors).unwrap_or(0),
                    is_healthy,
                    cooldown_remaining_secs,
                }
            })
            .collect()
    }

    /// Stream with fallback: calls `complete()` across all providers in order,
    /// then converts the successful response to a stream.
    ///
    /// Unlike `stream()`, this supports fallback across providers because it
    /// buffers the full response before yielding events.
    pub async fn stream_with_fallback(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> ProviderStream {
        match self.complete(messages, config).await {
            Ok(resp) => response_to_stream(resp),
            Err(e) => Box::pin(futures::stream::once(async move { Err(e) })),
        }
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
    /// Uses the first healthy provider (respects circuit-breaker cooldown).
    /// Falls back to index 0 if all providers are unhealthy (preserves best-effort behavior).
    /// Streaming cannot fall back mid-stream — use `stream_with_fallback()` for that.
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let now = std::time::Instant::now();
        let idx = {
            let h = self.health.lock();
            (0..self.providers.len()).find(|&i| {
                h.get(i)
                    .map(|health| health.unhealthy_until.map(|t| t <= now).unwrap_or(true))
                    .unwrap_or(true)
            })
            .or(if self.providers.is_empty() { None } else { Some(0) })
        };
        match idx {
            Some(i) => self.providers[i].stream(messages, config),
            None => Box::pin(futures::stream::once(async {
                Err(ProviderError::InvalidRequest("No providers available — FallbackProvider is empty or all providers are unhealthy".into()))
            })),
        }
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        let mut last_err: Option<ProviderError> = None;
        for (i, provider) in self.providers.iter().enumerate() {
            // Skip providers that are in a circuit-breaker cooldown
            {
                let h = self.health.lock();
                if let Some(h) = h.get(i)
                    && h.unhealthy_until.is_some_and(|t| std::time::Instant::now() < t)
                {
                    continue;
                }
            }

            match provider.complete(messages.clone(), config.clone()).await {
                Ok(response) => {
                    // Reset health on success
                    if let Some(h) = self.health.lock().get_mut(i) {
                        *h = ProviderHealth::default();
                    }
                    return Ok(response);
                }
                Err(e) => {
                    if should_fallback(&e, &self.strategy) {
                        tracing::debug!(
                            "FallbackProvider: provider[{}] ({}) failed with `{}`, trying next",
                            i,
                            provider.provider_name(),
                            e
                        );
                        // Track consecutive errors for circuit breaker
                        if let Some(h) = self.health.lock().get_mut(i) {
                            h.consecutive_errors += 1;
                            if h.consecutive_errors >= 3 {
                                h.unhealthy_until = Some(
                                    std::time::Instant::now()
                                        + std::time::Duration::from_secs(30),
                                );
                            }
                        }
                        last_err = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| ProviderError::InvalidRequest("No providers available — FallbackProvider is empty or all providers are unhealthy".into())))
    }
}

// ---------------------------------------------------------------------------
// Agent Loop
// ---------------------------------------------------------------------------

/// Hook callbacks for the agent loop.
#[async_trait]
pub trait AgentHooks: Send + Sync {
    /// Called before each step to inspect or modify the config (e.g. adjust temperature,
    /// filter active tools, or inject step-specific context).
    async fn prepare_step(&self, _step: usize, _config: &mut ProviderConfig) {}
    /// Return `true` to block a tool call — its result will be replaced with an
    /// "approval required" message and the agent will be asked again.
    async fn needs_approval(&self, _tool: &ToolUseBlock) -> bool {
        false
    }
    /// Called after each step completes (including all tool results for that step).
    /// Use for logging, metrics, or updating external state.
    async fn on_step_finish(&self, _step: &AgentStep) {}
}

/// No-op hooks (default).
pub struct DefaultHooks;

#[async_trait]
impl AgentHooks for DefaultHooks {}

/// Run an agent loop with hooks, step recording, and a `max_steps` limit.
///
/// Like [`run_agent_loop`], but:
/// - Returns [`AgentResult`] containing all intermediate [`AgentStep`]s, not just the final response.
/// - Calls [`AgentHooks`] callbacks on every step (`prepare_step`, `needs_approval`, `on_step_finish`).
/// - Stops after `max_steps` iterations and returns an error if exceeded.
/// - `tool_handler` returns `Vec<(id, Vec<ContentBlock>)>` — supports rich (image, audio) results.
pub async fn run_agent_loop_with_hooks<P, F, Fut, H>(
    provider: &P,
    mut messages: Vec<Message>,
    config: ProviderConfig,
    tool_handler: F,
    hooks: &H,
    max_steps: Option<usize>,
) -> Result<AgentResult, ProviderError>
where
    P: ChatProvider,
    F: Fn(Vec<ToolUseBlock>) -> Fut,
    Fut: Future<Output = Vec<(String, Vec<ContentBlock>)>> + Send,
    H: AgentHooks,
{
    let mut steps: Vec<AgentStep> = Vec::new();
    let mut step_n: usize = 0;

    loop {
        if let Some(max) = max_steps
            && step_n >= max
        {
            return Err(ProviderError::InvalidRequest(format!(
                "max_steps ({max}) exceeded without reaching EndTurn"
            )));
        }

        let mut step_config = config.clone();
        hooks.prepare_step(step_n, &mut step_config).await;

        // Apply active_tools filter to step_config (not config)
        let effective_config = if let Some(ref names) = step_config.active_tools.clone() {
            let mut c = step_config.clone();
            c.tools.retain(|t| names.contains(&t.name));
            c
        } else {
            step_config
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
        let mut precomputed_results: Vec<Option<(String, Vec<ContentBlock>)>> =
            vec![None; approved_tool_uses.len()];
        for (i, tu) in approved_tool_uses.iter().enumerate() {
            if hooks.needs_approval(tu).await {
                precomputed_results[i] = Some((
                    tu.id.clone(),
                    vec![ContentBlock::text("Tool call requires human approval")],
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
        let tool_results: Vec<(String, Vec<ContentBlock>)> = approved_tool_uses
            .iter()
            .zip(precomputed_results.iter())
            .map(|(tu, precomputed)| {
                if let Some(r) = precomputed {
                    r.clone()
                } else {
                    handler_iter
                        .next()
                        .unwrap_or_else(|| (tu.id.clone(), vec![]))
                }
            })
            .collect();

        messages.push(Message::with_content(
            Role::Assistant,
            response.content.clone(),
        ));
        messages.push(Message::with_tool_result_blocks(tool_results.clone()));

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
/// Loop: `complete` → append assistant turn → call `tool_handler` with all
/// [`ToolUseBlock`]s → append tool results → repeat until `stop_reason != ToolUse`.
///
/// Returns the final [`Response`] (the one that didn't request more tools).
/// For step-by-step tracing, approval gates, or rich (non-text) tool results,
/// use [`run_agent_loop_with_hooks`] instead.
///
/// # Example
/// ```no_run
/// # use sideseat::{ChatProvider, ProviderConfig, Message, ToolUseBlock, run_agent_loop};
/// # async fn example() -> Result<(), sideseat::ProviderError> {
/// # let provider = sideseat::mock::MockProvider::new();
/// let response = run_agent_loop(
///     &provider,
///     vec![Message::user("Search for Rust async tutorials")],
///     ProviderConfig::new("claude-haiku-4-5-20251001"),
///     |tools: Vec<ToolUseBlock>| async move {
///         tools.into_iter()
///             .map(|tu| (tu.id, format!("Result for {}", tu.name)))
///             .collect()
///     },
/// ).await?;
/// println!("{}", response.first_text().unwrap_or(""));
/// # Ok(()) }
/// ```
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
        move |tools| {
            let fut = tool_handler(tools);
            async move {
                fut.await
                    .into_iter()
                    .map(|(id, text)| (id, vec![ContentBlock::text(text)]))
                    .collect()
            }
        },
        &DefaultHooks,
        None,
    )
    .await?
    .response)
}

// ---------------------------------------------------------------------------
// Batch
// ---------------------------------------------------------------------------

/// Run multiple completion requests concurrently and collect all results.
///
/// Results are returned in the same order as the input requests.
/// An error at index N does not affect other requests.
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

/// Run embedding requests concurrently and collect all results.
///
/// Results are in the same order as `requests`.
pub async fn batch_embed<P: EmbeddingProvider + Sync>(
    provider: &P,
    requests: Vec<EmbeddingRequest>,
) -> Vec<Result<EmbeddingResponse, ProviderError>> {
    use futures::future::join_all;
    let futs: Vec<_> = requests.into_iter().map(|req| provider.embed(req)).collect();
    join_all(futs).await
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
        .ok_or_else(|| ProviderError::EmptyResponse("No text content in response".into()))
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
