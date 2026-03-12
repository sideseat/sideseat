use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::Mutex;

use async_trait::async_trait;
use futures::StreamExt;

use crate::error::ProviderError;
use crate::provider::{
    AudioProvider, ChatProvider, EmbeddingProvider, ImageProvider, ModerationProvider, Provider,
    ProviderStream, StatefulProvider, VideoProvider,
};
use crate::types::{
    ContentBlock, ContentBlockStart, ContentDelta, EmbeddingRequest, EmbeddingResponse,
    ImageEditRequest, ImageGenerationRequest, ImageGenerationResponse, Message, ModelInfo,
    ModerationRequest, ModerationResponse, PartialConfig, ProviderConfig, Response, SpeechRequest,
    SpeechResponse, StreamEvent, ThinkingBlock, TokenCount, TranscriptionRequest,
    TranscriptionResponse, VideoGenerationRequest, VideoGenerationResponse,
};

// ---------------------------------------------------------------------------
// Middleware trait
// ---------------------------------------------------------------------------

/// Intercepts provider calls before and after execution.
///
/// Implement any subset of the hooks; all default to pass-through.
///
/// ## Lifecycle
///
/// **`complete()` calls:** `before_complete` → provider → `after_complete` (success) **or** `on_error` (failure)
///
/// **`stream()` calls:** `before_complete` → stream events via `transform_stream` → `after_stream` (success) **or** `on_stream_error` (failure)
///
/// `after_stream` and `on_stream_error` are mutually exclusive — exactly one fires per stream.
/// `after_complete` and `on_error` are never called for streaming requests.
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

    /// Called when the provider returns an error. May transform the error (e.g. add context or
    /// re-classify). Return a different error to substitute, or the same error to propagate.
    ///
    /// Note: recovery (returning `Ok(Response)`) is handled by `FallbackProvider`, not this hook.
    async fn on_error(
        &self,
        error: ProviderError,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> ProviderError {
        error
    }

    /// Called when a stream completes successfully. Runs LIFO (same order as `after_complete`).
    async fn after_stream(&self, _messages: &[Message], _config: &ProviderConfig) {}

    /// Called when a stream terminates with an error. Runs FIFO (same order as `on_error`).
    async fn on_stream_error(
        &self,
        error: ProviderError,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> ProviderError {
        error
    }

    /// Transform the output stream. Applied LIFO (same as `after_complete`) for onion semantics.
    /// Default: pass stream through unchanged.
    ///
    /// Implementations must clone any data from `&self` into the returned stream —
    /// no borrows from `self` may escape into the returned stream.
    fn transform_stream(&self, stream: ProviderStream) -> ProviderStream {
        stream
    }

    async fn before_embed(
        &self,
        request: EmbeddingRequest,
    ) -> Result<EmbeddingRequest, ProviderError> {
        Ok(request)
    }

    async fn after_embed(
        &self,
        response: EmbeddingResponse,
    ) -> Result<EmbeddingResponse, ProviderError> {
        Ok(response)
    }

    async fn before_generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationRequest, ProviderError> {
        Ok(request)
    }

    async fn after_generate_image(
        &self,
        response: ImageGenerationResponse,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        Ok(response)
    }

    async fn before_edit_image(
        &self,
        request: ImageEditRequest,
    ) -> Result<ImageEditRequest, ProviderError> {
        Ok(request)
    }

    async fn after_edit_image(
        &self,
        response: ImageGenerationResponse,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        Ok(response)
    }

    async fn before_generate_video(
        &self,
        request: VideoGenerationRequest,
    ) -> Result<VideoGenerationRequest, ProviderError> {
        Ok(request)
    }

    async fn after_generate_video(
        &self,
        response: VideoGenerationResponse,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        Ok(response)
    }

    async fn before_generate_speech(
        &self,
        request: SpeechRequest,
    ) -> Result<SpeechRequest, ProviderError> {
        Ok(request)
    }

    async fn after_generate_speech(
        &self,
        response: SpeechResponse,
    ) -> Result<SpeechResponse, ProviderError> {
        Ok(response)
    }

    async fn before_transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionRequest, ProviderError> {
        Ok(request)
    }

    async fn after_transcribe(
        &self,
        response: TranscriptionResponse,
    ) -> Result<TranscriptionResponse, ProviderError> {
        Ok(response)
    }

    async fn before_moderate(
        &self,
        request: ModerationRequest,
    ) -> Result<ModerationRequest, ProviderError> {
        Ok(request)
    }

    async fn after_moderate(
        &self,
        response: ModerationResponse,
    ) -> Result<ModerationResponse, ProviderError> {
        Ok(response)
    }
}

// ---------------------------------------------------------------------------
// MiddlewareStack
// ---------------------------------------------------------------------------

/// Wraps a provider with an ordered list of middlewares (onion model).
///
/// - `before_complete` runs FIFO (first added = outermost).
/// - `after_complete` / `transform_stream` run LIFO (last added = innermost runs first).
/// - `on_error` runs FIFO; each middleware may transform the error.
pub struct MiddlewareStack<P> {
    inner: Arc<P>,
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl<P: Provider + Default + 'static> Default for MiddlewareStack<P> {
    fn default() -> Self {
        Self::new(P::default())
    }
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

    pub fn inner(&self) -> &P {
        &self.inner
    }
}

/// Propagate an error through all `on_error` hooks (FIFO).
async fn propagate_error(middlewares: &[Arc<dyn Middleware>], e: ProviderError) -> ProviderError {
    let mut err = e;
    for mw in middlewares {
        err = mw.on_error(err, &[], &ProviderConfig::default()).await;
    }
    err
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

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.inner.list_models().await
    }
}

