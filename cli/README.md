# SideSeat

**AI Development Workbench** — Debug, trace, and understand your AI agents.

[![npm](https://img.shields.io/npm/v/sideseat)](https://www.npmjs.com/package/sideseat)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue)](https://github.com/sideseat/sideseat/blob/main/LICENSE)

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

SDKs: [Python (PyPI)](https://pypi.org/project/sideseat/) | [JavaScript (npm)](https://www.npmjs.com/package/@sideseat/sdk)

## Features

- **Real-time tracing** — Watch LLM requests and tool calls as they happen
- **Message threading** — See full conversations, tool calls, and images
- **Cost tracking** — Automatic token counting and cost calculation

## Usage

```bash
sideseat              # Start with defaults
sideseat --port 8080  # Custom port
sideseat --no-auth    # Disable authentication
sideseat --help       # Show all options
```

Requires Node.js 18+ on macOS, Linux, or Windows.

## Compatibility

**Agent frameworks** — Strands Agents, LangGraph, CrewAI, AutoGen, Google ADK, OpenAI Agents, LangChain, PydanticAI

**LLM providers** — OpenAI, Anthropic, AWS Bedrock, Azure OpenAI, Google Gemini

**Telemetry** — Vercel AI SDK, OpenInference, MLflow, Logfire

## Resources

- [Documentation](https://sideseat.ai/docs)
- [GitHub](https://github.com/sideseat/sideseat)
- [Issues](https://github.com/sideseat/sideseat/issues)

## License

[AGPL-3.0](https://github.com/sideseat/sideseat/blob/main/LICENSE)
