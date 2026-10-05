//! The contract through the public API. nextest runs each test in its own process, so each one
//! owns the global providers.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use opentelemetry::trace::{Span as _, SpanKind, Status, TraceContextExt as _, Tracer as _};
use opentelemetry::{Context, Key, KeyValue, global};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{
    InMemorySpanExporter, SimpleSpanProcessor, SpanData, SpanProcessor,
};
use sideseat::{Error, Options, Session, SpanOptions};

const TIMEOUT: Duration = Duration::from_secs(5);

fn recording() -> (sideseat::SideSeat, InMemorySpanExporter) {
    let exporter = InMemorySpanExporter::default();
    let client = sideseat::init(
        Options::new()
            .export(false)
            .span_processor(SimpleSpanProcessor::new(exporter.clone())),
    )
    .unwrap();
    (client, exporter)
}

fn attribute(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.to_string())
}

fn named<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    spans
        .iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| panic!("no span named {name}"))
}

/// A span a framework creates through the global tracer, after an await point.
async fn framework_call(name: &'static str) {
    tokio::task::yield_now().await;
    global::tracer("some-framework").start(name).end();
}

#[tokio::test]
async fn a_session_reaches_spans_a_framework_creates() {
    let (_client, exporter) = recording();

    Session::new("conversation-42")
        .unwrap()
        .user("user-7")
        .unwrap()
        .scope(framework_call("chat model"))
        .await;
    framework_call("outside").await;

    let spans = exporter.get_finished_spans().unwrap();
    let inside = named(&spans, "chat model");
    assert_eq!(
        attribute(inside, "session.id").as_deref(),
        Some("conversation-42")
    );
    assert_eq!(attribute(inside, "user.id").as_deref(), Some("user-7"));
    assert_eq!(attribute(named(&spans, "outside"), "session.id"), None);
}

#[tokio::test]
async fn a_nested_session_overrides_and_restores_the_outer_one() {
    let (_client, exporter) = recording();
    let outer = Session::new("outer").unwrap().user("user-1").unwrap();

    outer
        .scope(async {
            framework_call("before").await;
            Session::new("inner")
                .unwrap()
                .scope(framework_call("nested"))
                .await;
            framework_call("after").await;
        })
        .await;

    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(
        attribute(named(&spans, "nested"), "session.id").as_deref(),
        Some("inner")
    );
    assert_eq!(
        attribute(named(&spans, "nested"), "user.id").as_deref(),
        Some("user-1")
    );
    for name in ["before", "after"] {
        assert_eq!(
            attribute(named(&spans, name), "session.id").as_deref(),
            Some("outer")
        );
    }
}

#[test]
fn an_entered_session_holds_for_synchronous_code_until_the_guard_drops() {
    let (_client, exporter) = recording();

    {
        let _entered = Session::new("sync").unwrap().enter();
        global::tracer("lib").start("inside").end();
    }
    global::tracer("lib").start("outside").end();

    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(
        attribute(named(&spans, "inside"), "session.id").as_deref(),
        Some("sync")
    );
    assert_eq!(attribute(named(&spans, "outside"), "session.id"), None);
}

#[tokio::test]
async fn trace_starts_a_new_root_and_span_nests_under_the_active_one() {
    let (client, exporter) = recording();
    let options = SpanOptions::new().session(Session::new("s-1").unwrap());

    let outer = global::tracer("app").start("ambient");
    let ambient = Context::current_with_span(outer);
    let result: Result<u8, std::io::Error> = client
        .trace("agent-run", options, || async {
            client
                .span(
                    "retrieve",
                    SpanOptions::new()
                        .kind(SpanKind::Client)
                        .attribute(KeyValue::new("app.documents", 4_i64)),
                    || async {
                        framework_call("chat model").await;
                        Ok::<_, std::io::Error>(())
                    },
                )
                .await?;
            Ok(7)
        })
        .with_context_of(ambient.clone())
        .await;
    ambient.span().end();

    assert_eq!(result.unwrap(), 7);
    let spans = exporter.get_finished_spans().unwrap();
    let (root, child, model) = (
        named(&spans, "agent-run"),
        named(&spans, "retrieve"),
        named(&spans, "chat model"),
    );
    assert_eq!(root.parent_span_id, opentelemetry::trace::SpanId::INVALID);
    assert_ne!(
        root.span_context.trace_id(),
        named(&spans, "ambient").span_context.trace_id()
    );
    assert_eq!(child.parent_span_id, root.span_context.span_id());
    assert_eq!(model.parent_span_id, child.span_context.span_id());
    assert_eq!(child.span_kind, SpanKind::Client);
    assert_eq!(attribute(child, "app.documents").as_deref(), Some("4"));
    for span in [root, child, model] {
        assert_eq!(
            attribute(span, "session.id").as_deref(),
            Some("s-1"),
            "{}",
            span.name
        );
    }
}

