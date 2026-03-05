use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use futures::StreamExt;
use opentelemetry::global::BoxedSpan;
use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::trace::{
    FutureExt as OtelFutureExt, Span, SpanKind, SpanRef, Status, TraceContextExt, Tracer,
};
use opentelemetry::{Array, Context, KeyValue, Value};

use crate::error::ProviderError;
use crate::provider::{
    AudioProvider, ChatProvider, EmbeddingProvider, ImageProvider, ModerationProvider, Provider,
    ProviderStream, VideoProvider,
};
use crate::types::{
    EmbeddingRequest, EmbeddingResponse, ImageEditRequest, ImageGenerationRequest,
    ImageGenerationResponse, Message, ModelInfo, ModerationRequest, ModerationResponse,
    ProviderConfig, Response, Role, SpeechRequest, SpeechResponse, StreamEvent, TokenCount,
    TranscriptionRequest, TranscriptionResponse, Usage, VideoGenerationRequest,
    VideoGenerationResponse,
};

// Semantic convention constants (GenAI is experimental — not using semconv crate)
const GEN_AI_SYSTEM: &str = "gen_ai.system";
const GEN_AI_OPERATION: &str = "gen_ai.operation.name";
const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";
const GEN_AI_RESPONSE_MODEL: &str = "gen_ai.response.model";
const GEN_AI_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
const GEN_AI_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
const GEN_AI_FINISH_REASONS: &str = "gen_ai.response.finish_reasons";
const GEN_AI_REQUEST_TEMP: &str = "gen_ai.request.temperature";
const GEN_AI_REQUEST_MAX_TOKENS: &str = "gen_ai.request.max_tokens";
const GEN_AI_USER_MESSAGE: &str = "gen_ai.user.message";
const GEN_AI_ASSISTANT_MESSAGE: &str = "gen_ai.assistant.message";
const GEN_AI_CHOICE: &str = "gen_ai.choice";
const METRIC_TOKEN_USAGE: &str = "gen_ai.client.token.usage";
const METRIC_OPERATION_DURATION: &str = "gen_ai.client.operation.duration";

/// Configuration for OTel instrumentation behavior.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// Capture request/response content as span events. Default: false (privacy).
    pub capture_content: bool,
    /// Record `gen_ai.client.token.usage` and `gen_ai.client.operation.duration`. Default: true.
    pub record_metrics: bool,
    /// Tracer/meter name passed to `opentelemetry::global`. Default: `"sideseat"`.
    pub tracer_name: &'static str,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            capture_content: false,
            record_metrics: true,
            tracer_name: "sideseat",
        }
    }
}

/// Wraps any [`Provider`] with OpenTelemetry tracing and metrics instrumentation.
///
/// Spans follow the GenAI semantic conventions (experimental). Instruments are created
/// once at construction time.
pub struct InstrumentedProvider<P> {
    inner: Arc<P>,
    config: TelemetryConfig,
    token_counter: Counter<u64>,
    duration_hist: Histogram<f64>,
}