#[async_trait]
impl<P: ChatProvider + Send + Sync + 'static> ChatProvider for MiddlewareStack<P> {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let middlewares = self.middlewares.clone();
        let inner = self.inner.clone();

        Box::pin(async_stream::try_stream! {
            let (messages, config) = run_before_pipeline(&middlewares, messages, config).await?;

            for w in config.validate(inner.provider_name()) {
                tracing::warn!("ProviderConfig: {w}");
            }

            // Strip internal middleware keys before forwarding to the inner provider.
            let mut inner_config = config.clone();
            inner_config.extra.retain(|k, _| !k.starts_with('_'));

            let mut s: ProviderStream = inner.stream(messages.clone(), inner_config);
            // Apply transform_stream in LIFO order (outer middleware wraps inner — onion model)
            for mw in middlewares.iter().rev() {
                s = mw.transform_stream(s);
            }
            futures::pin_mut!(s);
            while let Some(item) = s.next().await {
                match item {
                    Ok(event) => yield event,
                    Err(e) => {
                        let mut err = e;
                        for mw in middlewares.iter() {
                            err = mw.on_stream_error(err, &messages, &config).await;
                        }
                        Err(err)?;
                    }
                }
            }
            // Stream completed successfully — fire after_stream in LIFO order.
            for mw in middlewares.iter().rev() {
                mw.after_stream(&messages, &config).await;
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

        for w in config.validate(self.inner.provider_name()) {
            tracing::warn!("ProviderConfig: {w}");
        }

        // Strip internal middleware keys (e.g. "_tm" from TimingMiddleware) before
        // forwarding to the provider — providers must not receive unknown extra fields.
        let mut inner_config = config.clone();
        inner_config.extra.retain(|k, _| !k.starts_with('_'));

        let result = self.inner.complete(messages.clone(), inner_config).await;

        match result {
            Ok(mut response) => {
                // after_complete in reverse order (LIFO)
                for mw in self.middlewares.iter().rev() {
                    response = mw.after_complete(response, &messages, &config).await?;
                }
                Ok(response)
            }
            Err(e) => {
                let mut err = e;
                for mw in &self.middlewares {
                    err = mw.on_error(err, &messages, &config).await;
                }
                Err(err)
            }
        }
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        let (messages, config) = run_before_pipeline(&self.middlewares, messages, config).await?;
        let mut inner_config = config.clone();
        inner_config.extra.retain(|k, _| !k.starts_with('_'));
        self.inner.count_tokens(messages, inner_config).await
    }
}

#[async_trait]
impl<P: Provider + EmbeddingProvider + Send + Sync + 'static> EmbeddingProvider
    for MiddlewareStack<P>
{
    async fn embed(
        &self,
        mut request: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ProviderError> {
        for mw in &self.middlewares {
            request = mw.before_embed(request).await?;
        }
        let mut response = match self.inner.embed(request).await {
            Ok(r) => r,
            Err(e) => return Err(propagate_error(&self.middlewares, e).await),
        };
        for mw in self.middlewares.iter().rev() {
            response = mw.after_embed(response).await?;
        }
        Ok(response)
    }
}

#[async_trait]
impl<P: Provider + ImageProvider + Send + Sync + 'static> ImageProvider for MiddlewareStack<P> {
    async fn generate_image(
        &self,
        mut request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        for mw in &self.middlewares {
            request = mw.before_generate_image(request).await?;
        }
        let mut response = match self.inner.generate_image(request).await {
            Ok(r) => r,
            Err(e) => return Err(propagate_error(&self.middlewares, e).await),
        };
        for mw in self.middlewares.iter().rev() {
            response = mw.after_generate_image(response).await?;
        }
        Ok(response)
    }

    async fn edit_image(
        &self,
        mut request: ImageEditRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        for mw in &self.middlewares {
            request = mw.before_edit_image(request).await?;
        }
        let mut response = match self.inner.edit_image(request).await {
            Ok(r) => r,
            Err(e) => return Err(propagate_error(&self.middlewares, e).await),
        };
        for mw in self.middlewares.iter().rev() {
            response = mw.after_edit_image(response).await?;
        }
        Ok(response)
    }
}

#[async_trait]
impl<P: Provider + VideoProvider + Send + Sync + 'static> VideoProvider for MiddlewareStack<P> {
    async fn generate_video(
        &self,
        mut request: VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        for mw in &self.middlewares {
            request = mw.before_generate_video(request).await?;
        }
        let mut response = match self.inner.generate_video(request).await {
            Ok(r) => r,
            Err(e) => return Err(propagate_error(&self.middlewares, e).await),
        };
        for mw in self.middlewares.iter().rev() {
            response = mw.after_generate_video(response).await?;
        }
        Ok(response)
    }
}