#[tokio::test]
async fn trace_keeps_the_current_session_when_it_names_none() {
    let (client, exporter) = recording();

    Session::new("ambient-session")
        .unwrap()
        .scope(client.trace("run", SpanOptions::new(), || async {
            Ok::<_, std::io::Error>(())
        }))
        .await
        .unwrap();

    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(
        attribute(named(&spans, "run"), "session.id").as_deref(),
        Some("ambient-session")
    );
}

#[tokio::test]
async fn a_failed_operation_marks_its_span_as_an_error() {
    let (client, exporter) = recording();

    let result: Result<(), String> = client
        .span("book", SpanOptions::new(), || async {
            Err("no seats".to_string())
        })
        .await;

    assert!(result.is_err());
    let spans = exporter.get_finished_spans().unwrap();
    let span = named(&spans, "book");
    assert_eq!(span.status, Status::error("no seats"));
    assert_eq!(span.events.events[0].name, "exception");
}

#[test]
fn init_is_idempotent_for_equal_settings_and_refuses_different_ones() {
    let first = sideseat::init(Options::new().export(false).service_name("a")).unwrap();
    let again = sideseat::init(Options::new().export(false).service_name("a")).unwrap();
    let different = sideseat::init(Options::new().export(false).service_name("b"));

    assert_eq!(again, first);
    assert!(matches!(different, Err(Error::AlreadyInitialized)));
    assert!(sideseat::client().is_some());

    assert!(first.shutdown(TIMEOUT));
    assert!(first.shutdown(TIMEOUT), "shutdown is idempotent");
    assert!(sideseat::client().is_none());
    assert!(sideseat::init(Options::new().export(false).service_name("b")).is_ok());
}

#[tokio::test]
async fn when_disabled_work_still_runs_and_nothing_is_recorded() {
    let client = sideseat::init(Options::new().disabled(true)).unwrap();

    let value = client
        .trace("run", SpanOptions::new(), || async {
            Ok::<_, std::io::Error>(3)
        })
        .await
        .unwrap();

    assert_eq!(value, 3);
    assert!(client.tracer_provider().is_none());
    assert!(client.flush(TIMEOUT));
    assert!(client.shutdown(TIMEOUT));
}

/// Keeps the resource the provider hands its processors.
#[derive(Debug, Clone, Default)]
struct ResourceProbe(Arc<Mutex<Option<Resource>>>);

impl SpanProcessor for ResourceProbe {
    fn on_start(&self, _span: &mut opentelemetry_sdk::trace::Span, _cx: &Context) {}
    fn on_end(&self, _span: SpanData) {}
    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }
    fn set_resource(&mut self, resource: &Resource) {
        *self.0.lock().unwrap() = Some(resource.clone());
    }
}

#[test]
fn every_signal_carries_the_sideseat_resource() {
    let probe = ResourceProbe::default();
    let _client = sideseat::init(
        Options::new()
            .export(false)
            .resource_attribute(KeyValue::new("deployment.environment.name", "test"))
            .span_processor(probe.clone()),
    )
    .unwrap();

    let resource = probe.0.lock().unwrap().clone().unwrap();
    let get = |key: &'static str| {
        resource
            .get(&Key::from_static_str(key))
            .map(|v| v.to_string())
    };
    assert_eq!(get("telemetry.sdk.name").as_deref(), Some("sideseat"));
    assert_eq!(get("telemetry.sdk.language").as_deref(), Some("rust"));
    assert_eq!(
        get("telemetry.sdk.version").as_deref(),
        Some(sideseat::VERSION)
    );
    assert_eq!(get("service.name").as_deref(), Some("sideseat-app"));
    assert_eq!(get("service.version").as_deref(), Some(sideseat::VERSION));
    assert_eq!(get("deployment.environment.name").as_deref(), Some("test"));
}

