//! The pipeline: one tracer, logger, and meter provider exporting over OTLP/HTTP, installed as the
//! process's global providers.

use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError, Weak, mpsc};
use std::time::Duration;

use opentelemetry::trace::{FutureExt as _, SpanKind, Status, TraceContextExt as _, Tracer as _};
use opentelemetry::{Context, KeyValue, global};
use opentelemetry_otlp::{WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{BatchSpanProcessor, SdkTracer, SdkTracerProvider, SpanProcessor};

use crate::config::Settings;
use crate::correlation::{Correlation, CorrelationProcessor};
use crate::{Error, Options, Session};

const TRACER_NAME: &str = "sideseat";

/// Options for a span started by [`SideSeat::trace`] or [`SideSeat::span`].
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct SpanOptions {
    kind: Option<SpanKind>,
    attributes: Vec<KeyValue>,
    session: Option<Session>,
}

impl SpanOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn kind(mut self, kind: SpanKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn attribute(mut self, attribute: KeyValue) -> Self {
        self.attributes.push(attribute);
        self
    }

    pub fn attributes(mut self, attributes: impl IntoIterator<Item = KeyValue>) -> Self {
        self.attributes.extend(attributes);
        self
    }

    /// The session the span and its descendants belong to.
    pub fn session(mut self, session: Session) -> Self {
        self.session = Some(session);
        self
    }
}

/// A configured pipeline. Clones share it, and compare equal.
///
/// The pipeline shuts down when [`shutdown`](Self::shutdown) is called or when the last clone
/// drops, so keep the client for as long as the program records telemetry.
#[derive(Clone)]
#[must_use = "the pipeline shuts down when the last clone of the client drops"]
pub struct SideSeat {
    inner: Arc<Inner>,
}

struct Inner {
    settings: Settings,
    tracer: Option<SdkTracer>,
    providers: Option<Providers>,
    shutdown: Mutex<Option<bool>>,
}

#[derive(Clone)]
struct Providers {
    tracer: SdkTracerProvider,
    logger: Option<SdkLoggerProvider>,
    meter: Option<SdkMeterProvider>,
}

impl PartialEq for SideSeat {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for SideSeat {}

impl fmt::Debug for SideSeat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SideSeat")
            .field("settings", &self.inner.settings)
            .finish_non_exhaustive()
    }
}

/// The running pipeline, held weakly: a strong reference here would outlive every clone the
/// application holds - statics are never dropped - so the pipeline would never shut down on drop.
static CLIENT: Mutex<Weak<Inner>> = Mutex::new(Weak::new());

/// Configures telemetry for this process and returns the client.
///
/// Calling it again with the same settings returns the same client. Calling it with different
/// settings, or with span processors while a client runs, is [`Error::AlreadyInitialized`]: the
/// providers are global, so a silent second configuration would leave the process exporting with
/// whichever won. After [`shutdown`](SideSeat::shutdown), or once every clone of the client has
/// dropped, `init` configures a new pipeline.
pub fn init(options: Options) -> Result<SideSeat, Error> {
    let (settings, processors) = options.resolve()?;
    let mut current = CLIENT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(client) = running(&current) {
        return if client.inner.settings == settings && processors.is_empty() {
            Ok(client)
        } else {
            Err(Error::AlreadyInitialized)
        };
    }
    let client = SideSeat::start(settings, processors)?;
    *current = Arc::downgrade(&client.inner);
    Ok(client)
}

/// The client [`init`] created, if it is still running.
pub fn client() -> Option<SideSeat> {
    running(&CLIENT.lock().unwrap_or_else(PoisonError::into_inner))
}

fn running(current: &Weak<Inner>) -> Option<SideSeat> {
    current
        .upgrade()
        .map(|inner| SideSeat { inner })
        .filter(|client| !client.is_shut_down())
}

