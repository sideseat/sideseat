use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use async_trait::async_trait;
use futures::StreamExt;

use crate::error::ProviderError;
use crate::provider::{Provider, ProviderStream};
use crate::types::{
    ContentBlock, ContentBlockStart, ContentDelta, EmbeddingRequest, EmbeddingResponse,
    ImageGenerationRequest, ImageGenerationResponse, Message, ModelInfo, PartialConfig,
    ProviderConfig, Response, SpeechRequest, SpeechResponse, StreamEvent, ThinkingBlock,
    TokenCount, TranscriptionRequest, TranscriptionResponse, VideoGenerationRequest,
    VideoGenerationResponse,
};

// ---------------------------------------------------------------------------
// Middleware trait
// ---------------------------------------------------------------------------

/// Intercepts provider calls before and after execution.
///
/// Implement any subset of the three hooks; defaults are pass-through.
///
/// Note: `after_complete` and `on_error` do NOT apply to `stream()` calls —
/// only `before_complete` is applied to streaming requests.
#[async_trait]
pub trait Middleware: Send + Sync {
    /// Called before forwarding to the provider. Return modified (messages, config).
    async fn before_complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<(Vec<Message>, ProviderConfig), ProviderError> {
        Ok((messages, config))
    }

    /// Called after a successful response. May modify the response.
    async fn after_complete(
        &self,
        response: Response,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> Result<Response, ProviderError> {
        Ok(response)
    }

    /// Called on error. Return `Ok` to recover, `Err` to propagate.
    async fn on_error(
        &self,
        error: ProviderError,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> Result<Response, ProviderError> {
        Err(error)
    }

    /// Transform the output stream. Applied FIFO after `before_complete`.
    /// Default: pass stream through unchanged.
    ///
    /// Implementations must clone any data from `&self` into the returned stream —
    /// no borrows from `self` may escape into the returned stream.
    fn transform_stream(&self, stream: ProviderStream) -> ProviderStream {
        stream
    }
}

// ---------------------------------------------------------------------------
// MiddlewareStack
// ---------------------------------------------------------------------------

/// Wraps a provider with an ordered list of middlewares (onion model).
///
/// - `before_complete` runs FIFO (first added = outermost).
/// - `after_complete` runs LIFO (last added = innermost runs first).
/// - `on_error` runs FIFO; first middleware to return `Ok` wins.
///
/// For `stream()`, only `before_complete` is applied.
pub struct MiddlewareStack<P> {
    inner: Arc<P>,
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl<P: Provider + 'static> MiddlewareStack<P> {
    pub fn new(provider: P) -> Self {
        Self {
            inner: Arc::new(provider),
            middlewares: Vec::new(),
        }
    }

    /// Add a middleware to the stack.
    pub fn with(mut self, mw: impl Middleware + 'static) -> Self {
        self.middlewares.push(Arc::new(mw));
        self
    }
}

async fn run_before_pipeline(
    middlewares: &[Arc<dyn Middleware>],
    mut messages: Vec<Message>,
    mut config: ProviderConfig,
) -> Result<(Vec<Message>, ProviderConfig), ProviderError> {
    for mw in middlewares {
        (messages, config) = mw.before_complete(messages, config).await?;
    }
    Ok((messages, config))
}

#[async_trait]
impl<P: Provider + Send + Sync + 'static> Provider for MiddlewareStack<P> {
    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }

    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let middlewares = self.middlewares.clone();
        let inner = self.inner.clone();

