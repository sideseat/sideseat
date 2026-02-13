# SideSeat JavaScript SDK

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

**Supported frameworks:** Vercel AI SDK, Strands Agents, LangChain, CrewAI, AutoGen, OpenAI Agents, Google ADK, PydanticAI

## Quick Start

**Requirements:** Node.js 18+

**1. Start the server**

```bash
npx sideseat
```

**2. Install and initialize**

```bash
npm install ai @ai-sdk/amazon-bedrock @sideseat/sdk
```

```typescript
import { init } from '@sideseat/sdk';
import { generateText } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';

init();

const { text } = await generateText({
  model: bedrock('anthropic.claude-sonnet-4-5-20250929-v1:0'),
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

### Vercel AI SDK

Vercel AI SDK has built-in OpenTelemetry support via `experimental_telemetry`. Enable it on each call:

```typescript
import { init, shutdown } from '@sideseat/sdk';
import { generateText, generateObject, tool } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';
import { z } from 'zod';

init();

// Text generation
const { text } = await generateText({
  model: bedrock('anthropic.claude-sonnet-4-5-20250929-v1:0'),
  prompt: 'What is the capital of France?',
  experimental_telemetry: { isEnabled: true },
});

// Structured output
const { object } = await generateObject({
  model: bedrock('anthropic.claude-sonnet-4-5-20250929-v1:0'),
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
  model: bedrock('anthropic.claude-sonnet-4-5-20250929-v1:0'),
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
  model: bedrock('anthropic.claude-sonnet-4-5-20250929-v1:0'),
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
| `framework`      | `string`   | `sideseat`              | Framework identifier       |
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
import { createClient } from '@sideseat/sdk';

const client = await createClient({ projectId: 'my-project' });
// Connection validated before returning
```

### Global Instance

```typescript
import { init, getClient, shutdown, isInitialized } from '@sideseat/sdk';

init({ projectId: 'my-project' }); // Initialize once
const client = getClient(); // Access anywhere
await shutdown(); // Clean up
```

### Custom Spans

```typescript
const client = init();

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
const client = init();
client.setupConsoleExporter(); // Print to stdout
client.setupFileExporter('traces.jsonl'); // Write to file
```

### Disabled Mode

```typescript
init({ disabled: true }); // Or set SIDESEAT_DISABLED=true
```

### Existing OpenTelemetry Setup

If a `TracerProvider` already exists, SideSeat adds its exporter to the existing provider.

### Direct Class Usage

For multiple independent instances:

```typescript
import { SideSeat } from '@sideseat/sdk';

const client1 = new SideSeat({ projectId: 'project-a' });
const client2 = new SideSeat({ projectId: 'project-b' });
```

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
| `init(options?)`         | `SideSeat`          | Create global instance (sync)  |
| `createClient(options?)` | `Promise<SideSeat>` | Create global instance (async) |
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
| `tracerProvider` | `NodeTracerProvider` | OpenTelemetry tracer provider |
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

```typescript
Frameworks.VercelAI    // "vercel-ai"
Frameworks.Strands     // "strands"
Frameworks.LangChain   // "langchain"
Frameworks.CrewAI      // "crewai"
Frameworks.AutoGen     // "autogen"
Frameworks.OpenAIAgents // "openai-agents"
Frameworks.GoogleADK   // "google-adk"
Frameworks.PydanticAI  // "pydantic-ai"
```

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
