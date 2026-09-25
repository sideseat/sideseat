use std::error::Error;

use opentelemetry::global;
use opentelemetry::trace::{Span as _, SpanKind, TraceContextExt as _, Tracer as _};
use opentelemetry::{Context, KeyValue};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{BatchSpanProcessor, SdkTracerProvider};
use sideseat::telemetry::SideSeat;

const SESSION_ID: &str = "sdk-conformance-session";
const USER_ID: &str = "sdk-conformance-user";
const INPUT_MESSAGES: &str =
    r#"[{"role":"user","parts":[{"type":"text","content":"What is the weather in London?"}]}]"#;
const FIRST_OUTPUT: &str = r#"[{"role":"assistant","parts":[{"type":"text","content":"I will check the weather."}],"finish_reason":"tool_calls"}]"#;
const FINAL_OUTPUT: &str = r#"[{"role":"assistant","parts":[{"type":"text","content":"It is 18°C and sunny in London."}],"finish_reason":"stop"}]"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mode = std::env::args().nth(1);
    match mode.as_deref() {
        Some("sdk") => run_with_sideseat()?,
        Some("otel") => run_with_opentelemetry()?,
        _ => {
            eprintln!("usage: cargo run -p sideseat --example sdk-conformance -- sdk|otel");
            std::process::exit(2);
        }
    }
    Ok(())
}

fn run_with_sideseat() -> Result<(), Box<dyn Error>> {
    let endpoint =
        std::env::var("SIDESEAT_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:5388".to_string());
    let project = std::env::var("SIDESEAT_PROJECT_ID").unwrap_or_else(|_| "default".to_string());
    let guard = SideSeat::new()
        .with_endpoint(endpoint)
        .with_project_id(project)
        .init()?;

    emit_conversation();
    drop(guard);
    Ok(())
}

fn run_with_opentelemetry() -> Result<(), Box<dyn Error>> {
    let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(trace_endpoint())
        .build()?;
    let resource = Resource::builder_empty()
        .with_attributes([
            KeyValue::new("service.name", "rust-conformance"),
            KeyValue::new("sideseat.framework", "rust-conformance"),
        ])
        .build();
    let provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_span_processor(BatchSpanProcessor::builder(trace_exporter).build())
        .build();
    global::set_tracer_provider(provider.clone());

    emit_conversation();
    provider.force_flush()?;
    provider.shutdown()?;
    Ok(())
}

fn trace_endpoint() -> String {
    let endpoint =
        std::env::var("SIDESEAT_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:5388".to_string());
    let project = std::env::var("SIDESEAT_PROJECT_ID").unwrap_or_else(|_| "default".to_string());
    format!(
        "{}/otel/{project}/v1/traces",
        endpoint.trim_end_matches('/')
    )
}

fn emit_conversation() {
    let tracer = global::tracer("rust-conformance");
    let mut root = tracer
        .span_builder("canonical-agent-run")
        .with_kind(SpanKind::Internal)
        .start_with_context(&tracer, &Context::new());
    add_correlation(&mut root);
    root.set_attribute(KeyValue::new("sideseat.framework", "rust-conformance"));

    let root_context = Context::new().with_span(root);
    let _root_guard = root_context.clone().attach();

    emit_span("chat canonical-model", SpanKind::Client, |span| {
        span.set_attribute(KeyValue::new("gen_ai.operation.name", "chat"));
        span.set_attribute(KeyValue::new("gen_ai.provider.name", "conformance"));
        span.set_attribute(KeyValue::new("gen_ai.request.model", "canonical-model"));
        span.set_attribute(KeyValue::new("gen_ai.input.messages", INPUT_MESSAGES));
        span.set_attribute(KeyValue::new("gen_ai.output.messages", FIRST_OUTPUT));
        span.set_attribute(KeyValue::new("gen_ai.usage.input_tokens", 12_i64));
        span.set_attribute(KeyValue::new("gen_ai.usage.output_tokens", 6_i64));
    });

    emit_span("execute_tool get_weather", SpanKind::Internal, |span| {
        span.set_attribute(KeyValue::new("gen_ai.operation.name", "execute_tool"));
        span.set_attribute(KeyValue::new("gen_ai.tool.name", "get_weather"));
        span.set_attribute(KeyValue::new("gen_ai.tool.call.id", "call-weather-1"));
        span.set_attribute(KeyValue::new("gen_ai.tool.type", "function"));
        span.set_attribute(KeyValue::new(
            "gen_ai.tool.call.arguments",
            r#"{"city":"London"}"#,
        ));
        span.set_attribute(KeyValue::new(
            "gen_ai.tool.call.result",
            r#"{"temperature_c":18,"condition":"sunny"}"#,
        ));
    });

    emit_span("chat canonical-model", SpanKind::Client, |span| {
        span.set_attribute(KeyValue::new("gen_ai.operation.name", "chat"));
        span.set_attribute(KeyValue::new("gen_ai.provider.name", "conformance"));
        span.set_attribute(KeyValue::new("gen_ai.request.model", "canonical-model"));
        span.set_attribute(KeyValue::new("gen_ai.output.messages", FINAL_OUTPUT));
        span.set_attribute(KeyValue::new(
            "gen_ai.response.finish_reasons",
            r#"["stop"]"#,
        ));
        span.set_attribute(KeyValue::new("gen_ai.usage.input_tokens", 24_i64));
        span.set_attribute(KeyValue::new("gen_ai.usage.output_tokens", 10_i64));
    });

    root_context.span().end();
}

fn emit_span(
    name: &'static str,
    kind: SpanKind,
    add_attributes: impl FnOnce(&mut opentelemetry::global::BoxedSpan),
) {
    let tracer = global::tracer("rust-conformance");
    let mut span = tracer.span_builder(name).with_kind(kind).start(&tracer);
    add_correlation(&mut span);
    add_attributes(&mut span);
    span.end();
}

fn add_correlation(span: &mut opentelemetry::global::BoxedSpan) {
    span.set_attribute(KeyValue::new("session.id", SESSION_ID));
    span.set_attribute(KeyValue::new("user.id", USER_ID));
}