impl SideSeat {
    fn start(settings: Settings, extra: Vec<Box<dyn SpanProcessor>>) -> Result<Self, Error> {
        if settings.debug {
            eprintln!("[sideseat] {settings:?}");
        }
        if settings.disabled {
            return Ok(Self::new(settings, None));
        }

        let resource = resource(&settings);
        let mut tracer_builder = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_span_processor(CorrelationProcessor);
        for processor in extra {
            tracer_builder = tracer_builder.with_span_processor(BoxedProcessor(processor));
        }
        let mut logger = None;
        let mut meter = None;
        let headers = settings.export_headers();
        if settings.export {
            let traces = opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_endpoint(settings.signal_endpoint("traces"))
                .with_headers(headers.clone())
                .build()
                .map_err(|e| exporter_error("trace", &e))?;
            tracer_builder =
                tracer_builder.with_span_processor(BatchSpanProcessor::builder(traces).build());
        }
        if settings.export && settings.metrics {
            let metrics = opentelemetry_otlp::MetricExporter::builder()
                .with_http()
                .with_endpoint(settings.signal_endpoint("metrics"))
                .with_headers(headers.clone())
                .build()
                .map_err(|e| exporter_error("metric", &e))?;
            meter = Some(
                SdkMeterProvider::builder()
                    .with_resource(resource.clone())
                    .with_reader(PeriodicReader::builder(metrics).build())
                    .build(),
            );
        }
        if settings.logs {
            // Built even without export, as in every SDK, so an appender can be attached in tests.
            let mut builder = SdkLoggerProvider::builder().with_resource(resource);
            if settings.export {
                let logs = opentelemetry_otlp::LogExporter::builder()
                    .with_http()
                    .with_endpoint(settings.signal_endpoint("logs"))
                    .with_headers(headers)
                    .build()
                    .map_err(|e| exporter_error("log", &e))?;
                builder = builder.with_batch_exporter(logs);
            }
            logger = Some(builder.build());
        }

        let tracer = tracer_builder.build();
        global::set_tracer_provider(tracer.clone());
        if let Some(meter) = &meter {
            global::set_meter_provider(meter.clone());
        }
        Ok(Self::new(
            settings,
            Some(Providers {
                tracer,
                logger,
                meter,
            }),
        ))
    }

    fn new(settings: Settings, providers: Option<Providers>) -> Self {
        use opentelemetry::trace::TracerProvider as _;
        let tracer = providers.as_ref().map(|p| {
            p.tracer.tracer_with_scope(
                opentelemetry::InstrumentationScope::builder(TRACER_NAME)
                    .with_version(crate::VERSION)
                    .build(),
            )
        });
        Self {
            inner: Arc::new(Inner {
                settings,
                tracer,
                providers,
                shutdown: Mutex::new(None),
            }),
        }
    }

    /// Whether prompts, responses, and tool payloads should be recorded. Instrumentation written
    /// against this crate reads it.
    pub fn captures_content(&self) -> bool {
        self.inner.settings.capture_content
    }

    /// The tracer provider, for libraries that take one explicitly. `None` when disabled.
    pub fn tracer_provider(&self) -> Option<&SdkTracerProvider> {
        self.inner.providers.as_ref().map(|p| &p.tracer)
    }

    /// The logger provider, for a log appender such as `opentelemetry-appender-tracing`. OpenTelemetry
    /// Rust has no global logger provider, so log records reach SideSeat only through an appender
    /// built on this one. `None` when disabled or when logs are off; without export it records
    /// nothing.
    pub fn logger_provider(&self) -> Option<&SdkLoggerProvider> {
        self.inner.providers.as_ref()?.logger.as_ref()
    }

