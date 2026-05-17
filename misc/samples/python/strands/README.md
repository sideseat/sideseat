# Strands Samples

## Setup

```bash
cp .env.example .env
# Edit .env with your API keys
uv sync
```

## Commands

```bash
# Run specific sample
uv run strands tool_use
uv run strands mcp_tools
uv run strands structured_output
uv run strands reasoning
uv run strands swarm
uv run strands files
uv run strands rag_local
uv run strands image_gen
uv run strands agent_core

# Run all samples
uv run strands all

# List available samples and models
uv run strands --list
```

## Model Selection

```bash
# Bedrock (default)
uv run strands tool_use --model bedrock-haiku
uv run strands tool_use --model bedrock-sonnet
uv run strands tool_use --model bedrock-nova

# Anthropic direct API
uv run strands tool_use --model anthropic-haiku
uv run strands tool_use --model anthropic-sonnet

# OpenAI
uv run strands tool_use --model openai-gpt5nano

# Gemini
uv run strands tool_use --model gemini-flash
```

## Telemetry

```bash
# Default: StrandsTelemetry
uv run strands tool_use

# SideSeat telemetry with binary encoding
uv run strands tool_use --sideseat
```

## Environment Variables

| Variable                      | Required         | Description                     |
| ----------------------------- | ---------------- | ------------------------------- |
| `AWS_REGION`                  | No               | AWS region (default: us-east-1) |
| `ANTHROPIC_API_KEY`           | For anthropic-\* | Anthropic API key               |
| `OPENAI_API_KEY`              | For openai-\*    | OpenAI API key                  |
| `GOOGLE_API_KEY`              | For gemini-\*    | Google API key                  |
| `AGENT_CORE_MEMORY_ID`        | For agent_core   | AWS AgentCore memory ID         |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | No               | OTLP endpoint                   |

## Extended Thinking (Reasoning)

The `reasoning` sample demonstrates extended thinking capabilities for supported models.
Extended thinking enables chain-of-thought reasoning with visible thinking steps.

```bash
# Run with extended thinking enabled (Bedrock Sonnet recommended)
uv run strands reasoning --model bedrock-sonnet

# Also works with Anthropic direct API
uv run strands reasoning --model anthropic-sonnet
```

Supported models for extended thinking:

- `bedrock-sonnet`, `bedrock-haiku` (via `additional_request_fields`)
- `anthropic-sonnet`, `anthropic-haiku` (via `thinking` parameter)

Models without extended thinking support will still work but won't show thinking steps.

## SDK WebSocket bridge (presence + introspection)

`strands_ws` demonstrates the persistent SideSeat WebSocket bridge. The
script registers an agent on the server and then blocks; list who is
online with:

```bash
curl http://127.0.0.1:5388/api/v1/project/default/registrations
```

```bash
uv run strands strands_ws --sideseat
```

The sample sets the `gen_ai.agent.name` trace attribute to the same name
it registers under, so spans and registrations cross-reference cleanly.

For Strands `Swarm` and `Graph`, `client.register([swarm])` /
`client.register([graph])` records the composite plus every inner
`Agent` automatically — the server returns them under `swarms` /
`graphs` and `agents` respectively in the listing.
