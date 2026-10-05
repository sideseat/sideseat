# SDK contract

Every SideSeat SDK (Python, TypeScript, .NET, Rust) implements the behaviour in this document. The
SDKs differ in idiom, not in semantics: the same program written against any of them exports the same
telemetry and reads back the same spans, traces, sessions, and messages.

An SDK is a thin, opinionated OpenTelemetry distribution. It does not define a telemetry format of its
own. Framework-specific interpretation happens on the server, in `server/assets/rules/`.

## Scope

An SDK does four things:

1. **Pipeline.** It configures OpenTelemetry tracing, logs, and metrics with one OTLP/HTTP exporter per
   signal, pointed at a SideSeat project.
2. **Integrations.** It switches on the telemetry a framework or provider already emits, or installs the
   instrumentation that framework needs, and makes sure each span is exported exactly once.
3. **Correlation.** It attaches `session.id` and `user.id` to every span in a scope, including spans a
   framework creates.
4. **Structure.** It creates root traces and child spans for application code.

## Configuration

Explicit arguments win over environment variables, which win over defaults.

| Setting | Environment | Default |
| --- | --- | --- |
| endpoint | `SIDESEAT_ENDPOINT`, then `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://127.0.0.1:5388` |
| project | `SIDESEAT_PROJECT_ID` | `default` |
| API key | `SIDESEAT_API_KEY` | none |
| service name | `OTEL_SERVICE_NAME` | the primary integration's package name, else `sideseat-app` |
| service version | `OTEL_SERVICE_VERSION` | the primary integration's package version, else the SDK version |
| integrations | `SIDESEAT_INTEGRATIONS` (comma-separated) | auto-detected |
| content capture | `SIDESEAT_CAPTURE_CONTENT` | on |
| disabled | `SIDESEAT_DISABLED` | off |
| debug logging | `SIDESEAT_DEBUG` | off |

Boolean variables accept `1/0`, `true/false`, and `yes/no`, in any case. An invalid value is an error,
not a silent default. An empty or blank argument or variable counts as unset.

**Endpoint resolution.** An endpoint without a path is a SideSeat server; the OTLP base is
`{endpoint}/otel/{project}`. An endpoint with a path is already an OTLP base. Each signal is exported to
`{base}/v1/{traces|logs|metrics}`. A trailing slash is ignored. A scheme other than `http` or `https`
is an error.

**Authentication.** An API key is sent as `Authorization: Bearer <key>`. Headers from
`OTEL_EXPORTER_OTLP_HEADERS` are kept; the API key replaces an `Authorization` header of any spelling.

**Signals.** Metrics are exported every minute, the OpenTelemetry default. Each signal can be switched
off, and turning export off sends nothing, which tests use with extra span processors.

## Lifecycle

- `init` configures the pipeline once per process and returns the client. Calling it again with the same
  settings returns the existing client. Calling it with different settings is an error: telemetry
  configuration is global, so a silent second configuration would leave the process exporting with
  whichever won.
- `flush(timeout)` exports everything pending and reports whether it all succeeded within the timeout.
  Where the OpenTelemetry SDK does not surface export results from a flush (.NET), it reports whether
  every exporter finished in time.
- `shutdown(timeout)` flushes, stops every exporter and integration, and reports success. It runs at
  process exit automatically and is idempotent. Rust has no exit hook; its pipeline shuts down when the
  last clone of the client drops. OpenTelemetry's global providers can be set once per process, so a
  second pipeline after shutdown is a testing facility (`sideseat.testing` in Python), not a production
  feature.
- When disabled, every call is a no-op. Spans are non-recording and nothing is exported.

## Provider ownership

SideSeat owns the global tracer, logger, and meter providers unless an integration has to own one. A
framework that builds its own tracer provider (Logfire, Laminar) owns it; SideSeat then attaches its
processors to that provider and suppresses the framework's own exporters. At most one integration may own
a provider.