#[async_trait]
impl<P: Provider + AudioProvider + Send + Sync + 'static> AudioProvider for MiddlewareStack<P> {
    async fn generate_speech(
        &self,
        mut request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        for mw in &self.middlewares {
            request = mw.before_generate_speech(request).await?;
        }
        let mut response = match self.inner.generate_speech(request).await {
            Ok(r) => r,
            Err(e) => return Err(propagate_error(&self.middlewares, e).await),
        };
        for mw in self.middlewares.iter().rev() {
            response = mw.after_generate_speech(response).await?;
        }
        Ok(response)
    }

    async fn transcribe(
        &self,
        mut request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        for mw in &self.middlewares {
            request = mw.before_transcribe(request).await?;
        }
        let mut response = match self.inner.transcribe(request).await {
            Ok(r) => r,
            Err(e) => return Err(propagate_error(&self.middlewares, e).await),
        };
        for mw in self.middlewares.iter().rev() {
            response = mw.after_transcribe(response).await?;
        }
        Ok(response)
    }

    async fn translate(
        &self,
        mut request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        for mw in &self.middlewares {
            request = mw.before_transcribe(request).await?;
        }
        let mut response = match self.inner.translate(request).await {
            Ok(r) => r,
            Err(e) => return Err(propagate_error(&self.middlewares, e).await),
        };
        for mw in self.middlewares.iter().rev() {
            response = mw.after_transcribe(response).await?;
        }
        Ok(response)
    }
}

#[async_trait]
impl<P: Provider + ModerationProvider + Send + Sync + 'static> ModerationProvider
    for MiddlewareStack<P>
{
    async fn moderate(
        &self,
        mut request: ModerationRequest,
    ) -> Result<ModerationResponse, ProviderError> {
        for mw in &self.middlewares {
            request = mw.before_moderate(request).await?;
        }
        let mut response = match self.inner.moderate(request).await {
            Ok(r) => r,
            Err(e) => return Err(propagate_error(&self.middlewares, e).await),
        };
        for mw in self.middlewares.iter().rev() {
            response = mw.after_moderate(response).await?;
        }
        Ok(response)
    }
}

#[async_trait]
impl<P: Provider + StatefulProvider + Send + Sync + 'static> StatefulProvider
    for MiddlewareStack<P>
{
    async fn retrieve_response(
        &self,
        response_id: &str,
    ) -> Result<crate::types::Response, ProviderError> {
        self.inner.retrieve_response(response_id).await
    }

    async fn cancel_response(
        &self,
        response_id: &str,
    ) -> Result<crate::types::Response, ProviderError> {
        self.inner.cancel_response(response_id).await
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
    ) -> ProviderError {
        tracing::debug!("<- provider error: {}", error);
        error
    }

    async fn after_stream(&self, _messages: &[Message], _config: &ProviderConfig) {
        tracing::debug!("<- stream completed");
    }

    async fn on_stream_error(
        &self,
        error: ProviderError,
        _messages: &[Message],
        _config: &ProviderConfig,
    ) -> ProviderError {
        tracing::debug!("<- stream error: {}", error);
        error
    }
}

