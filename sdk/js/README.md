# SideSeat TypeScript SDK

**AI Development Workbench** — Debug, trace, and understand your AI agents.

[![npm](https://img.shields.io/npm/v/@sideseat/sdk)](https://www.npmjs.com/package/@sideseat/sdk)
[![Node 18+](https://img.shields.io/badge/node-18%2B-blue)](https://nodejs.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

## Table of Contents

- [What is SideSeat?](#what-is-sideseat)
- [Quick Start](#quick-start)
- [Installation](#installation)
- [Framework Examples](#framework-examples)
- [Configuration](#configuration)
- [Advanced Usage](#advanced-usage)
- [Upgrading to 2.0](#upgrading-to-20)
- [Data and Privacy](#data-and-privacy)
- [Troubleshooting](#troubleshooting)
- [API Reference](#api-reference)

## What is SideSeat?

AI agents are hard to debug. Requests fly by, context builds up, and when something fails you're left guessing.

SideSeat captures every LLM call, tool call, and agent decision, then displays them in a web UI as they happen. Run it locally during development, or deploy to your private cloud for team visibility.

Built on [OpenTelemetry](https://opentelemetry.io/) — the open standard for observability.

**Features:**

- **Real-time tracing** — Watch LLM requests and tool calls as they happen
- **Message threading** — See full conversations, tool calls, and images
- **Cost tracking** — Automatic token counting and cost calculation

**Supported frameworks:** Vercel AI SDK, Strands (TypeScript), and any framework emitting OpenTelemetry traces

## Quick Start

**Requirements:** Node.js 18+

**1. Start the server**

```bash
npx sideseat
```

**2. Install and initialize**

```bash
npm install ai @ai-sdk/otel @ai-sdk/amazon-bedrock @sideseat/sdk
```

```typescript
import { init, Frameworks } from '@sideseat/sdk';
import { generateText, registerTelemetry } from 'ai';
import { LegacyOpenTelemetry } from '@ai-sdk/otel';
import { bedrock } from '@ai-sdk/amazon-bedrock';

init({ framework: Frameworks.VercelAI });

// AI SDK 7 emits spans only through a registered integration. Without this the per-call
// experimental_telemetry flag produces nothing. Register after init(): the integration
// captures a tracer in its constructor.
registerTelemetry(new LegacyOpenTelemetry());

const { text } = await generateText({
  model: bedrock('us.anthropic.claude-sonnet-4-5-20250929-v1:0'),
  prompt: 'What is 2+2?',
  experimental_telemetry: { isEnabled: true },
});

console.log(text);
```

**3. View traces**

Open [localhost:5388](http://localhost:5388) and run your agent. Traces appear in real time.

## Installation

```bash
npm install @sideseat/sdk
```

## Framework Examples

### Strands (TypeScript)

```bash
npm install @strands-agents/sdk @sideseat/sdk
```

```typescript
import { init, Frameworks } from '@sideseat/sdk';
import { Agent } from '@strands-agents/sdk';

init({ framework: Frameworks.Strands });

const agent = new Agent({ model: 'global.anthropic.claude-haiku-4-5-20251001-v1:0' });
const result = await agent.invoke('What is 2+2?');
console.log(result.toString());
```

### Vercel AI SDK

AI SDK 7 hands telemetry to registered integrations rather than emitting OpenTelemetry
spans itself, so two things are needed: `registerTelemetry(new LegacyOpenTelemetry())`
once at startup, and `experimental_telemetry: { isEnabled: true }` on each call. On AI
SDK 6 the per-call flag alone was enough.

Use `LegacyOpenTelemetry` rather than `OpenTelemetry` — SideSeat's framework detection
keys on the `ai.generateText` / `ai.*.doGenerate` span shape it preserves.

```bash
npm install @sideseat/sdk ai @ai-sdk/otel @ai-sdk/amazon-bedrock
```

```typescript
import { init, shutdown, Frameworks } from '@sideseat/sdk';
import { generateText, generateObject, tool, registerTelemetry } from 'ai';
import { LegacyOpenTelemetry } from '@ai-sdk/otel';
import { bedrock } from '@ai-sdk/amazon-bedrock';
import { z } from 'zod';

init({ framework: Frameworks.VercelAI });

// After init(): the integration captures a tracer in its constructor.
registerTelemetry(new LegacyOpenTelemetry());

// Text generation
const { text } = await generateText({
  model: bedrock('us.anthropic.claude-sonnet-4-5-20250929-v1:0'),
  prompt: 'What is the capital of France?',
  experimental_telemetry: { isEnabled: true },
});

// Structured output
const { object } = await generateObject({
  model: bedrock('us.anthropic.claude-sonnet-4-5-20250929-v1:0'),
  schema: z.object({ name: z.string(), age: z.number() }),
  prompt: 'Generate a person',
  experimental_telemetry: { isEnabled: true },
});

// Tool use
const weatherTool = tool({
  description: 'Get weather for a city',
  parameters: z.object({ city: z.string() }),
  execute: async ({ city }) => ({ temp: 72, condition: 'sunny' }),
});

const { text: weatherText } = await generateText({
  model: bedrock('us.anthropic.claude-sonnet-4-5-20250929-v1:0'),
  tools: { weather: weatherTool },
  prompt: 'What is the weather in Paris?',
  experimental_telemetry: { isEnabled: true },
});

// Flush traces before exit
await shutdown();
```

**Important:** Always include `experimental_telemetry: { isEnabled: true }` on each `generateText`, `generateObject`, or `streamText` call.

### Without SideSeat SDK

Manual OpenTelemetry setup for full control:

```typescript
import { NodeSDK } from '@opentelemetry/sdk-node';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http';

const sdk = new NodeSDK({ traceExporter: new OTLPTraceExporter() });
sdk.start();

import { generateText } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';

const { text } = await generateText({
  model: bedrock('us.anthropic.claude-sonnet-4-5-20250929-v1:0'),
  prompt: 'What is 2+2?',
  experimental_telemetry: { isEnabled: true },
});

console.log(text);
```

Set the endpoint:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:5388/otel/default
```

## Configuration

### Environment Variables

| Variable              | Default                 | Description                                    |
| --------------------- | ----------------------- | ---------------------------------------------- |
| `SIDESEAT_ENDPOINT`   | `http://127.0.0.1:5388` | Server URL                                     |
| `SIDESEAT_PROJECT_ID` | `default`               | Project identifier                             |
| `SIDESEAT_API_KEY`    | —                       | Authentication key                             |
| `SIDESEAT_DISABLED`   | `false`                 | Disable all telemetry                          |
| `SIDESEAT_DEBUG`      | `false`                 | Enable verbose logging                         |
| `SIDESEAT_LOG_LEVEL`  | `none`                  | Log level (none/error/warn/info/debug/verbose) |

Standard OpenTelemetry variables are also honoured:

| Variable                       | Description                                              |
| ------------------------------ | -------------------------------------------------------- |
| `OTEL_SERVICE_NAME`            | Override service name                                    |
| `OTEL_EXPORTER_OTLP_ENDPOINT`  | Endpoint fallback, after `SIDESEAT_ENDPOINT`              |
| `OTEL_EXPORTER_OTLP_HEADERS`   | Extra headers, merged with the API-key header             |
| `OTEL_EXPORTER_OTLP_TIMEOUT`   | Export timeout in **milliseconds** (default `30000`)      |

> **Units:** OpenTelemetry JS reads `OTEL_EXPORTER_OTLP_TIMEOUT` as milliseconds, while
> OpenTelemetry Python reads it as seconds. Both SDKs follow their own ecosystem's convention,
> so `30000` here and `30` in the Python SDK mean the same thing. This is an upstream
> inconsistency, not a SideSeat one.

### Constructor Options

```typescript
init({
  endpoint: 'http://localhost:5388',
  projectId: 'my-project',
  apiKey: 'pk-...',
  framework: Frameworks.VercelAI,
  serviceName: 'my-app',
  serviceVersion: '1.0.0',
  enableTraces: true,
  logLevel: 'debug',
  disabled: false,
  debug: false,
});
```

| Parameter        | Type       | Default                 | Description                |
| ---------------- | ---------- | ----------------------- | -------------------------- |
| `endpoint`       | `string`   | `http://127.0.0.1:5388` | Server URL                 |
| `projectId`      | `string`   | `default`               | Project identifier         |
| `apiKey`         | `string`   | `undefined`             | Authentication key         |
| `framework`      | `string`   | —                       | Framework identifier (**required**) |
| `serviceName`    | `string`   | `npm_package_name`      | Application name in traces |
| `serviceVersion` | `string`   | `npm_package_version`   | Application version        |
| `enableTraces`   | `boolean`  | `true`                  | Export trace spans         |
| `logLevel`       | `LogLevel` | `none`                  | OpenTelemetry log level    |
| `disabled`       | `boolean`  | `false`                 | Disable all telemetry      |
| `debug`          | `boolean`  | `false`                 | Enable verbose logging     |

**Resolution order:** Constructor → `SIDESEAT_*` env → `OTEL_*` env → defaults

## Advanced Usage

### Async Initialization

Use `createClient()` for async initialization with connection validation:

```typescript
import { createClient, Frameworks } from '@sideseat/sdk';

const client = await createClient({
  framework: Frameworks.VercelAI,
  projectId: 'my-project',
});
// Connection validated before returning
```

### Global Instance

```typescript
import { init, getClient, shutdown, isInitialized, Frameworks } from '@sideseat/sdk';

init({ framework: Frameworks.VercelAI, projectId: 'my-project' }); // Initialize once
const client = getClient(); // Access anywhere
await shutdown(); // Clean up
```

### Custom Spans

```typescript
const client = init({ framework: Frameworks.VercelAI });

// Async spans
const result = await client.span('process-request', async (span) => {
  span.setAttribute('user_id', '12345');
  return await doWork();
});

// Sync spans
const value = client.spanSync('compute', (span) => {
  span.setAttribute('input', 42);
  return calculate();
});
// Exceptions recorded automatically with stack traces
```

### Debug Exporters

```typescript
const client = init({ framework: Frameworks.VercelAI });
client.setupConsoleExporter(); // Print to stdout
client.setupFileExporter('traces.jsonl'); // Write to file
```

### Disabled Mode

```typescript
init({ framework: Frameworks.VercelAI, disabled: true }); // Or set SIDESEAT_DISABLED=true
```

### Existing OpenTelemetry Setup

If another library has already registered the global `TracerProvider`, SideSeat cannot add
its exporter to it — OpenTelemetry 2.x only accepts span processors at construction, and the
API refuses a second global registration. SideSeat creates its own provider instead, so its
own spans (`client.span`, `client.spanSync`, `client.getTracer`) are still exported; spans
created by other instrumentation through the global tracer are not. A warning is logged.
Initialize SideSeat first if you need those spans too.

### Direct Class Usage

For multiple independent instances:

```typescript
import { SideSeat, Frameworks } from '@sideseat/sdk';

const client1 = new SideSeat({ framework: Frameworks.VercelAI, projectId: 'project-a' });
const client2 = new SideSeat({ framework: Frameworks.Strands, projectId: 'project-b' });
```

## Upgrading to 2.0

2.0 moves the SDK onto the **OpenTelemetry JS 2.x** SDK packages. `@opentelemetry/api`
stays on 1.x, so context propagation with other instrumentation is unaffected.

**What changed for you:**

- **`framework` is now required in the type.** It was already required at runtime — the
  constructor threw `SideSeatError` without it — but `SideSeatOptions.framework` was typed
  optional, so omitting it compiled and only failed when the process ran. `init()`,
  `createClient()`, `new SideSeat()` and `Config.create()` now all require an options object
  carrying `framework`. Plain-JavaScript callers still get the same runtime error.
- **`client.addSpanProcessor(processor)` still works** and is the supported way to add
  exporters. OpenTelemetry 2.x removed `TracerProvider.addSpanProcessor`, so if you
  reached through to `client.tracerProvider.addSpanProcessor(...)` directly, switch to
  the client method.
- **Custom `SpanProcessor` implementations** must target OTel 2.x types. `ReadableSpan`
  replaced `parentSpanId` with `parentSpanContext?: SpanContext` and
  `instrumentationLibrary` with `instrumentationScope`.
- **If another library registered a global `TracerProvider` before SideSeat**, the OTel
  API refuses to hand the global over. SideSeat's own spans (`client.span`,
  `client.spanSync`, `client.getTracer`) are still exported — they go through SideSeat's
  own provider. What is lost is spans created by *other* instrumentation via the global
  tracer: those keep going to whoever registered first. A warning is logged. Initialize
  SideSeat before that library if you need those spans too.

`spanToDict()` output is unchanged, including the `parent_span_id` key.

## Data and Privacy

**What is collected:**

- Trace spans with timing and hierarchy
- LLM prompts and responses
- Token counts and model names
- Errors and stack traces

**Where it goes:**

All data is sent to your self-hosted server. Nothing leaves your infrastructure.

**Resilience:**

- Up to 2,048 spans buffered in memory
- Batched exports every 5 seconds
- 30-second timeout per export
- Server downtime does not affect your application

## Troubleshooting

| Problem            | Solution                                                   |
| ------------------ | ---------------------------------------------------------- |
| Connection refused | Server not running. Run `npx sideseat`                     |
| No traces appear   | Check `experimental_telemetry: { isEnabled: true }` is set |
| Duplicate traces   | Initialize `init()` once per process                       |
| Import errors      | Ensure Node.js 18+ and ESM/CJS compatibility               |

## API Reference

### Module Functions

| Function                 | Returns             | Description                    |
| ------------------------ | ------------------- | ------------------------------ |
| `init(options)`          | `SideSeat`          | Create global instance (sync)  |
| `createClient(options)`  | `Promise<SideSeat>` | Create global instance (async) |
| `getClient()`            | `SideSeat`          | Get global instance            |
| `shutdown()`             | `Promise<void>`     | Shut down global instance      |
| `isInitialized()`        | `boolean`           | Check if initialized           |

### SideSeat Class

```typescript
const client = new SideSeat(options);
```

**Properties:**

| Name             | Type                 | Description                   |
| ---------------- | -------------------- | ----------------------------- |
| `config`         | `Config`             | Immutable configuration       |
| `tracerProvider` | `NodeTracerProvider \| null` | OpenTelemetry tracer provider; `null` when disabled |
| `isDisabled`     | `boolean`            | Whether telemetry is disabled |
| `isReady`        | `boolean`            | Whether client is ready       |

**Methods:**

| Name                             | Returns            | Description                       |
| -------------------------------- | ------------------ | --------------------------------- |
| `span(name, fn)`                 | `Promise<T>`       | Create an async span              |
| `spanSync(name, fn)`             | `T`                | Create a sync span                |
| `getTracer(name?, version?)`     | `Tracer`           | Get an OpenTelemetry tracer       |
| `forceFlush(timeoutMs?)`         | `Promise<boolean>` | Export pending spans immediately  |
| `validateConnection(timeoutMs?)` | `Promise<boolean>` | Test server connectivity          |
| `shutdown(timeoutMs?)`           | `Promise<void>`    | Flush pending spans and shut down |
| `setupConsoleExporter()`         | `this`             | Add console exporter              |
| `setupFileExporter(path?)`       | `this`             | Add JSONL file exporter           |
| `addSpanProcessor(processor)`    | `this`             | Add custom span processor         |

### Frameworks

Frameworks instrumented by this SDK:

```typescript
Frameworks.Strands        // "strands"
Frameworks.VercelAI       // "vercel-ai"
Frameworks.ClaudeAgentSDK // "claude-agent-sdk"
```

The remaining constants exist so a Node process can tag spans with the same identifier the
Python SDK uses — useful in a polyglot system, but they do not add instrumentation here:

```typescript
Frameworks.LangChain    // "langchain"
Frameworks.LangGraph    // "langgraph"
Frameworks.CrewAI       // "crewai"
Frameworks.AutoGen      // "autogen"
Frameworks.AG2          // "ag2"
Frameworks.OpenAIAgents // "openai-agents"
Frameworks.GoogleADK    // "google-adk"
Frameworks.PydanticAI   // "pydantic-ai"
Frameworks.AgentFramework // "agent-framework"
Frameworks.Agno         // "agno"
Frameworks.Smolagents   // "smolagents"
Frameworks.AgentScope   // "agentscope"
Frameworks.Langflow     // "langflow"
Frameworks.Haystack     // "haystack"
Frameworks.BrowserUse   // "browser-use"

// Providers
Frameworks.Bedrock      // "bedrock"
Frameworks.Anthropic    // "anthropic"
Frameworks.OpenAI       // "openai"
Frameworks.GoogleGenAI  // "google-genai"
Frameworks.VertexAI     // "vertex-ai"
```

Any string is also accepted, so a framework absent from this list can still be named.

### Utilities

| Export                 | Description                            |
| ---------------------- | -------------------------------------- |
| `encodeValue(value)`   | JSON-encode a value; base64 for binary |
| `spanToDict(span)`     | Convert span to dictionary             |
| `JsonFileSpanExporter` | JSONL file exporter class              |
| `SideSeatError`        | SDK error class                        |
| `VERSION`              | SDK version string                     |

## Resources

- [Documentation](https://sideseat.ai/docs)
- [GitHub Discussions](https://github.com/sideseat/sideseat/discussions)
- [Issue Tracker](https://github.com/sideseat/sideseat/issues)

## License

[MIT](LICENSE)