        Box::pin(async_stream::try_stream! {
            let (messages, config) = run_before_pipeline(&middlewares, messages, config).await?;
            let mut s: ProviderStream = inner.stream(messages, config);
            // Apply transform_stream in FIFO order
            for mw in &middlewares {
                s = mw.transform_stream(s);
            }
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
        let (messages, config) =
            run_before_pipeline(&self.middlewares, messages.clone(), config.clone()).await?;

        let result = self.inner.complete(messages.clone(), config.clone()).await;

        match result {
            Ok(mut response) => {
                // after_complete in reverse order (LIFO)
                for mw in self.middlewares.iter().rev() {
                    response = mw.after_complete(response, &messages, &config).await?;
                }
                Ok(response)
            }
            Err(e) => {
                // on_error in order; first Ok wins
                let mut err = e;
                for mw in &self.middlewares {
                    match mw.on_error(err, &messages, &config).await {
                        Ok(response) => return Ok(response),
                        Err(e) => err = e,
                    }
                }
                Err(err)
            }
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.inner.list_models().await
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        self.inner.count_tokens(messages, config).await
    }

    async fn embed(
        &self,
        request: EmbeddingRequest,
        model: &str,
    ) -> Result<EmbeddingResponse, ProviderError> {
        self.inner.embed(request, model).await
    }

    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        self.inner.generate_image(request).await
    }

    async fn generate_video(
        &self,
        request: VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        self.inner.generate_video(request).await
    }

    async fn generate_speech(
        &self,
        request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        self.inner.generate_speech(request).await
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        self.inner.transcribe(request).await
    }
}

// ---------------------------------------------------------------------------
// Built-in middlewares
// ---------------------------------------------------------------------------

/// Logs provider requests and responses via `tracing::debug!`.
pub struct LoggingMiddleware;

#[async_trait]
impl Middleware for LoggingMiddleware {
    async fn before_complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<(Vec<Message>, ProviderConfig), ProviderError> {
        tracing::debug!(
            "-> provider request: model={} messages={}",
            config.model,
            messages.len()
        );
        Ok((messages, config))
    }

