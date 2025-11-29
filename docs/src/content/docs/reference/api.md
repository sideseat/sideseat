---
title: REST API Reference
description: Complete REST API reference for SideSeat trace query, session management, and real-time streaming endpoints.
---

This page documents all REST API endpoints for querying traces, spans, sessions, and subscribing to real-time updates.

## Base URL

All API endpoints are prefixed with `/api/v1`.

```
http://localhost:5001/api/v1
```

## Authentication

Most endpoints require authentication when auth is enabled. See [Authentication](/reference/auth/) for details.

## Pagination

List endpoints use cursor-based pagination for efficient traversal of large datasets.

**Query Parameters:**
- `limit` - Maximum items to return (default: 50, max: 100)
- `cursor` - Opaque cursor string from previous response

**Response Fields:**
- `next_cursor` - Cursor for next page (null if no more pages)
- `has_more` - Boolean indicating if more pages exist

**Example:**
```bash
# First page
curl "http://localhost:5001/api/v1/traces?limit=20"

# Next page using cursor
curl "http://localhost:5001/api/v1/traces?limit=20&cursor=eyJ0cyI6MTcwMzAwMTIzNCwiaWQiOiJ0cmFjZS0xMjMifQ"
```

---

## Traces

### List Traces

```
GET /api/v1/traces
```

Query traces with optional filters and pagination.

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `service` | string | Filter by service name |
| `framework` | string | Filter by detected framework (langchain, strands, etc.) |
| `agent` | string | Filter by agent name |
| `errors_only` | boolean | Only return traces with errors |
| `search` | string | Full-text search on trace_id and service_name |
| `attributes` | JSON string | Attribute filters (see below) |
| `cursor` | string | Pagination cursor |
| `limit` | number | Max results (default: 50, max: 100) |

**Attribute Filter Format:**
```json
[{"key": "environment", "op": "eq", "value": "production"}]
```

**Response:**
```json
{
  "traces": [
    {
      "trace_id": "abc123def456",
      "session_id": "session-789",
      "root_span_id": "span-001",
      "root_span_name": "agent.invoke",
      "service_name": "my-agent",
      "detected_framework": "strands",
      "span_count": 5,
      "start_time_ns": 1703001234567890000,
      "end_time_ns": 1703001235567890000,
      "duration_ns": 1000000000,
      "total_input_tokens": 150,
      "total_output_tokens": 200,
      "total_tokens": 350,
      "has_errors": false
    }
  ],
  "next_cursor": "eyJ0cyI6MTcwMzAwMTIzNCwiaWQiOiJhYmMxMjMifQ",
  "has_more": true
}
```

**Examples:**
```bash
# List recent traces
curl http://localhost:5001/api/v1/traces

# Filter by service
curl "http://localhost:5001/api/v1/traces?service=my-agent"

# Filter by framework
curl "http://localhost:5001/api/v1/traces?framework=langchain"

# Only traces with errors
curl "http://localhost:5001/api/v1/traces?errors_only=true"

# Search traces
curl "http://localhost:5001/api/v1/traces?search=abc123"

# Filter by attribute
curl "http://localhost:5001/api/v1/traces?attributes=%5B%7B%22key%22%3A%22environment%22%2C%22op%22%3A%22eq%22%2C%22value%22%3A%22production%22%7D%5D"
```

### Get Trace

```
GET /api/v1/traces/{trace_id}
```

Get a single trace with all indexed attributes.

**Response:**
```json
{
  "trace_id": "abc123def456",
  "session_id": "session-789",
  "root_span_id": "span-001",
  "root_span_name": "agent.invoke",
  "service_name": "my-agent",
  "detected_framework": "strands",
  "span_count": 5,
  "start_time_ns": 1703001234567890000,
  "end_time_ns": 1703001235567890000,
  "duration_ns": 1000000000,
  "total_input_tokens": 150,
  "total_output_tokens": 200,
  "total_tokens": 350,
  "has_errors": false,
  "attributes": {
    "environment": "production",
    "user.id": "user-123"
  }
}
```

