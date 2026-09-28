use super::*;

#[derive(Debug, Clone, Default)]
struct CorrelationContext {
    session_id: Option<String>,
    user_id: Option<String>,
}

/// Options shared by [`SideSeatGuard::trace`] and [`SideSeatGuard::span`].
#[derive(Debug, Clone)]
pub struct SideSeatSpanOptions {
    pub kind: SpanKind,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub attributes: Vec<KeyValue>,
}

impl SideSeatSpanOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_kind(mut self, kind: SpanKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    pub fn with_user_id(mut self, id: impl Into<String>) -> Self {
        self.user_id = Some(id.into());
        self
    }

    pub fn with_attribute(mut self, attribute: KeyValue) -> Self {
        self.attributes.push(attribute);
        self
    }

    pub fn with_attributes(mut self, attributes: impl IntoIterator<Item = KeyValue>) -> Self {
        self.attributes.extend(attributes);
        self
    }
}

impl Default for SideSeatSpanOptions {
    fn default() -> Self {
        Self {
            kind: SpanKind::Internal,
            session_id: None,
            user_id: None,
            attributes: Vec::new(),
        }
    }
}

struct SideSeatSpanState {
    context: Context,
    ended: AtomicBool,
}

impl SideSeatSpanState {
    fn end(&self) {
        if !self.ended.swap(true, Ordering::AcqRel) {
            self.context.span().end();
        }
    }
}

impl Drop for SideSeatSpanState {
    fn drop(&mut self) {
        self.end();
    }
}

/// A recording span created by the SideSeat SDK.
///
/// Clones refer to the same span. The span ends on an explicit [`end`](Self::end)
/// call or when the final clone is dropped.
#[derive(Clone)]
pub struct SideSeatSpan {
    state: Arc<SideSeatSpanState>,
}

impl SideSeatSpan {
    fn new(context: Context) -> Self {
        Self {
            state: Arc::new(SideSeatSpanState {
                context,
                ended: AtomicBool::new(false),
            }),
        }
    }

    pub fn set_attribute(&self, attribute: KeyValue) {
        self.state.context.span().set_attribute(attribute);
    }

    pub fn set_attributes(&self, attributes: impl IntoIterator<Item = KeyValue>) {
        self.state.context.span().set_attributes(attributes);
    }

    pub fn add_event(
        &self,
        name: impl Into<std::borrow::Cow<'static, str>>,
        attributes: Vec<KeyValue>,
    ) {
        self.state.context.span().add_event(name, attributes);
    }

    pub fn set_status(&self, status: Status) {
        self.state.context.span().set_status(status);
    }

    pub fn context(&self) -> Context {
        self.state.context.clone()
    }

    pub fn end(&self) {
        self.state.end();
    }
}

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
    endpoint: String,
    project_id: String,
    api_key: Option<String>,
    capture_content: bool,
    service_name: String,
    service_version: Option<String>,
    framework: Option<String>,
}

impl SideSeat {
    /// Creates a new builder, reading defaults from environment variables:
    /// `SIDESEAT_ENDPOINT` (default: `http://localhost:5388`) and
    /// `SIDESEAT_PROJECT_ID` (default: `default`).
    pub fn new() -> Self {
        Self::from_environment(crate::env::optional)
    }