impl<P: ChatProvider + Send + Sync + 'static> InstrumentedProvider<P> {
    pub fn new(inner: P) -> Self {
        Self::with_config(inner, TelemetryConfig::default())
    }

    pub fn with_config(inner: P, config: TelemetryConfig) -> Self {
        let meter = opentelemetry::global::meter(config.tracer_name);
        let token_counter = meter
            .u64_counter(METRIC_TOKEN_USAGE)
            .with_description("Number of input and output tokens used by LLM operations")
            .with_unit("{token}")
            .build();
        let duration_hist = meter
            .f64_histogram(METRIC_OPERATION_DURATION)
            .with_description("Duration of LLM operations in seconds")
            .with_unit("s")
            .build();
        Self {
            inner: Arc::new(inner),
            config,
            token_counter,
            duration_hist,
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn build_span(
    tracer_name: &'static str,
    operation: &'static str,
    system: &'static str,
    model: &str,
    config: Option<&ProviderConfig>,
) -> BoxedSpan {
    let tracer = opentelemetry::global::tracer(tracer_name);
    let builder = tracer.span_builder(operation).with_kind(SpanKind::Client);
    let mut span = tracer.build(builder);
    span.set_attribute(KeyValue::new(GEN_AI_SYSTEM, system));
    span.set_attribute(KeyValue::new(GEN_AI_OPERATION, operation));
    span.set_attribute(KeyValue::new(GEN_AI_REQUEST_MODEL, model.to_owned()));
    if let Some(cfg) = config {
        if let Some(t) = cfg.temperature {
            span.set_attribute(KeyValue::new(GEN_AI_REQUEST_TEMP, t));
        }
        if let Some(m) = cfg.max_tokens {
            span.set_attribute(KeyValue::new(GEN_AI_REQUEST_MAX_TOKENS, m as i64));
        }
    }
    span
}

fn record_metrics(
    counter: &Counter<u64>,
    hist: &Histogram<f64>,
    usage: &Usage,
    operation: &'static str,
    system: &'static str,
    model: &str,
    elapsed_secs: f64,
) {
    let base = [
        KeyValue::new(GEN_AI_SYSTEM, system),
        KeyValue::new(GEN_AI_OPERATION, operation),
        KeyValue::new(GEN_AI_REQUEST_MODEL, model.to_owned()),
    ];
    if usage.input_tokens > 0 {
        counter.add(
            usage.input_tokens,
            &[
                base[0].clone(),
                base[1].clone(),
                base[2].clone(),
                KeyValue::new("gen_ai.token.type", "input"),
            ],
        );
    }
    if usage.output_tokens > 0 {
        counter.add(
            usage.output_tokens,
            &[
                base[0].clone(),
                base[1].clone(),
                base[2].clone(),
                KeyValue::new("gen_ai.token.type", "output"),
            ],
        );
    }
    hist.record(elapsed_secs, &base);
}

fn add_content_events(span: &SpanRef<'_>, messages: &[Message], response: &Response) {
    for msg in messages {
        let event_name = match msg.role {
            Role::User => GEN_AI_USER_MESSAGE,
            _ => GEN_AI_ASSISTANT_MESSAGE,
        };
        let text = msg
            .content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join(" ");
        span.add_event(event_name, vec![KeyValue::new("content", text)]);
    }
    let text = response
        .content
        .iter()
        .filter_map(|b| b.as_text())
        .collect::<Vec<_>>()
        .join("");
    span.add_event(
        GEN_AI_CHOICE,
        vec![
            KeyValue::new("index", 0_i64),
            KeyValue::new(
                "finish_reason",
                format!("{:?}", response.stop_reason).to_lowercase(),
            ),
            KeyValue::new("message.content", text),
        ],
    );
}

// ---------------------------------------------------------------------------
// Provider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl<P: ChatProvider + Send + Sync + 'static> Provider for InstrumentedProvider<P> {
    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.inner.list_models().await
    }
}

