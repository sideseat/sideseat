use std::error::Error;
use std::time::Duration;

use opentelemetry::global;
use opentelemetry::trace::{Span as _, SpanKind, TraceContextExt as _, Tracer as _};
use opentelemetry::{Context, KeyValue};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{BatchSpanProcessor, SdkTracerProvider};
use sideseat::{Options, Session, SpanOptions};

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
        Some("sdk") => run_with_sideseat().await?,
        Some("otel") => run_with_opentelemetry()?,
        _ => {
            eprintln!("usage: cargo run -p sideseat --example sdk-conformance -- sdk|otel");
            std::process::exit(2);
        }
    }
    Ok(())
}

async fn run_with_sideseat() -> Result<(), Box<dyn Error>> {
    let telemetry = sideseat::init(
        Options::new()
            .service_name("rust-conformance")
            .resource_attribute(KeyValue::new("sideseat.framework", "rust-conformance"))
            .logs(false),
    )?;
    let conversation = Session::new(SESSION_ID)?.user(USER_ID)?;
    let chat = || SpanOptions::new().kind(SpanKind::Client);

    telemetry
        .trace(
            "canonical-agent-run",
            SpanOptions::new().session(conversation),
            || async {
                telemetry
                    .span(
                        "chat canonical-model",
                        chat().attributes(chat_attributes(INPUT_MESSAGES, FIRST_OUTPUT, 12, 6)),
                        done,
                    )
                    .await?;
                telemetry
                    .span(
                        "execute_tool get_weather",
                        SpanOptions::new().attributes(tool_attributes()),
                        done,
                    )
                    .await?;
                telemetry
                    .span(
                        "chat canonical-model",
                        chat().attributes(final_chat_attributes()),
                        done,
                    )
                    .await
            },
        )
        .await?;
    if !telemetry.shutdown(Duration::from_secs(10)) {
        return Err("the SDK did not export every span".into());
    }
    Ok(())
}

async fn done() -> Result<(), std::io::Error> {
    operate();
    Ok(())
}

/// Every operation takes measurable time, as real model and tool calls do. A tool span that starts
/// and ends on one clock tick gives its call and result one anchor, which the feed reads as a single
/// response - so without this the two modes' feeds would differ by timing rather than by telemetry.
fn operate() {
    std::thread::sleep(Duration::from_millis(1));
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

    emit_raw_conversation();
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

fn emit_raw_conversation() {
    let tracer = global::tracer("rust-conformance");
    let mut root = tracer
        .span_builder("canonical-agent-run")
        .with_kind(SpanKind::Internal)
        .start_with_context(&tracer, &Context::new());
    add_correlation(&mut root);
    root.set_attribute(KeyValue::new("sideseat.framework", "rust-conformance"));

    let root_context = Context::new().with_span(root);
    let _root_guard = root_context.clone().attach();

    emit_raw_span("chat canonical-model", SpanKind::Client, |span| {
        for attribute in chat_attributes(INPUT_MESSAGES, FIRST_OUTPUT, 12, 6) {
            span.set_attribute(attribute);
        }
    });

    emit_raw_span("execute_tool get_weather", SpanKind::Internal, |span| {
        for attribute in tool_attributes() {
            span.set_attribute(attribute);
        }
    });

    emit_raw_span("chat canonical-model", SpanKind::Client, |span| {
        for attribute in final_chat_attributes() {
            span.set_attribute(attribute);
        }
    });

    root_context.span().end();
}

fn emit_raw_span(
    name: &'static str,
    kind: SpanKind,
    add_attributes: impl FnOnce(&mut opentelemetry::global::BoxedSpan),
) {
    let tracer = global::tracer("rust-conformance");
    let mut span = tracer.span_builder(name).with_kind(kind).start(&tracer);
    add_correlation(&mut span);
    add_attributes(&mut span);
    operate();
    span.end();
}

fn chat_attributes(
    input: &'static str,
    output: &'static str,
    input_tokens: i64,
    output_tokens: i64,
) -> Vec<KeyValue> {
    vec![
        KeyValue::new("gen_ai.operation.name", "chat"),
        KeyValue::new("gen_ai.provider.name", "conformance"),
        KeyValue::new("gen_ai.request.model", "canonical-model"),
        KeyValue::new("gen_ai.input.messages", input),
        KeyValue::new("gen_ai.output.messages", output),
        KeyValue::new("gen_ai.usage.input_tokens", input_tokens),
        KeyValue::new("gen_ai.usage.output_tokens", output_tokens),
    ]
}

fn tool_attributes() -> Vec<KeyValue> {
    vec![
        KeyValue::new("gen_ai.operation.name", "execute_tool"),
        KeyValue::new("gen_ai.tool.name", "get_weather"),
        KeyValue::new("gen_ai.tool.call.id", "call-weather-1"),
        KeyValue::new("gen_ai.tool.type", "function"),
        KeyValue::new("gen_ai.tool.call.arguments", r#"{"city":"London"}"#),
        KeyValue::new(
            "gen_ai.tool.call.result",
            r#"{"temperature_c":18,"condition":"sunny"}"#,
        ),
    ]
}

fn final_chat_attributes() -> Vec<KeyValue> {
    vec![
        KeyValue::new("gen_ai.operation.name", "chat"),
        KeyValue::new("gen_ai.provider.name", "conformance"),
        KeyValue::new("gen_ai.request.model", "canonical-model"),
        KeyValue::new("gen_ai.output.messages", FINAL_OUTPUT),
        KeyValue::new("gen_ai.response.finish_reasons", r#"["stop"]"#),
        KeyValue::new("gen_ai.usage.input_tokens", 24_i64),
        KeyValue::new("gen_ai.usage.output_tokens", 10_i64),
    ]
}

fn add_correlation(span: &mut opentelemetry::global::BoxedSpan) {
    span.set_attribute(KeyValue::new("session.id", SESSION_ID));
    span.set_attribute(KeyValue::new("user.id", USER_ID));
}