**Errors:**
- `404` - Trace not found

### Delete Trace

```
DELETE /api/v1/traces/{trace_id}
```

Delete a trace and all associated spans, events, and attributes.

**Response:**
- `204 No Content` - Successfully deleted
- `404 Not Found` - Trace not found

### Get Filter Options

```
GET /api/v1/traces/filters
```

Get available filter values for building UI dropdowns.

**Response:**
```json
{
  "services": ["my-agent", "data-processor"],
  "frameworks": ["strands", "langchain", "openai"],
  "attributes": [
    {
      "key": "environment",
      "key_type": "string",
      "entity_type": "trace",
      "sample_values": ["production", "staging", "development"]
    },
    {
      "key": "user.id",
      "key_type": "string",
      "entity_type": "trace",
      "sample_values": ["user-123", "user-456"]
    }
  ]
}
```

---

## Spans

### List Spans

```
GET /api/v1/spans
```

Query spans with optional filters.

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `trace_id` | string | Filter by trace ID |
| `service` | string | Filter by service name |
| `framework` | string | Filter by detected framework |
| `category` | string | Filter by detected category (llm, tool, etc.) |
| `agent` | string | Filter by agent name |
| `tool` | string | Filter by tool name |
| `model` | string | Filter by request model |
| `cursor` | string | Pagination cursor |
| `limit` | number | Max results (default: 100, max: 1000) |

**Response:**
```json
{
  "spans": [
    {
      "span_id": "span-001",
      "trace_id": "abc123def456",
      "session_id": "session-789",
      "parent_span_id": null,
      "span_name": "agent.invoke",
      "service_name": "my-agent",
      "detected_framework": "strands",
      "detected_category": "agent",
      "gen_ai_system": "anthropic",
      "gen_ai_operation_name": "chat",
      "gen_ai_agent_name": "my-agent",
      "gen_ai_tool_name": null,
      "gen_ai_request_model": "claude-3-opus",
      "gen_ai_response_model": "claude-3-opus",
      "start_time_ns": 1703001234567890000,
      "end_time_ns": 1703001235567890000,
      "duration_ns": 1000000000,
      "time_to_first_token_ms": 150,
      "request_duration_ms": 1000,
      "status_code": 0,
      "usage_input_tokens": 150,
      "usage_output_tokens": 200,
      "usage_total_tokens": 350,
      "usage_cache_read_tokens": null,
      "usage_cache_write_tokens": null
    }
  ],
  "next_cursor": "eyJ0cyI6MTcwMzAwMTIzNCwiaWQiOiJzcGFuLTAwMSJ9",
  "has_more": true
}
```

**Examples:**
```bash
# Get all spans for a trace
curl "http://localhost:5001/api/v1/spans?trace_id=abc123def456"

# Filter by model
curl "http://localhost:5001/api/v1/spans?model=claude-3-opus"

# Filter LLM spans only
curl "http://localhost:5001/api/v1/spans?category=llm"
```

### Get Span

```
GET /api/v1/spans/{span_id}
```

Get a single span with full data and events.

**Response:**
```json
{
  "span_id": "span-001",
  "trace_id": "abc123def456",
  "session_id": "session-789",
  "parent_span_id": null,
  "span_name": "agent.invoke",
  "service_name": "my-agent",
  "detected_framework": "strands",
  "detected_category": "agent",
  "gen_ai_system": "anthropic",
  "gen_ai_operation_name": "chat",
  "gen_ai_agent_name": "my-agent",
  "gen_ai_tool_name": null,
  "gen_ai_request_model": "claude-3-opus",
  "gen_ai_response_model": "claude-3-opus",
  "start_time_ns": 1703001234567890000,
  "end_time_ns": 1703001235567890000,
  "duration_ns": 1000000000,
  "time_to_first_token_ms": 150,
  "request_duration_ms": 1000,
  "status_code": 0,
  "usage_input_tokens": 150,
  "usage_output_tokens": 200,
  "usage_total_tokens": 350,
  "usage_cache_read_tokens": null,
  "usage_cache_write_tokens": null,
  "data": {
    "attributes": { "custom.field": "value" },
    "resource": { "service.name": "my-agent" }
  },
  "events": [
    {
      "id": 1,
      "span_id": "span-001",
      "trace_id": "abc123def456",
      "event_name": "gen_ai.user.message",
      "event_time_ns": 1703001234567890000,
      "event_type": "user_message",
      "role": "user",
      "finish_reason": null,
      "content_preview": "What is the weather today?",
      "tool_name": null,
      "tool_call_id": null
    }
  ]
}
```