#[async_trait]
impl<P: ChatProvider + Send + Sync + 'static> ChatProvider for InstrumentedProvider<P> {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let tracer_name = self.config.tracer_name;
        let system = self.inner.provider_name();
        let model = config.model.clone();

        // Create span before the generator so timing starts when stream() is called.
        // Wrap in a Context so child spans (e.g. HTTP) nest under this span automatically.
        let cx = Context::current_with_span(build_span(
            tracer_name,
            "chat",
            system,
            &model,
            Some(&config),
        ));

        let inner = Arc::clone(&self.inner);
        let telemetry_config = self.config.clone();
        let token_counter = self.token_counter.clone();
        let duration_hist = self.duration_hist.clone();
        let started = Instant::now();

        Box::pin(async_stream::try_stream! {
            let mut accumulated_usage = Usage::default();
            let mut span_ended = false;

            // .with_context() attaches cx on each poll so inner spans are parented correctly.
            // cx itself (Send + 'static) is captured; SpanRef is only borrowed temporarily.
            let inner_stream = inner.stream(messages, config).with_context(cx.clone());
            futures::pin_mut!(inner_stream);

            while let Some(result) = inner_stream.next().await {
                match result {
                    Err(e) => {
                        if !span_ended {
                            // Temporary SpanRef — not held across yield.
                            let span = cx.span();
                            span.record_error(&e);
                            span.set_status(Status::Error {
                                description: e.to_string().into(),
                            });
                            span.end();
                            span_ended = true;
                        }
                        Err(e)?;
                    }
                    Ok(event) => {
                        match &event {
                            StreamEvent::Metadata { usage, model: resp_model, .. } => {
                                accumulated_usage += usage.clone();
                                // Temporary SpanRef — dropped before `yield event` below.
                                let span = cx.span();
                                span.set_attribute(KeyValue::new(
                                    GEN_AI_INPUT_TOKENS,
                                    accumulated_usage.input_tokens.min(i64::MAX as u64) as i64,
                                ));
                                span.set_attribute(KeyValue::new(
                                    GEN_AI_OUTPUT_TOKENS,
                                    accumulated_usage.output_tokens.min(i64::MAX as u64) as i64,
                                ));
                                if let Some(m) = resp_model {
                                    span.set_attribute(KeyValue::new(
                                        GEN_AI_RESPONSE_MODEL,
                                        m.clone(),
                                    ));
                                }
                            }
                            StreamEvent::MessageStop { stop_reason } => {
                                // Temporary SpanRef — dropped before `yield event` below.
                                let span = cx.span();
                                span.set_attribute(KeyValue::new(
                                    GEN_AI_FINISH_REASONS,
                                    Value::Array(Array::String(vec![
                                        format!("{stop_reason:?}").to_lowercase().into(),
                                    ])),
                                ));
                                span.set_status(Status::Ok);
                                span.end();
                                span_ended = true;
                                if telemetry_config.record_metrics {
                                    record_metrics(
                                        &token_counter,
                                        &duration_hist,
                                        &accumulated_usage,
                                        "chat",
                                        system,
                                        &model,
                                        started.elapsed().as_secs_f64(),
                                    );
                                }
                            }
                            _ => {}
                        }
                        yield event;
                    }
                }
            }

            if !span_ended {
                cx.span().end();
                if telemetry_config.record_metrics {
                    record_metrics(
                        &token_counter,
                        &duration_hist,
                        &accumulated_usage,
                        "chat",
                        system,
                        &model,
                        started.elapsed().as_secs_f64(),
                    );
                }
            }
        })
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        let started = Instant::now();
        // Wrap span in a Context so child spans (e.g. HTTP) nest under this span automatically.
        let cx = Context::current_with_span(build_span(
            self.config.tracer_name,
            "chat",
            self.inner.provider_name(),
            &config.model,
            Some(&config),
        ));

        // .with_context() attaches cx on each poll — Send-safe (ContextGuard not held across await).
        let result = self
            .inner
            .complete(messages.clone(), config.clone())
            .with_context(cx.clone())
            .await;
        let elapsed = started.elapsed().as_secs_f64();

        match result {
            Ok(resp) => {
                // cx.span() returns a SpanRef<'_> with interior mutability — no await after this.
                let span = cx.span();
                if let Some(m) = &resp.model {
                    span.set_attribute(KeyValue::new(GEN_AI_RESPONSE_MODEL, m.clone()));
                }
                span.set_attribute(KeyValue::new(
                    GEN_AI_INPUT_TOKENS,
                    resp.usage.input_tokens.min(i64::MAX as u64) as i64,
                ));
                span.set_attribute(KeyValue::new(
                    GEN_AI_OUTPUT_TOKENS,
                    resp.usage.output_tokens.min(i64::MAX as u64) as i64,
                ));
                span.set_attribute(KeyValue::new(
                    GEN_AI_FINISH_REASONS,
                    Value::Array(Array::String(vec![
                        format!("{:?}", resp.stop_reason).to_lowercase().into(),
                    ])),
                ));
                if self.config.capture_content {
                    add_content_events(&span, &messages, &resp);
                }
                span.set_status(Status::Ok);
                span.end();
                if self.config.record_metrics {
                    record_metrics(
                        &self.token_counter,
                        &self.duration_hist,
                        &resp.usage,
                        "chat",
                        self.inner.provider_name(),
                        &config.model,
                        elapsed,
                    );
                }
                Ok(resp)
            }
            Err(e) => {
                let span = cx.span();
                span.record_error(&e);
                span.set_status(Status::Error {
                    description: e.to_string().into(),
                });
                span.end();
                Err(e)
            }
        }
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
impl<P: ChatProvider + EmbeddingProvider + Send + Sync + 'static> EmbeddingProvider
    for InstrumentedProvider<P>
{
    async fn embed(
        &self,
        request: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ProviderError> {
        let started = Instant::now();
        let model = request.model.clone();
        let mut span = build_span(
            self.config.tracer_name,
            "embeddings",
            self.inner.provider_name(),
            &model,
            None,
        );

        let result = self.inner.embed(request).await;
        let elapsed = started.elapsed().as_secs_f64();

        match result {
            Ok(resp) => {
                span.set_status(Status::Ok);
                span.end();
                if self.config.record_metrics {
                    self.duration_hist.record(
                        elapsed,
                        &[
                            KeyValue::new(GEN_AI_SYSTEM, self.inner.provider_name()),
                            KeyValue::new(GEN_AI_OPERATION, "embeddings"),
                            KeyValue::new(GEN_AI_REQUEST_MODEL, model),
                        ],
                    );
                }
                Ok(resp)
            }
            Err(e) => {
                span.record_error(&e);
                span.set_status(Status::Error {
                    description: e.to_string().into(),
                });
                span.end();
                Err(e)
            }
        }
    }
}

#[async_trait]
impl<P: ChatProvider + ImageProvider + Send + Sync + 'static> ImageProvider
    for InstrumentedProvider<P>
{
    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        let started = Instant::now();
        let model = request.model.clone();
        let mut span = build_span(
            self.config.tracer_name,
            "image_generation",
            self.inner.provider_name(),
            &model,
            None,
        );

        let result = self.inner.generate_image(request).await;
        let elapsed = started.elapsed().as_secs_f64();

        match result {
            Ok(resp) => {
                span.set_status(Status::Ok);
                span.end();
                if self.config.record_metrics {
                    self.duration_hist.record(
                        elapsed,
                        &[
                            KeyValue::new(GEN_AI_SYSTEM, self.inner.provider_name()),
                            KeyValue::new(GEN_AI_OPERATION, "image_generation"),
                            KeyValue::new(GEN_AI_REQUEST_MODEL, model),
                        ],
                    );
                }
                Ok(resp)
            }
            Err(e) => {
                span.record_error(&e);
                span.set_status(Status::Error {
                    description: e.to_string().into(),
                });
                span.end();
                Err(e)
            }
        }
    }

    async fn edit_image(
        &self,
        request: ImageEditRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        let started = Instant::now();
        let model = request.model.clone();
        let mut span = build_span(
            self.config.tracer_name,
            "image_edit",
            self.inner.provider_name(),
            &model,
            None,
        );

        let result = self.inner.edit_image(request).await;
        let elapsed = started.elapsed().as_secs_f64();

        match result {
            Ok(resp) => {
                span.set_status(Status::Ok);
                span.end();
                if self.config.record_metrics {
                    self.duration_hist.record(
                        elapsed,
                        &[
                            KeyValue::new(GEN_AI_SYSTEM, self.inner.provider_name()),
                            KeyValue::new(GEN_AI_OPERATION, "image_edit"),
                            KeyValue::new(GEN_AI_REQUEST_MODEL, model),
                        ],
                    );
                }
                Ok(resp)
            }
            Err(e) => {
                span.record_error(&e);
                span.set_status(Status::Error {
                    description: e.to_string().into(),
                });
                span.end();
                Err(e)
            }
        }
    }
}

