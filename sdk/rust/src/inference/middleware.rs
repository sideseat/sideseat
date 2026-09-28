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

#[path = "middleware/builtins.rs"]
mod builtins;
pub use builtins::{
    DefaultSettingsMiddleware, ExtractReasoningMiddleware, LoggingMiddleware, RateLimitMiddleware,
    SimulateStreamingMiddleware, TimingMiddleware,
};

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