    /// Runs `work` in a new root span, even when another span is active. The current session is
    /// kept unless `options` names another.
    pub async fn trace<T, E, F, Fut>(
        &self,
        name: impl Into<Cow<'static, str>>,
        options: SpanOptions,
        work: F,
    ) -> Result<T, E>
    where
        E: fmt::Display,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let current = Context::current();
        let correlation = options
            .session
            .as_ref()
            .map_or_else(Correlation::default, |session| {
                session.correlation().clone()
            })
            .apply(&current);
        // A parent context without a span makes a root; the correlation still reaches the processor.
        let parent = Context::new().with_value(Correlation::of(&correlation));
        self.run(name, options, &parent, correlation, work).await
    }

    /// Runs `work` in a child of the active span, or in a root span when none is active.
    pub async fn span<T, E, F, Fut>(
        &self,
        name: impl Into<Cow<'static, str>>,
        options: SpanOptions,
        work: F,
    ) -> Result<T, E>
    where
        E: fmt::Display,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let current = Context::current();
        let context = match &options.session {
            Some(session) => session.context(&current),
            None => current,
        };
        self.run(name, options, &context.clone(), context, work)
            .await
    }

    async fn run<T, E, F, Fut>(
        &self,
        name: impl Into<Cow<'static, str>>,
        options: SpanOptions,
        parent: &Context,
        scope: Context,
        work: F,
    ) -> Result<T, E>
    where
        E: fmt::Display,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let Some(tracer) = &self.inner.tracer else {
            return work().with_context(scope).await;
        };
        let span = tracer
            .span_builder(name)
            .with_kind(options.kind.unwrap_or(SpanKind::Internal))
            .with_attributes(options.attributes)
            .start_with_context(tracer, parent);
        let context = scope.with_span(span);
        let result = work().with_context(context.clone()).await;
        let span = context.span();
        if let Err(error) = &result {
            span.add_event(
                "exception",
                vec![KeyValue::new("exception.message", error.to_string())],
            );
            span.set_status(Status::error(error.to_string()));
        }
        span.end();
        result
    }

    /// Exports everything pending. Returns whether all of it was exported within `timeout`.
    pub fn flush(&self, timeout: Duration) -> bool {
        let Some(providers) = self.inner.providers.clone() else {
            return true;
        };
        // OpenTelemetry Rust's force_flush takes no deadline, so the wait is bounded here. A flush
        // that overruns keeps going on its thread and its result is dropped.
        let (done, result) = mpsc::channel();
        std::thread::spawn(move || {
            let traces = report("trace flush", providers.tracer.force_flush());
            let logs = providers
                .logger
                .as_ref()
                .is_none_or(|logger| report("log flush", logger.force_flush()));
            let metrics = providers
                .meter
                .as_ref()
                .is_none_or(|meter| report("metric flush", meter.force_flush()));
            let _ = done.send(traces && logs && metrics);
        });
        result.recv_timeout(timeout).unwrap_or_else(|_| {
            eprintln!("[sideseat] flush did not complete within {timeout:?}");
            false
        })
    }

    /// Flushes and stops every exporter. Returns whether everything was exported. Calling it
    /// again returns the first result.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let mut done = self
            .inner
            .shutdown
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(result) = *done {
            return result;
        }
        let result = self.inner.providers.as_ref().is_none_or(|providers| {
            let traces = report(
                "trace shutdown",
                providers.tracer.shutdown_with_timeout(timeout),
            );
            let logs = providers
                .logger
                .as_ref()
                .is_none_or(|logger| report("log shutdown", logger.shutdown_with_timeout(timeout)));
            let metrics = providers.meter.as_ref().is_none_or(|meter| {
                report("metric shutdown", meter.shutdown_with_timeout(timeout))
            });
            traces && logs && metrics
        });
        *done = Some(result);
        result
    }

    fn is_shut_down(&self) -> bool {
        self.inner
            .shutdown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }
}

/// Shuts the pipeline down when the last clone drops, so a short-lived program that forgets
/// `shutdown` still exports what it recorded.
impl Drop for Inner {
    fn drop(&mut self) {
        let finished = self
            .shutdown
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        if let (false, Some(providers)) = (finished, &self.providers) {
            let _ = providers.tracer.shutdown();
            if let Some(logger) = &providers.logger {
                let _ = logger.shutdown();
            }
            if let Some(meter) = &providers.meter {
                let _ = meter.shutdown();
            }
        }
    }
}

fn resource(settings: &Settings) -> Resource {
    Resource::builder()
        .with_attributes([
            KeyValue::new("service.name", settings.service_name.clone()),
            KeyValue::new("service.version", settings.service_version.clone()),
            KeyValue::new("telemetry.sdk.name", "sideseat"),
            KeyValue::new("telemetry.sdk.language", "rust"),
            KeyValue::new("telemetry.sdk.version", crate::VERSION),
        ])
        .with_attributes(settings.resource_attributes.iter().cloned())
        .build()
}

fn exporter_error(signal: &'static str, error: &dyn fmt::Display) -> Error {
    Error::Exporter {
        signal,
        message: error.to_string(),
    }
}

fn report(operation: &str, result: opentelemetry_sdk::error::OTelSdkResult) -> bool {
    match result {
        Ok(()) => true,
        Err(error) => {
            eprintln!("[sideseat] {operation} failed: {error}");
            false
        }
    }
}

/// Adapts a boxed processor from [`Options::span_processor`] to the builder, which takes values.
#[derive(Debug)]
struct BoxedProcessor(Box<dyn SpanProcessor>);

impl SpanProcessor for BoxedProcessor {
    fn on_start(&self, span: &mut opentelemetry_sdk::trace::Span, cx: &Context) {
        self.0.on_start(span, cx);
    }

    fn on_end(&self, span: opentelemetry_sdk::trace::SpanData) {
        self.0.on_end(span);
    }

    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        self.0.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> opentelemetry_sdk::error::OTelSdkResult {
        self.0.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.0.set_resource(resource);
    }
}
