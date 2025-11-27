---
title: OpenTelemetry Collector
description: Built-in OTLP-compatible trace collector for AI agent observability.
---

SideSeat includes a built-in OpenTelemetry collector optimized for AI agent development workflows. It receives OTLP traces via HTTP and gRPC, stores them in Parquet files for efficient querying, and provides real-time streaming via SSE.

## Architecture

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   AI Agents     │────▶│   OTel Ingest   │────▶│  Write Buffer   │
│  (OTLP traces)  │     │  HTTP/gRPC/SSE  │     │  (bounded mem)  │
└─────────────────┘     └─────────────────┘     └────────┬────────┘
                                                         │
                        ┌─────────────────┐              │
                        │   SSE Clients   │◀─────────────┤
                        │  (real-time)    │              │
                        └─────────────────┘              ▼
                                                ┌─────────────────┐
                                                │  Parquet Files  │
                                                │  (FIFO storage) │
                                                └─────────────────┘
```

## Features

- **OTLP-compatible**: Receives traces via standard OpenTelemetry protocol (HTTP JSON/Protobuf, gRPC)
- **Framework detection**: Automatically detects LangChain, LlamaIndex, Strands, and other AI frameworks
- **GenAI field extraction**: Extracts token usage, model info, and other GenAI-specific fields
- **Bounded memory**: Configurable buffer limits prevent memory exhaustion
- **FIFO storage**: Automatic cleanup when storage limits are reached
- **Real-time streaming**: SSE endpoint for live trace updates
- **Efficient storage**: Parquet columnar format for fast queries

## Endpoints

### Trace Ingestion

| Endpoint | Method | Content-Type | Description |
|----------|--------|--------------|-------------|
| `/v1/traces` | POST | `application/json` | OTLP JSON traces |
| `/v1/traces` | POST | `application/x-protobuf` | OTLP Protobuf traces |
| `0.0.0.0:4317` | gRPC | Protobuf | OTLP gRPC endpoint |

### Query API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/traces` | GET | List traces with filtering |
| `/api/traces/:trace_id` | GET | Get single trace with spans |
| `/api/traces/:trace_id/spans` | GET | Get spans for a trace |
| `/api/spans/:span_id` | GET | Get single span details |

### Real-time Streaming

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/traces/stream` | GET | SSE stream of new traces |

## Configuration

All OTel settings are under the `otel` key in your config file:

```json
{
  "otel": {
    "enabled": true,
    "grpc_enabled": true,
    "grpc_port": 4317,
    "retention_max_gb": 20
  }
}
```

See [Config Manager](/reference/config/#otel-config) for the full configuration reference.

## Sending Traces

### Python with OpenTelemetry SDK

```python
from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter

# Configure exporter to send to SideSeat
exporter = OTLPSpanExporter(endpoint="http://localhost:5001/v1/traces")
provider = TracerProvider()
provider.add_span_processor(BatchSpanProcessor(exporter))
trace.set_tracer_provider(provider)

# Create traces
tracer = trace.get_tracer(__name__)
with tracer.start_as_current_span("my-agent-operation"):
    # Your agent code here
    pass
```

### Python with Strands SDK

```python
from strands import Agent
from strands.telemetry import OTLPExporter

# Configure Strands to export to SideSeat
exporter = OTLPExporter(endpoint="http://localhost:5001/v1/traces")

agent = Agent(
    model="anthropic/claude-sonnet-4-20250514",
    telemetry_exporter=exporter
)
```

### Node.js with OpenTelemetry SDK

```javascript
const { NodeTracerProvider } = require('@opentelemetry/sdk-trace-node');
const { OTLPTraceExporter } = require('@opentelemetry/exporter-trace-otlp-http');
const { BatchSpanProcessor } = require('@opentelemetry/sdk-trace-base');

const exporter = new OTLPTraceExporter({
  url: 'http://localhost:5001/v1/traces',
});

const provider = new NodeTracerProvider();
provider.addSpanProcessor(new BatchSpanProcessor(exporter));
provider.register();
```

### Using gRPC

For higher throughput, use the gRPC endpoint:

```python
from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import OTLPSpanExporter

exporter = OTLPSpanExporter(endpoint="localhost:4317", insecure=True)
```

## Framework Detection

SideSeat automatically detects and normalizes spans from popular AI frameworks:

| Framework | Detection Method | Extracted Fields |
|-----------|------------------|------------------|
| LangChain | Scope name, attributes | Chain type, run ID |
| LangGraph | Scope name, attributes | Node, edge, state |
| LlamaIndex | Scope name, attributes | Query, response |
| Strands | Scope name, resource attrs | Cycle ID, agent info |
| OpenInference | Attribute prefix | Session ID, user ID |
| Generic GenAI | `gen_ai.*` attributes | Model, tokens, system |

## GenAI Fields

The collector extracts and normalizes GenAI-specific fields:

| Field | Description |
|-------|-------------|
| `gen_ai_system` | AI provider (openai, anthropic, etc.) |
| `gen_ai_request_model` | Requested model name |
| `gen_ai_response_model` | Actual model used |
| `usage_input_tokens` | Input/prompt tokens |
| `usage_output_tokens` | Output/completion tokens |
| `gen_ai_operation_name` | Operation type (chat, completion) |

## Storage

Traces are stored in Parquet files under `data/traces/`:

```
data/traces/
├── spans_20241127_143022_abc123.parquet
├── spans_20241127_144533_def456.parquet
└── ...
```

### Retention

Storage is managed with FIFO (First-In-First-Out) deletion:

- **Size-based**: When `retention_max_gb` is exceeded, oldest files are deleted
- **Time-based**: Optional `retention_days` deletes files older than N days
- **Automatic**: Retention runs every `retention_check_interval_secs` (default: 5 min)

### Disk Safety

The collector monitors disk usage and protects against filling the disk:

- **Warning** (80%): Logs warning, continues operation
- **Critical** (95%): Stops accepting new traces until space is freed

## Real-time Streaming

Subscribe to new traces via Server-Sent Events:

```javascript
const eventSource = new EventSource('http://localhost:5001/api/traces/stream');

eventSource.onmessage = (event) => {
  const trace = JSON.parse(event.data);
  console.log('New trace:', trace.trace_id);
};
```

### SSE Limits

- Maximum connections: 100 (configurable)
- Connection timeout: 1 hour (configurable)
- Keepalive interval: 30 seconds (configurable)

## Query Examples

### List Recent Traces

```bash
curl http://localhost:5001/api/traces
```

### Filter by Service

```bash
curl "http://localhost:5001/api/traces?service=my-agent"
```

### Get Trace Details

```bash
curl http://localhost:5001/api/traces/abc123def456
```

## Troubleshooting

### Traces Not Appearing

1. Check OTel is enabled: `"otel": { "enabled": true }`
2. Verify endpoint URL matches your exporter configuration
3. Check server logs for ingestion errors

### High Memory Usage

Reduce buffer sizes in config:

```json
{
  "otel": {
    "channel_capacity": 500,
    "buffer_max_spans": 500,
    "buffer_max_bytes": 5242880
  }
}
```

### Disk Full

1. Reduce `retention_max_gb`
2. Enable `retention_days` for time-based cleanup
3. Manually delete old files in `data/traces/`