#[async_trait]
impl<P: ChatProvider + VideoProvider + Send + Sync + 'static> VideoProvider
    for InstrumentedProvider<P>
{
    async fn generate_video(
        &self,
        request: VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        let started = Instant::now();
        let model = request.model.clone();
        let mut span = build_span(
            self.config.tracer_name,
            "video_generation",
            self.inner.provider_name(),
            &model,
            None,
        );

        let result = self.inner.generate_video(request).await;
        let elapsed = started.elapsed().as_secs_f64();

        match result {
            Ok(resp) => {
                span.set_status(Status::Ok);
                span.end();
                if self.config.record_metrics {
                    self.duration_hist.record(
                        elapsed,
                        &[
                            KeyValue::new(GEN_AI_SYSTEM, self.inner.provider_name()),
                            KeyValue::new(GEN_AI_OPERATION, "video_generation"),
                            KeyValue::new(GEN_AI_REQUEST_MODEL, model),
                        ],
                    );
                }
                Ok(resp)
            }
            Err(e) => {
                span.record_error(&e);
                span.set_status(Status::Error {
                    description: e.to_string().into(),
                });
                span.end();
                Err(e)
            }
        }
    }
}

#[async_trait]
impl<P: ChatProvider + AudioProvider + Send + Sync + 'static> AudioProvider
    for InstrumentedProvider<P>
{
    async fn generate_speech(
        &self,
        request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        let started = Instant::now();
        let model = request.model.clone();
        let mut span = build_span(
            self.config.tracer_name,
            "speech",
            self.inner.provider_name(),
            &model,
            None,
        );

        let result = self.inner.generate_speech(request).await;
        let elapsed = started.elapsed().as_secs_f64();

        match result {
            Ok(resp) => {
                span.set_status(Status::Ok);
                span.end();
                if self.config.record_metrics {
                    self.duration_hist.record(
                        elapsed,
                        &[
                            KeyValue::new(GEN_AI_SYSTEM, self.inner.provider_name()),
                            KeyValue::new(GEN_AI_OPERATION, "speech"),
                            KeyValue::new(GEN_AI_REQUEST_MODEL, model),
                        ],
                    );
                }
                Ok(resp)
            }
            Err(e) => {
                span.record_error(&e);
                span.set_status(Status::Error {
                    description: e.to_string().into(),
                });
                span.end();
                Err(e)
            }
        }
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        let started = Instant::now();
        let model = request.model.clone();
        let mut span = build_span(
            self.config.tracer_name,
            "transcription",
            self.inner.provider_name(),
            &model,
            None,
        );

        let result = self.inner.transcribe(request).await;
        let elapsed = started.elapsed().as_secs_f64();

        match result {
            Ok(resp) => {
                span.set_status(Status::Ok);
                span.end();
                if self.config.record_metrics {
                    self.duration_hist.record(
                        elapsed,
                        &[
                            KeyValue::new(GEN_AI_SYSTEM, self.inner.provider_name()),
                            KeyValue::new(GEN_AI_OPERATION, "transcription"),
                            KeyValue::new(GEN_AI_REQUEST_MODEL, model),
                        ],
                    );
                }
                Ok(resp)
            }
            Err(e) => {
                span.record_error(&e);
                span.set_status(Status::Error {
                    description: e.to_string().into(),
                });
                span.end();
                Err(e)
            }
        }
    }

    async fn translate(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        let started = Instant::now();
        let model = request.model.clone();
        let mut span = build_span(
            self.config.tracer_name,
            "translation",
            self.inner.provider_name(),
            &model,
            None,
        );

        let result = self.inner.translate(request).await;
        let elapsed = started.elapsed().as_secs_f64();

        match result {
            Ok(resp) => {
                span.set_status(Status::Ok);
                span.end();
                if self.config.record_metrics {
                    self.duration_hist.record(
                        elapsed,
                        &[
                            KeyValue::new(GEN_AI_SYSTEM, self.inner.provider_name()),
                            KeyValue::new(GEN_AI_OPERATION, "translation"),
                            KeyValue::new(GEN_AI_REQUEST_MODEL, model),
                        ],
                    );
                }
                Ok(resp)
            }
            Err(e) => {
                span.record_error(&e);
                span.set_status(Status::Error {
                    description: e.to_string().into(),
                });
                span.end();
                Err(e)
            }
        }
    }
}