    fn from_environment(mut read: impl FnMut(&str) -> Option<String>) -> Self {
        Self {
            endpoint: read(crate::env::keys::SIDESEAT_ENDPOINT)
                .unwrap_or_else(|| "http://localhost:5388".to_string()),
            project_id: read(crate::env::keys::SIDESEAT_PROJECT_ID)
                .unwrap_or_else(|| "default".to_string()),
            api_key: read(crate::env::keys::SIDESEAT_API_KEY),
            capture_content: false,
            service_name: read("OTEL_SERVICE_NAME").unwrap_or_else(|| "sideseat-rust".to_string()),
            service_version: None,
            framework: None,
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

    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    pub fn with_service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    pub fn with_framework(mut self, framework: impl Into<String>) -> Self {
        self.framework = Some(framework.into());
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub fn captures_content(&self) -> bool {
        self.capture_content
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub fn service_version(&self) -> Option<&str> {
        self.service_version.as_deref()
    }

    pub fn framework(&self) -> Option<&str> {
        self.framework.as_deref()
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
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
        use opentelemetry_sdk::{
            Resource,
            metrics::{PeriodicReader, SdkMeterProvider},
            trace::{BatchSpanProcessor, SdkTracerProvider},
        };

        let traces_url = signal_endpoint(&self.endpoint, &self.project_id, "traces")?;
        let metrics_url = signal_endpoint(&self.endpoint, &self.project_id, "metrics")?;

        let mut headers = HashMap::new();
        if let Some(key) = &self.api_key {
            headers.insert("Authorization".to_string(), format!("Bearer {key}"));
        }

        let mut resource_attributes = vec![
            KeyValue::new("service.name", self.service_name),
            KeyValue::new("telemetry.sdk.name", "sideseat"),
            KeyValue::new("telemetry.sdk.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new("telemetry.sdk.language", "rust"),
        ];
        if let Some(service_version) = self.service_version {
            resource_attributes.push(KeyValue::new("service.version", service_version));
        }
        if let Some(framework) = self.framework {
            resource_attributes.push(KeyValue::new("sideseat.framework", framework));
        }
        let resource = Resource::builder_empty()
            .with_attributes(resource_attributes)
            .build();

        // Trace exporter
        let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(traces_url)
            .with_headers(headers.clone())
            .build()
            .map_err(|e| ProviderError::MissingConfig(format!("OTLP trace exporter: {e}")))?;

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
            .map_err(|e| ProviderError::MissingConfig(format!("OTLP metrics exporter: {e}")))?;

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
            tracer: tracer_provider.tracer("sideseat"),
            tracer_provider,
            meter_provider,
            shutdown_result: Mutex::new(None),
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
    tracer: opentelemetry_sdk::trace::SdkTracer,
    tracer_provider: opentelemetry_sdk::trace::SdkTracerProvider,
    meter_provider: opentelemetry_sdk::metrics::SdkMeterProvider,
    shutdown_result: Mutex<Option<Result<(), ProviderError>>>,
}

impl SideSeatGuard {
    /// Run asynchronous work in a new root trace.
    ///
    /// The trace is detached from the ambient OpenTelemetry context. Session and
    /// user correlation propagate to nested [`span`](Self::span) calls.
    pub async fn trace<T, E, F, Fut>(
        &self,
        name: impl Into<std::borrow::Cow<'static, str>>,
        options: SideSeatSpanOptions,
        work: F,
    ) -> Result<T, E>
    where
        E: std::error::Error,
        F: FnOnce(SideSeatSpan) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let span = self.start_trace(name, options);
        self.run(span, work).await
    }

    /// Run asynchronous work in a child of the current OpenTelemetry span.
    ///
    /// If there is no current span this starts a root span. Correlation inherited
    /// from an enclosing SideSeat trace/span is applied automatically.
    pub async fn span<T, E, F, Fut>(
        &self,
        name: impl Into<std::borrow::Cow<'static, str>>,
        options: SideSeatSpanOptions,
        work: F,
    ) -> Result<T, E>
    where
        E: std::error::Error,
        F: FnOnce(SideSeatSpan) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let span = self.start_span(name, options);
        self.run(span, work).await
    }

    /// Start a new root trace for manual lifecycle management.
    pub fn start_trace(
        &self,
        name: impl Into<std::borrow::Cow<'static, str>>,
        options: SideSeatSpanOptions,
    ) -> SideSeatSpan {
        self.start(name, Context::new(), options)
    }

    /// Start a child of the current span for manual lifecycle management.
    pub fn start_span(
        &self,
        name: impl Into<std::borrow::Cow<'static, str>>,
        options: SideSeatSpanOptions,
    ) -> SideSeatSpan {
        self.start(name, Context::current(), options)
    }

    fn start(
        &self,
        name: impl Into<std::borrow::Cow<'static, str>>,
        parent: Context,
        options: SideSeatSpanOptions,
    ) -> SideSeatSpan {
        let inherited = parent
            .get::<CorrelationContext>()
            .cloned()
            .unwrap_or_default();
        let correlation = CorrelationContext {
            session_id: options.session_id.or(inherited.session_id),
            user_id: options.user_id.or(inherited.user_id),
        };
        let mut attributes = options.attributes;
        if let Some(session_id) = &correlation.session_id {
            attributes.push(KeyValue::new("session.id", session_id.clone()));
        }
        if let Some(user_id) = &correlation.user_id {
            attributes.push(KeyValue::new("user.id", user_id.clone()));
        }

        let span = self
            .tracer
            .span_builder(name)
            .with_kind(options.kind)
            .with_attributes(attributes)
            .start_with_context(&self.tracer, &parent);
        SideSeatSpan::new(parent.with_value(correlation).with_span(span))
    }

    async fn run<T, E, F, Fut>(&self, span: SideSeatSpan, work: F) -> Result<T, E>
    where
        E: std::error::Error,
        F: FnOnce(SideSeatSpan) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let result = work(span.clone()).with_context(span.context()).await;
        match &result {
            Ok(_) => span.set_status(Status::Ok),
            Err(error) => {
                span.state.context.span().record_error(error);
                span.set_status(Status::Error {
                    description: error.to_string().into(),
                });
            }
        }
        span.end();
        result
    }

    /// Export all queued traces and metrics.
    pub fn force_flush(&self) -> Result<(), ProviderError> {
        telemetry_results(
            "flush",
            self.tracer_provider.force_flush(),
            self.meter_provider.force_flush(),
        )
    }

    /// Flush and shut down the installed OpenTelemetry providers.
    ///
    /// Calling this more than once is safe.
    pub fn shutdown(&self) -> Result<(), ProviderError> {
        let mut stored = self
            .shutdown_result
            .lock()
            .map_err(|_| ProviderError::Telemetry("shutdown state lock poisoned".to_string()))?;
        if let Some(result) = stored.as_ref() {
            return result.clone();
        }

        let result = telemetry_results(
            "shutdown",
            self.tracer_provider.shutdown(),
            self.meter_provider.shutdown(),
        );
        *stored = Some(result.clone());
        result
    }
}

impl Drop for SideSeatGuard {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            tracing::warn!("{error}");
        }
    }
}

fn telemetry_results(
    operation: &'static str,
    trace: opentelemetry_sdk::error::OTelSdkResult,
    metrics: opentelemetry_sdk::error::OTelSdkResult,
) -> Result<(), ProviderError> {
    match (trace, metrics) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(trace), Ok(())) => Err(ProviderError::Telemetry(format!(
            "trace {operation} failed: {trace}"
        ))),
        (Ok(()), Err(metrics)) => Err(ProviderError::Telemetry(format!(
            "metric {operation} failed: {metrics}"
        ))),
        (Err(trace), Err(metrics)) => Err(ProviderError::Telemetry(format!(
            "trace {operation} failed: {trace}; metric {operation} failed: {metrics}"
        ))),
    }
}

fn signal_endpoint(
    endpoint: &str,
    project_id: &str,
    signal: &'static str,
) -> Result<String, ProviderError> {
    let mut url = reqwest::Url::parse(endpoint).map_err(|error| {
        ProviderError::MissingConfig(format!("invalid SIDESEAT_ENDPOINT {endpoint:?}: {error}"))
    })?;
    let path = url.path().trim_end_matches('/');
    let signal_suffix = format!("/v1/{signal}");
    let path = if path.ends_with("/v1/traces") {
        format!("{}{signal_suffix}", path.trim_end_matches("/v1/traces"))
    } else if path.ends_with("/v1/metrics") {
        format!("{}{signal_suffix}", path.trim_end_matches("/v1/metrics"))
    } else if path.ends_with("/v1") {
        format!("{path}/{signal}")
    } else if path.is_empty() {
        format!("/otel/{project_id}/v1/{signal}")
    } else {
        format!("{path}/v1/{signal}")
    };
    url.set_path(&path);
    Ok(url.to_string())
}

#[cfg(test)]
#[path = "../telemetry_tests.rs"]
mod tests;
