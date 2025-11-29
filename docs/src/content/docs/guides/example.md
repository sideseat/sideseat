---
title: Getting Started
description: Set up SideSeat and start collecting traces from your AI agents.
---

This guide walks you through setting up SideSeat and collecting your first traces from an AI agent.

## Prerequisites

- Rust 1.75 or higher
- Node.js 20.19+ or 22.12+
- Make

## Installation

### Clone and Build

```bash
# Clone the repository
git clone https://github.com/spugachev/sideseat.git
cd sideseat

# Install dependencies and build
make setup
```

### Start the Server

```bash
# Start in development mode
make dev
```

You'll see output like:

```
SideSeat v1.0.4
  Local: http://127.0.0.1:5001/ui?token=abc123...
```

Click the URL to open the dashboard in your browser.

## Collecting Traces

SideSeat accepts traces via the standard OpenTelemetry Protocol (OTLP). Configure your agent to send traces to:

- **HTTP**: `http://localhost:5001/otel/v1/traces`
- **gRPC**: `localhost:4317`

### Python with Strands SDK

[Strands](https://strandsagents.com) is an AI agent framework with built-in OpenTelemetry support:

```python
from strands import Agent
from strands.models import BedrockModel
from strands.telemetry import StrandsTelemetry

# Configure telemetry to export to SideSeat
telemetry = StrandsTelemetry()
telemetry.setup_otlp_exporter(endpoint="http://localhost:5001/otel/v1/traces")

# Create model
model = BedrockModel(model_id="us.anthropic.claude-haiku-4-5-20251001-v1:0")

# Create agent with trace attributes for filtering
agent = Agent(
    name="my-assistant",
    model=model,
    trace_attributes={
        "session.id": "conversation-123",  # Group traces by session
        "user.id": "user-456",            # Track by user
        "environment": "development",      # Filter by environment
    },
)

# Run the agent
response = agent("What's the capital of France?")
print(response)

# Important: Flush telemetry before exiting
telemetry.tracer_provider.force_flush()
```

### Python with OpenTelemetry SDK

For any Python application, use the OpenTelemetry SDK directly:

```python
from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.resources import Resource

# Configure resource attributes
resource = Resource.create({
    "service.name": "my-ai-agent",
    "deployment.environment": "development",
})

# Configure exporter
exporter = OTLPSpanExporter(endpoint="http://localhost:5001/otel/v1/traces")
provider = TracerProvider(resource=resource)
provider.add_span_processor(BatchSpanProcessor(exporter))
trace.set_tracer_provider(provider)

# Create tracer
tracer = trace.get_tracer(__name__)

# Create traces with GenAI attributes
with tracer.start_as_current_span("llm.completion") as span:
    span.set_attribute("gen_ai.system", "openai")
    span.set_attribute("gen_ai.request.model", "gpt-4")
    span.set_attribute("gen_ai.operation.name", "chat")

    # Your LLM call here
    response = call_openai()

    # Record token usage
    span.set_attribute("gen_ai.usage.input_tokens", 150)
    span.set_attribute("gen_ai.usage.output_tokens", 200)
```

### Using gRPC (Higher Throughput)

For higher throughput, use the gRPC endpoint:

```python
from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import OTLPSpanExporter

# gRPC exporter (insecure for local dev)
exporter = OTLPSpanExporter(endpoint="localhost:4317", insecure=True)
```

### Node.js with OpenTelemetry SDK

```javascript
const { NodeTracerProvider } = require('@opentelemetry/sdk-trace-node');
const { OTLPTraceExporter } = require('@opentelemetry/exporter-trace-otlp-http');
const { BatchSpanProcessor } = require('@opentelemetry/sdk-trace-base');
const { Resource } = require('@opentelemetry/resources');

// Configure resource
const resource = new Resource({
  'service.name': 'my-ai-agent',
  'deployment.environment': 'development',
});

// Configure exporter
const exporter = new OTLPTraceExporter({
  url: 'http://localhost:5001/otel/v1/traces',
});

// Set up provider
const provider = new NodeTracerProvider({ resource });
provider.addSpanProcessor(new BatchSpanProcessor(exporter));
provider.register();

// Create tracer
const tracer = provider.getTracer('my-ai-agent');

// Create spans
const span = tracer.startSpan('llm.completion');
span.setAttribute('gen_ai.system', 'anthropic');
span.setAttribute('gen_ai.request.model', 'claude-3-opus');
// ... your code ...
span.end();
```

## Viewing Traces

1. Open the SideSeat dashboard at `http://localhost:5001/ui`
2. Traces appear in real-time as they're received
3. Click a trace to see its spans and details
4. Use filters to find specific traces by service, framework, or attributes

### Filtering Traces

Use the filter panel to narrow down traces:

- **Service**: Filter by service name (e.g., `my-ai-agent`)
- **Framework**: Filter by detected framework (strands, langchain, openai)
- **Errors Only**: Show only traces with errors
- **Search**: Full-text search on trace IDs and service names
- **Attributes**: Filter by indexed attributes (environment, user.id, etc.)

### Understanding Span Data

Each span shows:

- **Basic Info**: Name, service, framework, duration
- **GenAI Fields**: Model, system, tokens used, TTFT
- **Events**: User messages, assistant responses, tool calls
- **Attributes**: Custom fields you've attached

## Real-time Streaming

Subscribe to trace events programmatically using Server-Sent Events:

```javascript
const eventSource = new EventSource('http://localhost:5001/api/v1/traces/sse');

eventSource.onmessage = (event) => {
  const payload = JSON.parse(event.data);

  switch (payload.event.type) {
    case 'NewSpan':
      console.log('New span received:', payload.event.data);
      break;
    case 'TraceCompleted':
      console.log('Trace finished:', payload.event.data.trace_id);
      break;
  }
};

eventSource.onerror = (error) => {
  console.error('Connection error:', error);
  eventSource.close();
};
```

## Using Sessions

Sessions group related traces (e.g., a multi-turn conversation). Set the `session.id` attribute on your traces:

```python
# Strands
agent = Agent(
    model=model,
    trace_attributes={"session.id": "conversation-abc123"}
)

# OpenTelemetry SDK
span.set_attribute("session.id", "conversation-abc123")
```

Then view sessions in the dashboard or query via API:

```bash
# List all sessions
curl http://localhost:5001/api/v1/sessions

# Get traces for a session
curl http://localhost:5001/api/v1/sessions/conversation-abc123/traces
```

## Querying the API

SideSeat provides a REST API for programmatic access:

```bash
# List recent traces
curl http://localhost:5001/api/v1/traces

# Filter by service
curl "http://localhost:5001/api/v1/traces?service=my-ai-agent"

# Get a specific trace
curl http://localhost:5001/api/v1/traces/abc123def456

# Get spans for a trace
curl "http://localhost:5001/api/v1/spans?trace_id=abc123def456"

# Get filter options (for building UIs)
curl http://localhost:5001/api/v1/traces/filters
```

See the [API Reference](/reference/api/) for complete documentation.

## Configuration

### Custom Port

```bash
# CLI flag
sideseat start --port 8080

# Environment variable
SIDESEAT_PORT=8080 make dev
```

### Disable Authentication

For development, you can disable authentication:

```bash
sideseat start --no-auth
```

### Set Retention

Configure how long to keep trace data:

```json
// sideseat.json
{
  "otel": {
    "retention": {
      "days": 7
    }
  }
}
```

See [Configuration](/reference/config/) for all options.

## Troubleshooting

### Traces Not Appearing

1. **Check the endpoint URL** - Ensure your exporter points to `http://localhost:5001/otel/v1/traces`
2. **Flush before exit** - Call `force_flush()` on your tracer provider before the process exits
3. **Check server logs** - Look for ingestion errors in the terminal running SideSeat
4. **Verify OTel is enabled** - Check that `otel.enabled` is `true` in your config

### High Memory Usage

Reduce buffer sizes if memory is a concern:

```json
{
  "otel": {
    "ingestion": {
      "buffer_max_spans": 1000,
      "buffer_max_bytes": 5242880
    }
  }
}
```

### Connection Refused

1. Ensure SideSeat is running
2. Check the port isn't blocked by a firewall
3. For gRPC, ensure you're using the correct port (4317 by default)

## Next Steps

- [OpenTelemetry Collector](/reference/otel/) - Advanced configuration for trace collection
- [REST API Reference](/reference/api/) - Complete API documentation
- [Configuration](/reference/config/) - All configuration options
- [Authentication](/reference/auth/) - Secure your instance