An existing SDK tracer provider set by the application is reused rather than replaced where the
language's OpenTelemetry SDK can add processors to a built provider (Python). Elsewhere it cannot, so the
SDK offers a hook into the application's own pipeline instead (`AddSideSeat` in .NET), or warns that
the global provider was already taken (TypeScript). Rust's global setter cannot tell an SDK provider from
another, so `init` replaces it.

Processor order on the tracer provider is fixed:

1. the correlation processor, so attributes are present before any other processor reads the span;
2. integration processors, such as span repair for a framework that emits a broken topology;
3. the batch exporter.

## Correlation

`session(session_id, user_id)` is a scope, not a span. Inside it, every span started in the process
receives `session.id` and `user.id` attributes, including spans created by a framework. A session id is
required and a user id optional; an empty one is an error, because it would merge unrelated
conversations. The values travel
in the OpenTelemetry context under a private key, so they follow the trace context across `async`
boundaries and into threads that propagate context. They are deliberately not W3C baggage: HTTP client
instrumentation injects baggage into every outgoing request, which would send end-user identifiers to
model providers. A service that continues a session sets it again. A nested scope overrides the outer
one for its duration and restores it on exit.

`trace(name, session_id, user_id)` starts a new root span, even when another span is active, and applies
the same correlation to its descendants. `span(name)` starts a child of the active span.

Application code never needs to set `session.id` on a span by hand.

## Content

Content capture is on by default because the point of SideSeat is to read the conversation. The SDK sets
the standard `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT` switch, and each integration enables
its framework's own equivalent. Rust is the exception: modifying the environment of a running Rust
program is unsound once other threads may read it, so it exposes the setting as `captures_content()`.
Turning capture off is honoured by every integration. Binary content is
exported base64-encoded; the server moves it to file storage on ingest.

## Integrations

An integration names a framework or provider and knows how to turn its telemetry on. It declares:

- the packages that identify it, used for auto-detection and for `service.name`/`service.version`;
- whether it owns a tracer provider;
- what to do before the provider exists, after it exists, and at shutdown.

Several integrations can be active at once, for example a framework and the provider it calls. The first
one listed is the primary integration and is recorded as the `sideseat.framework` resource attribute;
all of them are recorded in `sideseat.integrations`. Auto-detection never activates a bare provider
client library, because those are installed transitively by many frameworks.

An integration must not leave the process environment changed after `init` returns, except for the
documented content-capture switches that instrumentations read lazily. One integration needs a narrow
exception: a framework that configures a child process only through the environment it inherits, where
the language cannot wrap the framework's options instead (the Claude Agent SDK in TypeScript, whose ES
module exports cannot be patched). It sets only variables the application has not set and removes them
at shutdown. Switches an integration sets elsewhere, such as .NET `AppContext` switches, are restored at
shutdown.

An SDK whose language has no integrations (Rust) neither detects nor records any; applications set
`sideseat.framework` as an explicit resource attribute when it applies.

## Resource

Every signal carries `service.name`, `service.version`, `telemetry.sdk.name = sideseat`,
`telemetry.sdk.language`, and `telemetry.sdk.version`; when an integration is active, also
`sideseat.framework` and `sideseat.integrations`. `OTEL_RESOURCE_ATTRIBUTES` sits underneath them and
explicit resource attributes on top.

## Errors

Configuration errors raise at `init`. Nothing after `init` raises because of telemetry: export failures
are logged and reported through the boolean results of `flush` and `shutdown`. An integration whose
package is missing is an error when it was requested explicitly, and is skipped when it was only
auto-detected.

## Conformance

Each SDK has a conformance program in `examples/<language>/conformance` that runs one canonical
conversation twice: through the SDK, and through plain OpenTelemetry configured by hand. Both runs are
captured as OTLP fixtures, and `sdk_and_plain_otel_conformance_are_identical` in the message goldens
requires identical span, trace, session, and feed views. The rubric those views are scored against is in
`server/tests/fixtures/messages/README.md`.