/// Logs the elapsed time of each `complete()` call via `tracing::debug!`.
///
/// Uses a counter + HashMap to correctly match start/end times for concurrent calls.
/// The request ID is injected into `config.extra["_tm"]` and read back in after/error hooks.
pub struct TimingMiddleware {
    counter: Arc<AtomicU64>,
    starts: Arc<Mutex<HashMap<u64, Instant>>>,
}

impl Default for TimingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl TimingMiddleware {
    pub fn new() -> Self {
        Self {
            counter: Arc::new(AtomicU64::new(0)),
            starts: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl Middleware for TimingMiddleware {
    async fn before_complete(
        &self,
        messages: Vec<Message>,
        mut config: ProviderConfig,
    ) -> Result<(Vec<Message>, ProviderConfig), ProviderError> {
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        self.starts.lock().insert(id, Instant::now());
        config
            .extra
            .insert("_tm".into(), serde_json::Value::from(id));
        Ok((messages, config))
    }

    async fn after_complete(
        &self,
        response: Response,
        _messages: &[Message],
        config: &ProviderConfig,
    ) -> Result<Response, ProviderError> {
        if let Some(id) = config.extra.get("_tm").and_then(|v| v.as_u64())
            && let Some(start) = self.starts.lock().remove(&id)
        {
            tracing::debug!("request completed in {}ms", start.elapsed().as_millis());
        }
        Ok(response)
    }

    async fn on_error(
        &self,
        error: ProviderError,
        _messages: &[Message],
        config: &ProviderConfig,
    ) -> ProviderError {
        if let Some(id) = config.extra.get("_tm").and_then(|v| v.as_u64())
            && let Some(start) = self.starts.lock().remove(&id)
        {
            tracing::debug!("request failed in {}ms", start.elapsed().as_millis());
        }
        error
    }

    async fn after_stream(&self, _messages: &[Message], config: &ProviderConfig) {
        if let Some(id) = config.extra.get("_tm").and_then(|v| v.as_u64())
            && let Some(start) = self.starts.lock().remove(&id)
        {
            tracing::debug!("stream completed in {}ms", start.elapsed().as_millis());
        }
    }

    async fn on_stream_error(
        &self,
        error: ProviderError,
        _messages: &[Message],
        config: &ProviderConfig,
    ) -> ProviderError {
        if let Some(id) = config.extra.get("_tm").and_then(|v| v.as_u64())
            && let Some(start) = self.starts.lock().remove(&id)
        {
            tracing::debug!("stream failed in {}ms", start.elapsed().as_millis());
        }
        error
    }
}

// ---------------------------------------------------------------------------
// RateLimitMiddleware
// ---------------------------------------------------------------------------

/// Limits provider requests to a maximum rate using a sliding window algorithm.
///
/// When the limit is reached, requests are delayed (not rejected) until the
/// window resets. Thread-safe — safe to share across concurrent callers.
///
/// # Example
/// ```
/// use sideseat::middleware::{MiddlewareStack, RateLimitMiddleware};
/// use sideseat::mock::MockProvider;
///
/// // Allow at most 60 requests per minute
/// let stack = MiddlewareStack::new(MockProvider::new())
///     .with(RateLimitMiddleware::per_minute(60));
/// ```
pub struct RateLimitMiddleware {
    max_per_window: u32,
    window_ms: u64,
    state: Arc<Mutex<RateLimiterState>>,
}

struct RateLimiterState {
    window_start: std::time::Instant,
    count: u32,
}

impl RateLimitMiddleware {
    /// Create a rate limiter with `max_requests` allowed per `window_ms` milliseconds.
    pub fn new(max_requests: u32, window_ms: u64) -> Self {
        Self {
            max_per_window: max_requests,
            window_ms,
            state: Arc::new(Mutex::new(RateLimiterState {
                window_start: std::time::Instant::now(),
                count: 0,
            })),
        }
    }

    /// Limit to `n` requests per minute.
    pub fn per_minute(n: u32) -> Self {
        Self::new(n, 60_000)
    }

    /// Limit to `n` requests per second.
    pub fn per_second(n: u32) -> Self {
        Self::new(n, 1_000)
    }

    async fn acquire(&self) {
        loop {
            let sleep_ms = {
                let mut s = self.state.lock();
                let elapsed = s.window_start.elapsed().as_millis() as u64;
                if elapsed >= self.window_ms {
                    // New window
                    s.window_start = std::time::Instant::now();
                    s.count = 1;
                    0
                } else if s.count < self.max_per_window {
                    s.count += 1;
                    0
                } else {
                    // Window full — sleep until it resets
                    self.window_ms - elapsed
                }
            };
            if sleep_ms == 0 {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(sleep_ms)).await;
        }
    }
}

#[async_trait]
impl Middleware for RateLimitMiddleware {
    async fn before_complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<(Vec<Message>, ProviderConfig), ProviderError> {
        self.acquire().await;
        Ok((messages, config))
    }

    async fn before_embed(
        &self,
        request: EmbeddingRequest,
    ) -> Result<EmbeddingRequest, ProviderError> {
        self.acquire().await;
        Ok(request)
    }

    async fn before_generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationRequest, ProviderError> {
        self.acquire().await;
        Ok(request)
    }

    async fn before_generate_video(
        &self,
        request: VideoGenerationRequest,
    ) -> Result<VideoGenerationRequest, ProviderError> {
        self.acquire().await;
        Ok(request)
    }

    async fn before_generate_speech(
        &self,
        request: SpeechRequest,
    ) -> Result<SpeechRequest, ProviderError> {
        self.acquire().await;
        Ok(request)
    }

    async fn before_transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionRequest, ProviderError> {
        self.acquire().await;
        Ok(request)
    }

    async fn before_moderate(
        &self,
        request: ModerationRequest,
    ) -> Result<ModerationRequest, ProviderError> {
        self.acquire().await;
        Ok(request)
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
    /// Create with default `<think>` / `</think>` tags.
    pub fn new() -> Self {
        Self {
            open_tag: "<think>".into(),
            close_tag: "</think>".into(),
        }
    }

    /// Create with custom open and close tags (e.g. `"<reasoning>"`, `"</reasoning>"`).
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
            if let ContentBlock::Text(ref tb) = block {
                let (remaining_text, segments) =
                    extract_think_segments(&tb.text, &self.open_tag, &self.close_tag);
                for seg in segments {
                    new_content.push(ContentBlock::Thinking(ThinkingBlock {
                        text: seg,
                        signature: None,
                    }));
                }
                if !remaining_text.trim().is_empty() {
                    new_content.push(ContentBlock::text(remaining_text));
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
                        if let Some(mut buf) = think_buf.take() {
                            // inside a think block
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
                                            delta: ContentDelta::Thinking { text: thinking_content },
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
                                        delta: ContentDelta::Thinking { text: safe },
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
                                    delta: ContentDelta::Thinking { text: buf },
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

    fn before_edit(&self, request: ImageEditRequest) -> ImageEditRequest {
        request
    }

    fn after_edit(&self, response: ImageGenerationResponse) -> ImageGenerationResponse {
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
    P: ImageProvider + ChatProvider + Send + Sync + 'static,
    M: ImageModelMiddleware + 'static,
{
    WrappedImageModel {
        inner: provider,
        middleware,
    }
}

#[async_trait]
impl<P: ImageProvider + ChatProvider + Send + Sync, M: ImageModelMiddleware + Send + Sync> Provider
    for WrappedImageModel<P, M>
{
    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.inner.list_models().await
    }
}

#[async_trait]
impl<P: ImageProvider + ChatProvider + Send + Sync, M: ImageModelMiddleware + Send + Sync>
    ChatProvider for WrappedImageModel<P, M>
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

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        self.inner.count_tokens(messages, config).await
    }
}

#[async_trait]
impl<P: ImageProvider + ChatProvider + Send + Sync, M: ImageModelMiddleware + Send + Sync>
    ImageProvider for WrappedImageModel<P, M>
{
    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        let request = self.middleware.before_generate(request);
        let response = self.inner.generate_image(request).await?;
        Ok(self.middleware.after_generate(response))
    }

    async fn edit_image(
        &self,
        request: ImageEditRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        let request = self.middleware.before_edit(request);
        let response = self.inner.edit_image(request).await?;
        Ok(self.middleware.after_edit(response))
    }
}
