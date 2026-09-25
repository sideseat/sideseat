# SideSeat Rust SDK

**AI Development Workbench** — trace LLM calls, tools, agents, sessions, and custom application work.

[![crates.io](https://img.shields.io/crates/v/sideseat)](https://crates.io/crates/sideseat)
[![Rust 1.94.1+](https://img.shields.io/badge/rust-1.94.1%2B-blue)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

The `sideseat` crate provides:

- an OTLP/HTTP pipeline configured for a SideSeat server;
- independent root traces and correctly parented child spans;
- automatic `session.id` and `user.id` propagation;
- GenAI instrumentation for the crate's built-in model providers;
- explicit flush and shutdown results for short-lived processes.

The telemetry path is tested against the same canonical conversation through both the
SideSeat SDK and plain OpenTelemetry. The resulting span, trace, session, project-feed,
message-order, content, and tool-causality views must be identical.

## Requirements

- Rust 1.94.1 or newer
- a running SideSeat server

```bash
npx sideseat
cargo add sideseat
```

## Root traces, child spans, and sessions

```rust,no_run
use sideseat::{KeyValue, SideSeat, SideSeatSpanOptions, SpanKind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let telemetry = SideSeat::new()
        .with_service_name("travel-agent")
        .with_service_version(env!("CARGO_PKG_VERSION"))
        .with_framework("custom-rust-agent")
        .init()?;

    telemetry
        .trace(
            "plan-trip",
            SideSeatSpanOptions::new()
                .with_session_id("conversation-42")
                .with_user_id("user-7"),
            |_trace| async {
                telemetry
                    .span(
                        "retrieve-context",
                        SideSeatSpanOptions::new()
                            .with_kind(SpanKind::Client)
                            .with_attribute(KeyValue::new("app.documents", 4_i64)),
                        |_span| async { Ok::<_, std::io::Error>(()) },
                    )
                    .await
            },
        )
        .await?;

    telemetry.shutdown()?;
    Ok(())
}
```

`trace()` always starts a new root, even when another OpenTelemetry span is active.
`span()` uses the current span as its parent. Correlation set on either call propagates to
nested SideSeat spans and a nested override is scoped to that nested operation.

Use `start_trace()` and `start_span()` when you need manual lifecycle control. A
`SideSeatSpan` ends when `end()` is called or its final clone is dropped.

## Instrumenting model providers

`InstrumentedProvider` wraps any provider implemented by this crate and emits GenAI
semantic-convention spans and metrics:

```rust,no_run
use sideseat::{
    ChatProvider, InstrumentedProvider, Message, ProviderConfig, SideSeat,
    providers::AnthropicProvider,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let builder = SideSeat::new()
        .with_service_name("support-agent")
        .with_framework("sideseat-rust")
        .with_capture_content(true);
    let provider = InstrumentedProvider::with_config(
        AnthropicProvider::from_env()?,
        builder.telemetry_config(),
    );
    let telemetry = builder.init()?;

    let response = provider
        .complete(
            vec![Message::user("How can I reset my password?")],
            ProviderConfig::new("claude-sonnet-4-5"),
        )
        .await?;
    println!("{:?}", response.first_text());

    telemetry.shutdown()?;
    Ok(())
}
```

Content capture is disabled by default. Enabling it records prompts and model responses as
span events; only enable it when that data is allowed to leave the process.

## Configuration

| Builder | Environment fallback | Default |
| --- | --- | --- |
| `with_endpoint` | `SIDESEAT_ENDPOINT` | `http://localhost:5388` |
| `with_project_id` | `SIDESEAT_PROJECT_ID` | `default` |
| `with_api_key` | `SIDESEAT_API_KEY` | unset |
| `with_service_name` | `OTEL_SERVICE_NAME` | `sideseat-rust` |
| `with_service_version` | none | unset |
| `with_framework` | none | unset |
| `with_capture_content` | none | `false` |

Endpoint routing accepts all common forms:

```text
http://localhost:5388
  -> http://localhost:5388/otel/{project_id}/v1/traces

http://collector:4318/otel/my-project
  -> http://collector:4318/otel/my-project/v1/traces

http://collector:4318/otel/my-project/v1/traces
  -> unchanged
```

When an API key is configured, traces and metrics use
`Authorization: Bearer <key>`.

## Shutdown

Keep `SideSeatGuard` alive for the lifetime of the process. `force_flush()` exports queued
telemetry without closing the providers. `shutdown()` flushes and closes them and returns an
error when an exporter cannot finish. Both shutdown and span ending are idempotent.

Dropping the guard also shuts down the providers, but an explicit call is preferred when the
process must know whether the final export succeeded.

## Compatibility

The crate follows the repository's Rust MSRV and current OpenTelemetry Rust release. It is
pre-1.0, so public API changes may occur between minor versions; backward compatibility is
not promised yet.

## Resources

- [Documentation](https://sideseat.ai/docs/sdks/rust/)
- [GitHub](https://github.com/sideseat/sideseat)
- [Issues](https://github.com/sideseat/sideseat/issues)

## License

[MIT](LICENSE)