### Get Span Events

```
GET /api/v1/spans/{span_id}/events
```

Get all events for a span.

**Response:**
```json
{
  "events": [
    {
      "id": 1,
      "span_id": "span-001",
      "trace_id": "abc123def456",
      "event_name": "gen_ai.user.message",
      "event_time_ns": 1703001234567890000,
      "event_type": "user_message",
      "role": "user",
      "finish_reason": null,
      "content_preview": "What is the weather today?",
      "tool_name": null,
      "tool_call_id": null,
      "attributes": { "custom.field": "value" }
    },
    {
      "id": 2,
      "span_id": "span-001",
      "trace_id": "abc123def456",
      "event_name": "gen_ai.assistant.message",
      "event_time_ns": 1703001235567890000,
      "event_type": "assistant_message",
      "role": "assistant",
      "finish_reason": "end_turn",
      "content_preview": "The weather today is sunny with...",
      "tool_name": null,
      "tool_call_id": null,
      "attributes": {}
    }
  ]
}
```

---

## Sessions

Sessions group traces that share a common `session.id` attribute, useful for tracking multi-turn conversations.

### List Sessions

```
GET /api/v1/sessions
```

Query sessions with optional filters.

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `user_id` | string | Filter by user ID |
| `service` | string | Filter by service name |
| `cursor` | string | Pagination cursor |
| `limit` | number | Max results (default: 50, max: 100) |

**Response:**
```json
{
  "sessions": [
    {
      "session_id": "session-789",
      "user_id": "user-123",
      "service_name": "my-agent",
      "trace_count": 5,
      "span_count": 25,
      "total_input_tokens": 1500,
      "total_output_tokens": 2000,
      "total_tokens": 3500,
      "has_errors": false,
      "first_seen_ns": 1703001234567890000,
      "last_seen_ns": 1703005234567890000,
      "duration_ns": 4000000000000
    }
  ],
  "next_cursor": "eyJ0cyI6MTcwMzAwMTIzNCwiaWQiOiJzZXNzaW9uLTc4OSJ9",
  "has_more": true
}
```

### Get Session

```
GET /api/v1/sessions/{session_id}
```

Get a single session's details.

**Response:**
```json
{
  "session_id": "session-789",
  "user_id": "user-123",
  "service_name": "my-agent",
  "trace_count": 5,
  "span_count": 25,
  "total_input_tokens": 1500,
  "total_output_tokens": 2000,
  "total_tokens": 3500,
  "has_errors": false,
  "first_seen_ns": 1703001234567890000,
  "last_seen_ns": 1703005234567890000,
  "duration_ns": 4000000000000
}
```

### Delete Session

```
DELETE /api/v1/sessions/{session_id}
```

Delete a session and all associated traces, spans, and events.

**Response:**
- `204 No Content` - Successfully deleted
- `404 Not Found` - Session not found

### Get Session Traces

```
GET /api/v1/sessions/{session_id}/traces
```

Get all traces for a session.

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `cursor` | string | Pagination cursor |
| `limit` | number | Max results (default: 50, max: 100) |

**Response:**
```json
{
  "traces": [
    {
      "trace_id": "abc123def456",
      "root_span_name": "agent.invoke",
      "span_count": 5,
      "start_time_ns": 1703001234567890000,
      "duration_ns": 1000000000,
      "has_errors": false
    }
  ],
  "next_cursor": null,
  "has_more": false
}
```

---

## Real-time Streaming

### SSE Stream

```
GET /api/v1/traces/sse
```

