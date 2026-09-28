use std::sync::{Arc, Mutex};

use opentelemetry::trace::{SpanId, TracerProvider as _};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};

use super::{SideSeat, SideSeatGuard, SideSeatSpanOptions, Status, signal_endpoint};
use crate::env::keys;

#[derive(Clone, Debug, Default)]
struct TestExporter {
    spans: Arc<Mutex<Vec<SpanData>>>,
}

impl TestExporter {
    fn finished_spans(&self) -> Vec<SpanData> {
        self.spans
            .lock()
            .expect("test exporter lock poisoned")
            .clone()
    }
}

impl SpanExporter for TestExporter {
    fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
        let spans = Arc::clone(&self.spans);
        async move {
            spans
                .lock()
                .expect("test exporter lock poisoned")
                .extend(batch);
            Ok(())
        }
    }
}

#[test]
fn sideseat_reads_defaults_and_environment_values() {
    let defaults = SideSeat::from_environment(|_| None);
    assert_eq!(defaults.endpoint, "http://localhost:5388");
    assert_eq!(defaults.project_id, "default");
    assert_eq!(defaults.api_key, None);

    let configured = SideSeat::from_environment(|name| match name {
        keys::SIDESEAT_ENDPOINT => Some("http://test:9999".to_string()),
        keys::SIDESEAT_PROJECT_ID => Some("from-env".to_string()),
        keys::SIDESEAT_API_KEY => Some("test-key".to_string()),
        _ => None,
    });
    assert_eq!(configured.endpoint, "http://test:9999");
    assert_eq!(configured.project_id, "from-env");
    assert_eq!(configured.api_key.as_deref(), Some("test-key"));
}

#[test]
fn signal_endpoints_accept_server_collector_and_complete_urls() {
    assert_eq!(
        signal_endpoint("http://localhost:5388", "project-a", "traces").unwrap(),
        "http://localhost:5388/otel/project-a/v1/traces"
    );
    assert_eq!(
        signal_endpoint(
            "http://collector:4318/otel/project-a/",
            "ignored",
            "metrics"
        )
        .unwrap(),
        "http://collector:4318/otel/project-a/v1/metrics"
    );
    assert_eq!(
        signal_endpoint(
            "http://collector:4318/otel/project-a/v1/traces",
            "ignored",
            "metrics"
        )
        .unwrap(),
        "http://collector:4318/otel/project-a/v1/metrics"
    );
}

#[tokio::test]
async fn trace_and_span_preserve_root_child_and_correlation_semantics() {
    let exporter = TestExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let guard = SideSeatGuard {
        tracer: tracer_provider.tracer("test"),
        tracer_provider,
        meter_provider: SdkMeterProvider::builder().build(),
        shutdown_result: Mutex::new(None),
    };

    guard
        .trace(
            "root",
            SideSeatSpanOptions::new()
                .with_session_id("session-1")
                .with_user_id("user-1"),
            |_root| async {
                guard
                    .span(
                        "overridden-child",
                        SideSeatSpanOptions::new().with_session_id("session-2"),
                        |_child| async { Ok::<_, std::io::Error>(()) },
                    )
                    .await?;
                guard
                    .span(
                        "inherited-child",
                        SideSeatSpanOptions::new(),
                        |_child| async { Ok::<_, std::io::Error>(()) },
                    )
                    .await?;
                guard
                    .trace("nested-root", SideSeatSpanOptions::new(), |_nested| async {
                        Ok::<_, std::io::Error>(())
                    })
                    .await
            },
        )
        .await
        .unwrap();
    let error = guard
        .span("failed-root", SideSeatSpanOptions::new(), |_span| async {
            Err::<(), _>(std::io::Error::other("expected failure"))
        })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "expected failure");
    guard.force_flush().unwrap();

    let spans = exporter.finished_spans();
    let root = spans.iter().find(|span| span.name == "root").unwrap();
    let overridden = spans
        .iter()
        .find(|span| span.name == "overridden-child")
        .unwrap();
    let inherited = spans
        .iter()
        .find(|span| span.name == "inherited-child")
        .unwrap();
    let nested_root = spans
        .iter()
        .find(|span| span.name == "nested-root")
        .unwrap();
    let failed_root = spans
        .iter()
        .find(|span| span.name == "failed-root")
        .unwrap();
    assert_eq!(root.parent_span_id, SpanId::INVALID);
    assert_eq!(overridden.parent_span_id, root.span_context.span_id());
    assert_eq!(inherited.parent_span_id, root.span_context.span_id());
    assert_eq!(nested_root.parent_span_id, SpanId::INVALID);
    assert_eq!(failed_root.parent_span_id, SpanId::INVALID);
    for span in [root, inherited] {
        assert_eq!(attribute(span, "session.id"), Some("session-1"));
        assert_eq!(attribute(span, "user.id"), Some("user-1"));
    }
    assert_eq!(attribute(overridden, "session.id"), Some("session-2"));
    assert_eq!(attribute(overridden, "user.id"), Some("user-1"));
    assert_eq!(attribute(nested_root, "session.id"), None);
    assert!(matches!(
        failed_root.status,
        Status::Error { ref description } if description == "expected failure"
    ));
    assert!(
        failed_root
            .events
            .iter()
            .any(|event| event.name == "exception")
    );
    assert!(guard.shutdown().is_ok());
    assert!(guard.shutdown().is_ok());
}

fn attribute<'a>(span: &'a opentelemetry_sdk::trace::SpanData, key: &str) -> Option<&'a str> {
    span.attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == key)
        .and_then(|attribute| match &attribute.value {
            opentelemetry::Value::String(value) => Some(value.as_str()),
            _ => None,
        })
}
