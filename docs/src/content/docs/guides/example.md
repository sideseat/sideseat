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

### Configure Your AI Agent

SideSeat accepts traces via the standard OpenTelemetry Protocol (OTLP). Configure your agent to send traces to:

- **HTTP**: `http://localhost:5001/otel/v1/traces`
- **gRPC**: `localhost:4317`

### Python with OpenTelemetry

```python
from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter

# Configure exporter
exporter = OTLPSpanExporter(endpoint="http://localhost:5001/otel/v1/traces")
provider = TracerProvider()
provider.add_span_processor(BatchSpanProcessor(exporter))
trace.set_tracer_provider(provider)

# Create traces
tracer = trace.get_tracer(__name__)
with tracer.start_as_current_span("my-agent-operation"):
    # Your agent code here
    pass
```

### Using LangChain

LangChain has built-in OpenTelemetry support:

```python
from langchain_core.tracers import LangChainTracer
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter

# Configure OTLP exporter
exporter = OTLPSpanExporter(endpoint="http://localhost:5001/otel/v1/traces")

# Use with your LangChain agent
# SideSeat automatically detects LangChain spans
```

### Using Strands

```python
from strands import Agent
from strands.telemetry import OTLPExporter

exporter = OTLPExporter(endpoint="http://localhost:5001/otel/v1/traces")

agent = Agent(
    model="anthropic/claude-sonnet-4-20250514",
    telemetry_exporter=exporter
)
```

## Viewing Traces

1. Open the SideSeat dashboard at `http://localhost:5001/ui`
2. Traces appear in real-time as they're received
3. Click a trace to see its spans and details
4. Use filters to find specific traces by service, framework, or attributes

## Real-time Streaming

Subscribe to trace events programmatically:

```javascript
const eventSource = new EventSource('http://localhost:5001/api/v1/traces/sse');

eventSource.onmessage = (event) => {
  const payload = JSON.parse(event.data);
  console.log('Event:', payload.event.type);
};
```

## Next Steps

- [Configure](/reference/config/) server settings, storage limits, and retention policies
- [OpenTelemetry Collector](/reference/otel/) reference for advanced configuration
- [Authentication](/reference/auth/) for securing your instance
