# SideSeat for .NET

OpenTelemetry for AI agents in .NET: one call configures tracing, metrics, and logs for a [SideSeat](https://sideseat.ai)
project, switches on your framework's GenAI telemetry, and attributes every span to the right session
and user.

```bash
dotnet add package SideSeat
dotnet add package AWSSDK.Extensions.Bedrock.MEAI
npx sideseat            # a local SideSeat server on http://127.0.0.1:5388
```

## Quick start

```csharp
using Amazon.BedrockRuntime;
using Microsoft.Extensions.AI;
using SideSeat;

using var sideseat = SideSeatClient.Create(new SideSeatOptions
{
    Integrations = ["extensions-ai"],
});

IChatClient chat = new AmazonBedrockRuntimeClient()
    .AsIChatClient("global.anthropic.claude-sonnet-5-5")
    .AsBuilder()
    .UseOpenTelemetry()
    .Build();

using (sideseat.Session("conversation-42", userId: "user-7"))
{
    await chat.GetResponseAsync("Plan a weekend in Lisbon.");
    await chat.GetResponseAsync("What should I eat there?");
}
```

A session scope attributes every activity started inside it - including those your framework creates -
to the session and user, across `await`. It creates no span of its own, and the identifiers never leave
the process as W3C baggage.

## Integrations

| Name | Library | What SideSeat does |
| --- | --- | --- |
| `extensions-ai` | Microsoft.Extensions.AI | Listens to its activity source; wrap chat clients with `UseOpenTelemetry()`. |
| `agent-framework` | Microsoft Agent Framework | Listens to its agent and chat sources. |
| `semantic-kernel` | Semantic Kernel | Switches on its GenAI diagnostics, including content. |
| `openai` | OpenAI .NET | Switches on its experimental OpenTelemetry support. |

Content capture is on by default (`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT` and each library's
own switch); set `CaptureContent = false` to turn it off everywhere.

## Structure

```csharp
using (sideseat.StartTrace("plan-trip", sessionId: "s-1", userId: "u-1"))
using (var span = sideseat.StartSpan("retrieve-context"))
{
    span.SetAttribute("app.documents", 4);
}
```

`StartTrace` always starts a root span; `StartSpan` starts a child of `Activity.Current`.

## An existing OpenTelemetry pipeline

```csharp
builder.Services.AddOpenTelemetry().WithTracing(tracing => tracing.AddSideSeat(new SideSeatOptions
{
    Integrations = ["semantic-kernel"],
}));

using (new SideSeatSession("conversation-42", userId: "user-7"))
{
    await kernel.InvokePromptAsync("Plan a weekend in Lisbon.");
}
```

## Configuration

| Property | Environment | Default |
| --- | --- | --- |
| `Endpoint` | `SIDESEAT_ENDPOINT`, `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://127.0.0.1:5388` |
| `Project` | `SIDESEAT_PROJECT_ID` | `default` |
| `ApiKey` | `SIDESEAT_API_KEY` | none |
| `ServiceName` | `OTEL_SERVICE_NAME` | the primary integration's package, else `sideseat-app` |
| `Integrations` | `SIDESEAT_INTEGRATIONS` | detected; `[]` for none |
| `CaptureContent` | `SIDESEAT_CAPTURE_CONTENT` | `true` |
| `Disabled` | `SIDESEAT_DISABLED` | `false` |

`Flush()` and `Shutdown()` return whether every exporter finished; disposing the client shuts it down,
and so does process exit. `Create` returns the running client for the same options and throws for
different ones. `sideseat.LoggerFactory` creates loggers whose records are exported too. Every option is
at https://sideseat.ai/docs/sdks/dotnet/.

## License

MIT
