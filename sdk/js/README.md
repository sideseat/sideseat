# @sideseat/sdk

OpenTelemetry for AI agents in Node.js: one call configures tracing, metrics, and logs for a
[SideSeat](https://sideseat.ai) project, switches on your framework's telemetry, and attributes every
span to the right session and user.

```bash
npm install @sideseat/sdk
npx sideseat            # a local SideSeat server on http://127.0.0.1:5388
```

## Quick start

```ts
import * as sideseat from "@sideseat/sdk";
import { Agent } from "@strands-agents/sdk";

await sideseat.init({ integrations: ["strands"] });

const agent = new Agent({ model: "global.anthropic.claude-sonnet-5-5" });

await sideseat.session({ sessionId: "conversation-42", userId: "user-7" }, async () => {
  await agent.invoke("Plan a weekend in Lisbon.");
  await agent.invoke("What should I eat there?");
});

await sideseat.shutdown();
```

`session()` attributes every span started inside the callback - including the spans your framework
creates - to the session and user. It creates no span of its own. The identifiers stay inside the
process: they are never sent as W3C baggage, which HTTP instrumentation would forward to model
providers.

## Integrations

| Name | Framework | Notes |
| --- | --- | --- |
| `strands` | `@strands-agents/sdk` | Emits through the global tracer provider. |
| `vercel-ai` | `ai` (AI SDK 7+) | Registers `@ai-sdk/otel`; install it alongside `ai`. Calls still need `experimental_telemetry: { isEnabled: true }`. |
| `claude-agent-sdk` | `@anthropic-ai/claude-agent-sdk` | Configures the Claude Code CLI it spawns through inherited environment variables, removed at shutdown. If you pass `options.env`, spread `process.env` into it. |

Without `integrations`, the installed framework is detected. Pass `[]` for none.

## Structure

```ts
await sideseat.trace("plan-trip", { sessionId: "s-1", userId: "u-1" }, async (span) => {
  await sideseat.span("retrieve-context", async () => loadDocuments());
});
```

`trace()` always starts a root span; `span()` starts a child of the active span. Both record an
exception and mark the span as failed when the callback throws.

## Configuration

| Option | Environment | Default |
| --- | --- | --- |
| `endpoint` | `SIDESEAT_ENDPOINT`, `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://127.0.0.1:5388` |
| `project` | `SIDESEAT_PROJECT_ID` | `default` |
| `apiKey` | `SIDESEAT_API_KEY` | none |
| `serviceName` | `OTEL_SERVICE_NAME` | the framework's package name, else `sideseat-app` |
| `integrations` | `SIDESEAT_INTEGRATIONS` | detected |
| `captureContent` | `SIDESEAT_CAPTURE_CONTENT` | `true` |
| `disabled` | `SIDESEAT_DISABLED` | `false` |
| `debug` | `SIDESEAT_DEBUG` | `false` |

The full option list is at https://sideseat.ai/docs/sdks/typescript/configuration/. An endpoint
without a path is a SideSeat server; one with a path is used as the OTLP base. Calling
`init` twice with the same options returns the same client; different options reject with
`ConfigurationError`.

`flush()` and `shutdown()` resolve to whether all telemetry was exported; export failures never throw. Shutdown also runs before the
process exits and on `SIGINT`/`SIGTERM`.

## Testing

```ts
import { capture } from "@sideseat/sdk/testing";

const spans = await capture({ integrations: [] }, () =>
  sideseat.session({ sessionId: "s-1" }, () => runAgent()),
);
```

## License

MIT