#[test]
fn spans_are_exported_over_otlp_http_with_the_api_key() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (heads, received) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut buffer = vec![0; 64 * 1024];
            let read = stream.read(&mut buffer).unwrap();
            let head = String::from_utf8_lossy(&buffer[..read]).to_string();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .unwrap();
            heads.send(head).unwrap();
        }
    });

    let client = sideseat::init(
        Options::new()
            .endpoint(endpoint)
            .project("team a")
            .api_key("k-1")
            .logs(false),
    )
    .unwrap();
    global::tracer("lib").start("work").end();
    global::meter("lib")
        .u64_counter("requests")
        .build()
        .add(1, &[]);
    assert!(client.shutdown(TIMEOUT));

    let heads: Vec<String> = (0..2)
        .map(|_| received.recv_timeout(TIMEOUT).unwrap())
        .collect();
    let mut request_lines: Vec<&str> = heads
        .iter()
        .map(|head| head.lines().next().unwrap())
        .collect();
    request_lines.sort_unstable();
    assert_eq!(
        request_lines,
        [
            "POST /otel/team%20a/v1/metrics HTTP/1.1",
            "POST /otel/team%20a/v1/traces HTTP/1.1"
        ]
    );
    for head in &heads {
        assert!(
            head.lines()
                .any(|line| line.eq_ignore_ascii_case("authorization: Bearer k-1")),
            "{head}"
        );
    }
}

/// Records which lifecycle calls a provider made, and can make flushing slow.
#[derive(Debug, Clone, Default)]
struct LifecycleProbe {
    calls: Arc<Mutex<Vec<&'static str>>>,
    slow_flush: Option<Duration>,
}

impl SpanProcessor for LifecycleProbe {
    fn on_start(&self, _span: &mut opentelemetry_sdk::trace::Span, _cx: &Context) {}
    fn on_end(&self, _span: SpanData) {}
    fn force_flush(&self) -> OTelSdkResult {
        if let Some(delay) = self.slow_flush {
            std::thread::sleep(delay);
        }
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push("flush");
        Ok(())
    }
    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push("shutdown");
        Ok(())
    }
}

#[test]
fn dropping_the_last_clone_shuts_the_pipeline_down() {
    let probe = LifecycleProbe::default();
    let client = sideseat::init(
        Options::new()
            .export(false)
            .service_name("a")
            .span_processor(probe.clone()),
    )
    .unwrap();
    let clone = client.clone();

    drop(client);
    assert!(probe.calls.lock().unwrap().is_empty(), "a clone is alive");
    assert_eq!(sideseat::client().as_ref(), Some(&clone));
    drop(clone);

    assert!(probe.calls.lock().unwrap().contains(&"shutdown"));
    assert!(sideseat::client().is_none());
    assert!(sideseat::init(Options::new().export(false).service_name("b")).is_ok());
}

#[test]
fn flush_reports_false_when_it_overruns_its_timeout() {
    let probe = LifecycleProbe {
        slow_flush: Some(Duration::from_secs(2)),
        ..LifecycleProbe::default()
    };
    let client = sideseat::init(Options::new().export(false).span_processor(probe)).unwrap();

    let started = std::time::Instant::now();
    assert!(!client.flush(Duration::from_millis(50)));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn empty_session_and_user_ids_are_errors() {
    assert!(matches!(
        Session::new(""),
        Err(Error::EmptyId("session id"))
    ));
    assert!(matches!(
        Session::new("s").unwrap().user(""),
        Err(Error::EmptyId("user id"))
    ));
}

#[test]
fn signals_can_be_switched_off_and_logs_need_no_export() {
    let with_logs = sideseat::init(Options::new().export(false).metrics(false)).unwrap();
    assert!(with_logs.logger_provider().is_some());
    assert!(with_logs.shutdown(TIMEOUT));

    let without_logs = sideseat::init(Options::new().export(false).logs(false)).unwrap();
    assert!(without_logs.logger_provider().is_none());
}

trait WithContextOf: Sized {
    fn with_context_of(self, context: Context) -> opentelemetry::trace::WithContext<Self>;
}

impl<F: std::future::Future> WithContextOf for F {
    fn with_context_of(self, context: Context) -> opentelemetry::trace::WithContext<Self> {
        opentelemetry::trace::FutureExt::with_context(self, context)
    }
}
