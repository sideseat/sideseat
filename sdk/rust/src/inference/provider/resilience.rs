use super::*;

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
            tokio::time::sleep(tokio::time::Duration::from_millis(
                config.delay_for_attempt(attempt),
            ))
            .await;
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
        Self {
            inner: std::sync::Arc::new(inner),
            config,
        }
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
            (0..self.providers.len())
                .find(|&i| {
                    h.get(i)
                        .map(|health| health.unhealthy_until.map(|t| t <= now).unwrap_or(true))
                        .unwrap_or(true)
                })
                .or(if self.providers.is_empty() {
                    None
                } else {
                    Some(0)
                })
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
                    && h.unhealthy_until
                        .is_some_and(|t| std::time::Instant::now() < t)
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
                                    std::time::Instant::now() + std::time::Duration::from_secs(30),
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
        Err(last_err.unwrap_or_else(|| {
            ProviderError::InvalidRequest(
                "No providers available — FallbackProvider is empty or all providers are unhealthy"
                    .into(),
            )
        }))
    }
}