Subscribe to real-time trace events via Server-Sent Events (SSE).

**Event Types:**

| Event | Description |
|-------|-------------|
| `NewSpan` | A new span was received |
| `SpanUpdated` | An existing span was updated |
| `TraceCompleted` | A trace finished (all spans ended) |
| `HealthUpdate` | Collector health status changed |

**Event Format:**
```
data: {"event":{"type":"NewSpan","data":{"span_id":"span-001","trace_id":"abc123"}}}

data: {"event":{"type":"TraceCompleted","data":{"trace_id":"abc123"}}}
```

**JavaScript Example:**
```javascript
const eventSource = new EventSource('http://localhost:5001/api/v1/traces/sse');

eventSource.onmessage = (event) => {
  const payload = JSON.parse(event.data);

  switch (payload.event.type) {
    case 'NewSpan':
      console.log('New span:', payload.event.data);
      break;
    case 'TraceCompleted':
      console.log('Trace completed:', payload.event.data.trace_id);
      break;
    case 'HealthUpdate':
      console.log('Health:', payload.event.data);
      break;
  }
};

eventSource.onerror = (error) => {
  console.error('SSE error:', error);
  eventSource.close();
};
```

**Connection Limits:**
- Maximum connections: 100 (configurable)
- Timeout: 1 hour (configurable)
- Keepalive: 30 seconds (configurable)

---

## Health Check

### Get Health

```
GET /api/v1/health
```

Returns server health status including OTel collector status.

**Response:**
```json
{
  "status": "healthy",
  "version": "1.0.4",
  "otel": {
    "enabled": true,
    "status": "healthy",
    "metrics": {
      "total_traces": 1234,
      "total_spans": 5678
    }
  }
}
```

---

## Error Responses

All error responses follow a consistent format:

```json
{
  "error": "error_type",
  "code": "ERROR_CODE",
  "message": "Human-readable error message"
}
```

**Common Error Codes:**

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `TRACE_NOT_FOUND` | 404 | Requested trace does not exist |
| `SPAN_NOT_FOUND` | 404 | Requested span does not exist |
| `SESSION_NOT_FOUND` | 404 | Requested session does not exist |
| `STORAGE_ERROR` | 500 | Database or storage error |
| `AUTH_REQUIRED` | 401 | Authentication required |
| `TOKEN_EXPIRED` | 401 | JWT token has expired |

---

## Attribute Filtering

The trace query API supports filtering by indexed attributes using the EAV (Entity-Attribute-Value) pattern.

### Filter Operators

| Operator | Description | Value Type |
|----------|-------------|------------|
| `eq` | Equals | string |
| `ne` | Not equals | string |
| `contains` | Contains substring | string |
| `starts_with` | Starts with | string |
| `in` | In list | string[] |
| `gt` | Greater than | number |
| `lt` | Less than | number |
| `gte` | Greater than or equal | number |
| `lte` | Less than or equal | number |
| `is_null` | Attribute not present | null |
| `is_not_null` | Attribute present | null |

### Filter Examples

**Single filter:**
```json
[{"key": "environment", "op": "eq", "value": "production"}]
```

**Multiple filters (AND):**
```json
[
  {"key": "environment", "op": "eq", "value": "production"},
  {"key": "user.id", "op": "eq", "value": "user-123"}
]
```

**In list filter:**
```json
[{"key": "environment", "op": "in", "value": ["production", "staging"]}]
```

**Numeric comparison:**
```json
[{"key": "latency_ms", "op": "gt", "value": 1000}]
```

**Check attribute exists:**
```json
[{"key": "error.message", "op": "is_not_null", "value": null}]
```

### URL Encoding

When passing filters via URL query parameter, the JSON must be URL-encoded:

```bash
# Original: [{"key":"environment","op":"eq","value":"production"}]
# Encoded:
curl "http://localhost:5001/api/v1/traces?attributes=%5B%7B%22key%22%3A%22environment%22%2C%22op%22%3A%22eq%22%2C%22value%22%3A%22production%22%7D%5D"
```
