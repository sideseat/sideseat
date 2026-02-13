<h1 align="center">SideSeat</h1>

<p align="center">
  <strong>AI Development Workbench</strong><br>
  Debug, trace, and understand your AI agents.
</p>

<p align="center">
  <a href="https://github.com/sideseat/sideseat/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-blue" alt="License" /></a>
  <a href="https://www.npmjs.com/package/sideseat"><img src="https://img.shields.io/npm/v/sideseat" alt="npm" /></a>
  <a href="https://pypi.org/project/sideseat/"><img src="https://img.shields.io/pypi/v/sideseat" alt="PyPI" /></a>
</p>

<p align="center">
  <img src="misc/screenshots/screenshot_1.png" alt="SideSeat showing an AI agent conversation with tool calls" width="800" />
</p>

## What is SideSeat?

AI agents are hard to debug. Requests fly by, context builds up, and when something fails you're left guessing.

SideSeat captures every LLM call, tool call, and agent decision, then displays them in a web UI as they happen. Run it locally during development, or deploy to your private cloud for team visibility.

Built on [OpenTelemetry](https://opentelemetry.io/) — the open standard already supported by most AI frameworks.

## Quick Start

```bash
npx sideseat
```

Open [localhost:5388](http://localhost:5388) and instrument your agent. Here's [Strands Agents](https://strandsagents.com) as an example — see the [setup guide](https://sideseat.ai/docs) for Vercel AI, Google ADK, LangGraph, CrewAI, AutoGen, OpenAI Agents, and others.

**With SideSeat SDK** — automatic setup, one import:

```bash
pip install strands-agents sideseat
# or
uv add strands-agents sideseat
```

```python
from sideseat import SideSeat, Frameworks
from strands import Agent

SideSeat(framework=Frameworks.Strands)

agent = Agent()
response = agent("What is 2+2?")
print(response)
```

**Without SideSeat SDK** — manual OpenTelemetry setup:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:5388/otel/default
pip install 'strands-agents[otel]'
```

```python
from strands.telemetry import StrandsTelemetry
from strands import Agent

telemetry = StrandsTelemetry()
telemetry.setup_otlp_exporter()

agent = Agent()
response = agent("What is 2+2?")
print(response)
```

SDKs: [Python (PyPI)](https://pypi.org/project/sideseat/) | [TypeScript (npm)](https://www.npmjs.com/package/@sideseat/sdk)

## Features

- **Real-time tracing** — Watch LLM requests and tool calls as they happen
- **Message threading** — See full conversations, tool calls, and images
- **Cost tracking** — Automatic token counting and cost calculation

<p align="center">
  <img src="misc/screenshots/screenshot_2.png" alt="Detailed view showing message threading" width="800" />
</p>

<p align="center">
  <img src="misc/screenshots/screenshot_3.png" alt="Cost analytics and token usage breakdown" width="800" />
</p>

## AI Agent Development with MCP

SideSeat includes a built-in [MCP](https://modelcontextprotocol.io/) server that gives AI coding agents direct access to your agent's execution history — prompts sent, responses received, tool calls made, costs incurred, and errors encountered.

Connect your coding tool and let it optimize prompts, debug failures, and reduce costs using real observability data instead of guesswork.

```bash
# Kiro CLI
kiro-cli mcp add --name sideseat --url http://localhost:5388/api/v1/projects/default/mcp

# Claude Code
claude mcp add --transport http sideseat http://localhost:5388/api/v1/projects/default/mcp

# OpenAI Codex
codex mcp add --transport http sideseat http://localhost:5388/api/v1/projects/default/mcp
```

Config file for Kiro, Cursor, and other MCP clients:

```json
{
  "mcpServers": {
    "sideseat": {
      "url": "http://localhost:5388/api/v1/projects/default/mcp"
    }
  }
}
```

See the [MCP docs](https://sideseat.ai/docs/mcp/) for all setup options.

Then ask your coding agent:

> Look at my last 5 agent runs in SideSeat. Find any that errored or had high token usage. Show me the system prompts and suggest improvements.

7 tools are available: `list_traces`, `list_sessions`, `list_spans`, `get_trace`, `get_messages`, `get_raw_span`, `get_stats`. See the [MCP docs](https://sideseat.ai/docs/mcp/) for setup guides for Kiro, Claude Code, Codex, Cursor, and other clients.

## Deployment

**Local** — Run on your machine with `npx sideseat`. Data never leaves your computer.

**Self-hosted** — Deploy to your private cloud for team-wide observability. See the [configuration guide](https://sideseat.ai/docs/reference/config/).

## Compatibility

**Agent frameworks** — Strands Agents, LangGraph, CrewAI, AutoGen, Google ADK, OpenAI Agents, LangChain, PydanticAI

**LLM providers** — OpenAI, Anthropic, AWS Bedrock, Azure OpenAI, Google Vertex AI

**Telemetry** — Vercel AI SDK, OpenInference, MLflow, Logfire

## Resources

- **[Documentation](https://sideseat.ai/docs)** — Setup, configuration, API reference
- **[Discussions](https://github.com/sideseat/sideseat/discussions)** — Questions and ideas
- **[Issues](https://github.com/sideseat/sideseat/issues)** — Bug reports
- **[Contributing](CONTRIBUTING.md)** — Development guide

## License

[AGPL-3.0](LICENSE) — Free to use and modify. Distribute modified versions under the same license.
