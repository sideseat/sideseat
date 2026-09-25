# SideSeat

**AI Development Workbench** — Debug, trace, and understand your AI agents.

[![NuGet](https://img.shields.io/nuget/v/SideSeat)](https://www.nuget.org/packages/SideSeat)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

SideSeat captures every LLM call, tool call, and agent decision, then displays them in a web UI as they happen. Built on [OpenTelemetry](https://opentelemetry.io/).

## Installation

```bash
dotnet add package SideSeat
```

The package targets .NET Standard 2.0 and exports OTLP/HTTP protobuf traces to SideSeat.

## Quick start

Start the local workbench:

```bash
npx sideseat
```

Create an independent trace and child spans:

```csharp
using System.Diagnostics;
using SideSeat;

using var sideSeat = new SideSeatClient(new SideSeatOptions("semantic-kernel")
{
    ProjectId = "default",
    ServiceName = "travel-agent",
});

using (var trace = sideSeat.StartTrace(
    "plan-trip",
    sessionId: "conversation-42",
    userId: "user-7"))
{
    trace.SetAttribute("input.value", "Plan a weekend in Lisbon");

    using var modelCall = sideSeat.StartSpan("chat", ActivityKind.Client);
    modelCall
        .SetAttribute("gen_ai.system", "openai")
        .SetAttribute("gen_ai.request.model", "gpt-5");
}

sideSeat.ForceFlush();
```

`StartTrace` always creates a root operation, even when another `Activity` is active.
`StartSpan` uses the current activity as its parent. Session and user identifiers propagate
through child spans created by the client and are restored correctly after nested scopes end.

## Configuration

Code options take the values loaded from the environment as their starting point:

| Environment variable | Purpose | Default |
|---|---|---|
| `SIDESEAT_ENDPOINT` | SideSeat URL, OTLP project URL, or full traces URL | `http://127.0.0.1:5388` |
| `SIDESEAT_PROJECT_ID` | Project used with a base SideSeat URL | `default` |
| `SIDESEAT_API_KEY` | Bearer token | unset |
| `SIDESEAT_DISABLED` | Disable recording and export (`1`, `true`, or `yes`) | `false` |

`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` and `OTEL_EXPORTER_OTLP_ENDPOINT` are accepted
when `SIDESEAT_ENDPOINT` is unset. Endpoint routing is deterministic:

```text
http://localhost:5388
  -> http://localhost:5388/otel/{projectId}/v1/traces

http://collector:4318/otel/my-project
  -> http://collector:4318/otel/my-project/v1/traces

http://collector:4318/otel/my-project/v1/traces
  -> unchanged
```

Set `ExportTraces = false` if your application supplies another OpenTelemetry exporter
through `ConfigureTracerProvider`.

## Errors and shutdown

Record exceptions as structured OpenTelemetry events:

```csharp
using var span = sideSeat.StartSpan("tool-call");
try
{
    await CallToolAsync();
}
catch (Exception error)
{
    span.RecordException(error);
    throw;
}
```

Dispose the client during application shutdown. `Dispose()` flushes queued spans; call
`ForceFlush()` explicitly before a short-lived process exits when you need a synchronous result.

## Resources

- [Documentation](https://sideseat.ai/docs)
- [GitHub](https://github.com/sideseat/sideseat)
- [Issues](https://github.com/sideseat/sideseat/issues)

## License

[MIT](LICENSE)