#[async_trait]
impl<P: ChatProvider + ModerationProvider + Send + Sync + 'static> ModerationProvider
    for InstrumentedProvider<P>
{
    async fn moderate(
        &self,
        request: ModerationRequest,
    ) -> Result<ModerationResponse, ProviderError> {
        let started = Instant::now();
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| "omni-moderation-latest".to_string());
        let mut span = build_span(
            self.config.tracer_name,
            "moderation",
            self.inner.provider_name(),
            &model,
            None,
        );

        let result = self.inner.moderate(request).await;
        let elapsed = started.elapsed().as_secs_f64();

        match result {
            Ok(resp) => {
                span.set_status(Status::Ok);
                span.end();
                if self.config.record_metrics {
                    self.duration_hist.record(
                        elapsed,
                        &[
                            KeyValue::new(GEN_AI_SYSTEM, self.inner.provider_name()),
                            KeyValue::new(GEN_AI_OPERATION, "moderation"),
                            KeyValue::new(GEN_AI_REQUEST_MODEL, model),
                        ],
                    );
                }
                Ok(resp)
            }
            Err(e) => {
                span.record_error(&e);
                span.set_status(Status::Error {
                    description: e.to_string().into(),
                });
                span.end();
                Err(e)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SideSeat — initializes global OTel pipeline pointing at a SideSeat server
// ---------------------------------------------------------------------------

/// Builder that configures and installs the global OTel pipeline for a SideSeat server.
///
/// # Example
///
/// ```no_run
/// use sideseat::telemetry::SideSeat;
///
/// let _guard = SideSeat::new()
///     .with_project_id("my-project")
///     .init()
///     .expect("OTel init failed");
/// // _guard must stay alive; dropping it flushes and shuts down OTel.
/// ```
pub struct SideSeat {
    pub endpoint: String,
    pub project_id: String,
    pub api_key: Option<String>,
    pub capture_content: bool,
}

impl SideSeat {
    /// Creates a new builder, reading defaults from environment variables:
    /// `SIDESEAT_ENDPOINT` (default: `http://localhost:5388`) and
    /// `SIDESEAT_PROJECT_ID` (default: `default`).
    pub fn new() -> Self {
        Self {
            endpoint: crate::env::optional_or(crate::env::keys::SIDESEAT_ENDPOINT, "http://localhost:5388"),
            project_id: crate::env::optional_or(crate::env::keys::SIDESEAT_PROJECT_ID, "default"),
            api_key: crate::env::optional(crate::env::keys::SIDESEAT_API_KEY),
            capture_content: false,
        }
    }

    pub fn with_endpoint(mut self, e: impl Into<String>) -> Self {
        self.endpoint = e.into();
        self
    }

    pub fn with_project_id(mut self, id: impl Into<String>) -> Self {
        self.project_id = id.into();
        self
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn with_capture_content(mut self, v: bool) -> Self {
        self.capture_content = v;
        self
    }

    /// Build a [`TelemetryConfig`] reflecting this builder's settings.
    ///
    /// Call this before [`init()`](Self::init) to obtain a config suitable for
    /// [`InstrumentedProvider::with_config`].
    ///
    /// ```
    /// use sideseat::telemetry::{SideSeat, InstrumentedProvider};
    /// use sideseat::mock::MockProvider;
    ///
    /// let ss = SideSeat::new().with_capture_content(true);
    /// let config = ss.telemetry_config();
    /// assert!(config.capture_content);
    /// let _provider = InstrumentedProvider::with_config(MockProvider::new(), config);
    /// ```
    pub fn telemetry_config(&self) -> TelemetryConfig {
        TelemetryConfig {
            capture_content: self.capture_content,
            ..TelemetryConfig::default()
        }
    }

    /// Installs the global OTel `TracerProvider` and `MeterProvider`.
    ///
    /// Returns a [`SideSeatGuard`] that shuts down the providers on drop, flushing
    /// any remaining telemetry. Keep it alive for the duration of your program.
    pub fn init(self) -> Result<SideSeatGuard, ProviderError> {
        use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
        use opentelemetry_sdk::{
            Resource,
            metrics::{PeriodicReader, SdkMeterProvider},
            trace::{BatchSpanProcessor, SdkTracerProvider},
        };

        let traces_url = format!("{}/otel/{}/v1/traces", self.endpoint, self.project_id);
        let metrics_url = format!("{}/otel/{}/v1/metrics", self.endpoint, self.project_id);

        let mut headers = HashMap::new();
        if let Some(key) = &self.api_key {
            headers.insert("Authorization".to_string(), format!("Bearer {key}"));
        }

        let resource = Resource::builder_empty()
            .with_attribute(KeyValue::new("service.name", "sideseat-sdk"))
            .build();

        // Trace exporter
        let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(traces_url)
            .with_headers(headers.clone())
            .build()
            .map_err(|e| ProviderError::Config(format!("OTLP trace exporter: {e}")))?;

        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_span_processor(BatchSpanProcessor::builder(trace_exporter).build())
            .build();

        // Metrics exporter
        let metrics_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_endpoint(metrics_url)
            .with_headers(headers)
            .build()
            .map_err(|e| ProviderError::Config(format!("OTLP metrics exporter: {e}")))?;

        let meter_provider = SdkMeterProvider::builder()
            .with_resource(resource)
            .with_reader(
                PeriodicReader::builder(metrics_exporter)
                    .with_interval(std::time::Duration::from_secs(60))
                    .build(),
            )
            .build();

        opentelemetry::global::set_tracer_provider(tracer_provider.clone());
        opentelemetry::global::set_meter_provider(meter_provider.clone());

        Ok(SideSeatGuard {
            tracer_provider,
            meter_provider,
        })
    }
}

impl Default for SideSeat {
    fn default() -> Self {
        Self::new()
    }
}

/// Shuts down the OTel providers on drop, flushing remaining telemetry.
pub struct SideSeatGuard {
    tracer_provider: opentelemetry_sdk::trace::SdkTracerProvider,
    meter_provider: opentelemetry_sdk::metrics::SdkMeterProvider,
}

impl Drop for SideSeatGuard {
    fn drop(&mut self) {
        if let Err(e) = self.tracer_provider.shutdown() {
            tracing::warn!("OTel tracer provider shutdown failed: {e}");
        }
        if let Err(e) = self.meter_provider.shutdown() {
            tracing::warn!("OTel meter provider shutdown failed: {e}");
        }
    }
}