    async fn after_complete(
        &self,
        response: Response,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> Result<Response, ProviderError> {
        tracing::debug!(
            "<- provider response: model={:?} input={} output={}",
            response.model,
            response.usage.input_tokens,
            response.usage.output_tokens
        );
        Ok(response)
    }

    async fn on_error(
        &self,
        error: ProviderError,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> Result<Response, ProviderError> {
        tracing::debug!("<- provider error: {}", error);
        Err(error)
    }
}

/// Logs the elapsed time of each `complete()` call via `tracing::debug!`.
///
/// Uses a FIFO queue of start times to handle concurrent calls correctly.
pub struct TimingMiddleware {
    starts: Arc<Mutex<VecDeque<Instant>>>,
}

impl Default for TimingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl TimingMiddleware {
    pub fn new() -> Self {
        Self {
            starts: Arc::new(Mutex::new(VecDeque::new())),
        }
    }
}

#[async_trait]
impl Middleware for TimingMiddleware {
    async fn before_complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<(Vec<Message>, ProviderConfig), ProviderError> {
        self.starts.lock().push_back(Instant::now());
        Ok((messages, config))
    }

    async fn after_complete(
        &self,
        response: Response,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> Result<Response, ProviderError> {
        if let Some(start) = self.starts.lock().pop_front() {
            tracing::debug!("request completed in {}ms", start.elapsed().as_millis());
        }
        Ok(response)
    }

    async fn on_error(
        &self,
        error: ProviderError,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> Result<Response, ProviderError> {
        if let Some(start) = self.starts.lock().pop_front() {
            tracing::debug!("request failed in {}ms", start.elapsed().as_millis());
        }
        Err(error)
    }
}

// ---------------------------------------------------------------------------
// ExtractReasoningMiddleware
// ---------------------------------------------------------------------------

/// Extracts `<think>...</think>` (or custom tags) from text content into
/// `ContentBlock::Thinking` blocks in both complete and streaming paths.
pub struct ExtractReasoningMiddleware {
    open_tag: String,
    close_tag: String,
}

impl ExtractReasoningMiddleware {
    pub fn new() -> Self {
        Self {
            open_tag: "<think>".into(),
            close_tag: "</think>".into(),
        }
    }

    pub fn with_tags(open: &str, close: &str) -> Self {
        Self {
            open_tag: open.into(),
            close_tag: close.into(),
        }
    }
}

impl Default for ExtractReasoningMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract all `open_tag..close_tag` pairs from `text`, returning
/// `(text_without_tags, Vec<thinking_segment>)`.
fn extract_think_segments(text: &str, open_tag: &str, close_tag: &str) -> (String, Vec<String>) {
    let mut result_text = String::new();
    let mut thinking_segments = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find(open_tag) {
        result_text.push_str(&remaining[..start]);
        remaining = &remaining[start + open_tag.len()..];
        if let Some(end) = remaining.find(close_tag) {
            thinking_segments.push(remaining[..end].to_string());
            remaining = &remaining[end + close_tag.len()..];
        } else {
            // Unclosed tag — treat rest as thinking
            thinking_segments.push(remaining.to_string());
            remaining = "";
            break;
        }
    }
    result_text.push_str(remaining);
    (result_text, thinking_segments)
}

#[async_trait]
impl Middleware for ExtractReasoningMiddleware {
    async fn after_complete(
        &self,
        mut response: Response,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> Result<Response, ProviderError> {
        let mut new_content: Vec<ContentBlock> = Vec::new();
        for block in response.content {
            if let ContentBlock::Text(text) = &block {
                let (remaining_text, segments) =
                    extract_think_segments(text, &self.open_tag, &self.close_tag);
                for seg in segments {
                    new_content.push(ContentBlock::Thinking(ThinkingBlock {
                        thinking: seg,
                        signature: None,
                    }));
                }
                if !remaining_text.trim().is_empty() {
                    new_content.push(ContentBlock::Text(remaining_text));
                }
            } else {
                new_content.push(block);
            }
        }
        response.content = new_content;
        Ok(response)
    }

    fn transform_stream(&self, stream: ProviderStream) -> ProviderStream {
        let open_tag = self.open_tag.clone();
        let close_tag = self.close_tag.clone();

        Box::pin(async_stream::try_stream! {
            futures::pin_mut!(stream);

            // State machine for extracting think tags from streamed text
            let mut normal_buf = String::new();
            let mut think_buf: Option<String> = None;
            // track whether we've started a text block so we can emit Stop
            let mut in_text_block = false;
            let mut in_think_block = false;
            // index for injected blocks (high range to avoid collision)
            let mut inject_index: usize = 10_000;
            // original text block index (for ContentBlockStop passthrough)
            let mut text_block_index: Option<usize> = None;

            while let Some(item) = stream.next().await {
                let event = item?;
                match event {
                    StreamEvent::ContentBlockStart { index, block: ContentBlockStart::Text } => {
                        text_block_index = Some(index);
                        // Defer emitting start until we know if it's pure text or mixed
                        // We'll emit it on first non-think content
                    }
                    StreamEvent::ContentBlockDelta { index, delta: ContentDelta::Text { text } }
                        if text_block_index == Some(index) =>
                    {
                        // Feed into state machine
                        if think_buf.is_some() {
                            // inside a think block
                            let mut buf = think_buf.take().unwrap();
                            buf.push_str(&text);
                            // Check for close tag
                            if let Some(pos) = buf.find(&close_tag) {
                                let thinking_content = buf[..pos].to_string();
                                let suffix = buf[pos + close_tag.len()..].to_string();
                                // Emit accumulated thinking
                                if in_think_block {
                                    if !thinking_content.is_empty() {
                                        yield StreamEvent::ContentBlockDelta {
                                            index: inject_index,
                                            delta: ContentDelta::Thinking { thinking: thinking_content },
                                        };
                                    }
                                    yield StreamEvent::ContentBlockStop { index: inject_index };
                                    in_think_block = false;
                                    inject_index += 1;
                                }
                                think_buf = None;
                                normal_buf = suffix;
                            } else {
                                // Flush safe prefix of think_buf
                                let safe_len = buf.len().saturating_sub(close_tag.len());
                                if safe_len > 0 {
                                    let safe = buf[..safe_len].to_string();
                                    if !in_think_block {
                                        yield StreamEvent::ContentBlockStart {
                                            index: inject_index,
                                            block: ContentBlockStart::Thinking,
                                        };
                                        in_think_block = true;
                                    }
                                    yield StreamEvent::ContentBlockDelta {
                                        index: inject_index,
                                        delta: ContentDelta::Thinking { thinking: safe },
                                    };
                                    think_buf = Some(buf[safe_len..].to_string());
                                } else {
                                    think_buf = Some(buf);
                                }
                            }
                        } else {
                            // In normal text mode
                            normal_buf.push_str(&text);
                            // Check for open tag
                            if let Some(pos) = normal_buf.find(&open_tag) {
                                // Flush prefix as text
                                let prefix = normal_buf[..pos].to_string();
                                if !prefix.is_empty() {
                                    if !in_text_block {
                                        yield StreamEvent::ContentBlockStart {
                                            index: text_block_index.unwrap_or(0),
                                            block: ContentBlockStart::Text,
                                        };
                                    }
                                    yield StreamEvent::ContentBlockDelta {
                                        index: text_block_index.unwrap_or(0),
                                        delta: ContentDelta::Text { text: prefix },
                                    };
                                    yield StreamEvent::ContentBlockStop {
                                        index: text_block_index.unwrap_or(0),
                                    };
                                    in_text_block = false;
                                }
                                normal_buf = normal_buf[pos + open_tag.len()..].to_string();
                                think_buf = Some(String::new());
                                // Don't start think block yet — wait for content
                            } else {
                                // No open tag — flush safe prefix
                                let safe_len = normal_buf.len().saturating_sub(open_tag.len());
                                if safe_len > 0 {
                                    let safe = normal_buf[..safe_len].to_string();
                                    if !in_text_block {
                                        yield StreamEvent::ContentBlockStart {
                                            index: text_block_index.unwrap_or(0),
                                            block: ContentBlockStart::Text,
                                        };
                                        in_text_block = true;
                                    }
                                    yield StreamEvent::ContentBlockDelta {
                                        index: text_block_index.unwrap_or(0),
                                        delta: ContentDelta::Text { text: safe },
                                    };
                                    normal_buf = normal_buf[safe_len..].to_string();
                                }
                            }
                        }
                    }
                    StreamEvent::ContentBlockStop { index }
                        if text_block_index == Some(index) =>
                    {
                        // Flush remaining buffers
                        if let Some(buf) = think_buf.take() {
                            if !buf.is_empty() {
                                if !in_think_block {
                                    yield StreamEvent::ContentBlockStart {
                                        index: inject_index,
                                        block: ContentBlockStart::Thinking,
                                    };
                                    in_think_block = true;
                                }
                                yield StreamEvent::ContentBlockDelta {
                                    index: inject_index,
                                    delta: ContentDelta::Thinking { thinking: buf },
                                };
                            }
                            if in_think_block {
                                yield StreamEvent::ContentBlockStop { index: inject_index };
                                inject_index += 1;
                                in_think_block = false;
                            }
                        }
                        if !normal_buf.is_empty() {
                            if !in_text_block {
                                yield StreamEvent::ContentBlockStart {
                                    index: text_block_index.unwrap_or(0),
                                    block: ContentBlockStart::Text,
                                };
                                in_text_block = true;
                            }
                            yield StreamEvent::ContentBlockDelta {
                                index: text_block_index.unwrap_or(0),
                                delta: ContentDelta::Text { text: normal_buf.clone() },
                            };
                            normal_buf.clear();
                        }
                        if in_text_block {
                            yield StreamEvent::ContentBlockStop {
                                index: text_block_index.unwrap_or(0),
                            };
                            in_text_block = false;
                        }
                        text_block_index = None;
                    }
                    other => yield other,
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// SimulateStreamingMiddleware
// ---------------------------------------------------------------------------

/// Re-emits a collected stream event-by-event with an optional delay between each.
///
/// Useful for testing or for providing a consistent streaming interface
/// over non-streaming providers.
pub struct SimulateStreamingMiddleware {
    chunk_delay_ms: u64,
}

impl SimulateStreamingMiddleware {
    pub fn new() -> Self {
        Self { chunk_delay_ms: 0 }
    }

    pub fn with_delay_ms(mut self, ms: u64) -> Self {
        self.chunk_delay_ms = ms;
        self
    }
}

impl Default for SimulateStreamingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for SimulateStreamingMiddleware {
    fn transform_stream(&self, stream: ProviderStream) -> ProviderStream {
        let delay_ms = self.chunk_delay_ms;
        Box::pin(async_stream::try_stream! {
            // Collect all events first
            let mut events: Vec<StreamEvent> = Vec::new();
            futures::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                events.push(item?);
            }
            // Re-emit with optional delay
            for event in events {
                if delay_ms > 0 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                }
                yield event;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// DefaultSettingsMiddleware
// ---------------------------------------------------------------------------

/// Fills in unset fields of `ProviderConfig` with defaults. Caller-set values always win.
pub struct DefaultSettingsMiddleware {
    defaults: PartialConfig,
}

impl DefaultSettingsMiddleware {
    pub fn new() -> Self {
        Self {
            defaults: PartialConfig::default(),
        }
    }

    pub fn with_temperature(mut self, t: f64) -> Self {
        self.defaults.temperature = Some(t);
        self
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.defaults.max_tokens = Some(n);
        self
    }

    pub fn with_top_p(mut self, p: f64) -> Self {
        self.defaults.top_p = Some(p);
        self
    }

    pub fn with_top_k(mut self, k: u32) -> Self {
        self.defaults.top_k = Some(k);
        self
    }

    pub fn with_seed(mut self, s: u64) -> Self {
        self.defaults.seed = Some(s);
        self
    }

    pub fn with_stop_sequences(mut self, v: Vec<String>) -> Self {
        self.defaults.stop_sequences = Some(v);
        self
    }
}

impl Default for DefaultSettingsMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for DefaultSettingsMiddleware {
    async fn before_complete(
        &self,
        messages: Vec<Message>,
        mut config: ProviderConfig,
    ) -> Result<(Vec<Message>, ProviderConfig), ProviderError> {
        if config.temperature.is_none() {
            config.temperature = self.defaults.temperature;
        }
        if config.max_tokens.is_none() {
            config.max_tokens = self.defaults.max_tokens;
        }
        if config.top_p.is_none() {
            config.top_p = self.defaults.top_p;
        }
        if config.top_k.is_none() {
            config.top_k = self.defaults.top_k;
        }
        if config.seed.is_none() {
            config.seed = self.defaults.seed;
        }
        if config.stop_sequences.is_empty()
            && let Some(ref seqs) = self.defaults.stop_sequences
        {
            config.stop_sequences = seqs.clone();
        }
        Ok((messages, config))
    }
}

// ---------------------------------------------------------------------------
// TelemetryMiddleware
// ---------------------------------------------------------------------------

/// Emits `tracing` events for LLM requests, responses, and errors.
pub struct TelemetryMiddleware {
    function_id: String,
}

impl TelemetryMiddleware {
    pub fn new(function_id: impl Into<String>) -> Self {
        Self {
            function_id: function_id.into(),
        }
    }
}

#[async_trait]
impl Middleware for TelemetryMiddleware {
    async fn before_complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<(Vec<Message>, ProviderConfig), ProviderError> {
        tracing::info!(
            function_id = %self.function_id,
            model = %config.model,
            message_count = messages.len(),
            "llm.request"
        );
        Ok((messages, config))
    }

    async fn after_complete(
        &self,
        response: Response,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> Result<Response, ProviderError> {
        tracing::info!(
            function_id = %self.function_id,
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            stop_reason = ?response.stop_reason,
            "llm.response"
        );
        Ok(response)
    }

    async fn on_error(
        &self,
        err: ProviderError,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> Result<Response, ProviderError> {
        tracing::warn!(function_id = %self.function_id, error = %err, "llm.error");
        Err(err)
    }

    fn transform_stream(&self, stream: ProviderStream) -> ProviderStream {
        let function_id = self.function_id.clone();
        Box::pin(async_stream::try_stream! {
            futures::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                let event = item?;
                match &event {
                    StreamEvent::MessageStop { stop_reason } => {
                        tracing::info!(
                            function_id = %function_id,
                            stop_reason = ?stop_reason,
                            "llm.stream.complete"
                        );
                    }
                    StreamEvent::Metadata { usage, .. } => {
                        tracing::info!(
                            function_id = %function_id,
                            input_tokens = usage.input_tokens,
                            output_tokens = usage.output_tokens,
                            "llm.stream.usage"
                        );
                    }
                    _ => {}
                }
                yield event;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// ImageModelMiddleware
// ---------------------------------------------------------------------------

/// Intercepts image generation requests and responses.
pub trait ImageModelMiddleware: Send + Sync {
    fn before_generate(&self, request: ImageGenerationRequest) -> ImageGenerationRequest {
        request
    }

    fn after_generate(&self, response: ImageGenerationResponse) -> ImageGenerationResponse {
        response
    }
}

/// A provider wrapped with an `ImageModelMiddleware`.
pub struct WrappedImageModel<P, M> {
    inner: P,
    middleware: M,
}

/// Wrap a provider with an `ImageModelMiddleware`.
pub fn wrap_image_model<P, M>(provider: P, middleware: M) -> WrappedImageModel<P, M>
where
    P: Provider + Send + Sync + 'static,
    M: ImageModelMiddleware + 'static,
{
    WrappedImageModel {
        inner: provider,
        middleware,
    }
}

#[async_trait]
impl<P: Provider + Send + Sync, M: ImageModelMiddleware + Send + Sync> Provider
    for WrappedImageModel<P, M>
{
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        self.inner.stream(messages, config)
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        self.inner.complete(messages, config).await
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.inner.list_models().await
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        self.inner.count_tokens(messages, config).await
    }

    async fn embed(
        &self,
        request: EmbeddingRequest,
        model: &str,
    ) -> Result<EmbeddingResponse, ProviderError> {
        self.inner.embed(request, model).await
    }

    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        let request = self.middleware.before_generate(request);
        let response = self.inner.generate_image(request).await?;
        Ok(self.middleware.after_generate(response))
    }

    async fn generate_video(
        &self,
        request: crate::types::VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        self.inner.generate_video(request).await
    }

    async fn generate_speech(
        &self,
        request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        self.inner.generate_speech(request).await
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        self.inner.transcribe(request).await
    }
}
