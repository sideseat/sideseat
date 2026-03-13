# Misc

Shared resources, sample applications, and utilities for SideSeat development.

## Setup

```bash
cp misc/.env.example misc/.env                               # Configure environment
npm --prefix misc/samples/js install                         # Install JS deps
uv sync --directory misc/replay                              # Install replay deps

# Install each Python sample package individually (isolated envs)
uv sync --directory misc/samples/python/strands
uv sync --directory misc/samples/python/adk
uv sync --directory misc/samples/python/langgraph
uv sync --directory misc/samples/python/crewai
uv sync --directory misc/samples/python/autogen
uv sync --directory misc/samples/python/openai-agents
uv sync --directory misc/samples/python/agent-framework
uv sync --directory misc/samples/python/bedrock
uv sync --directory misc/samples/python/anthropic
uv sync --directory misc/samples/python/openai
uv sync --directory misc/samples/python/loadtest

# After SDK structural changes (new deps, new extras, pyproject.toml edits)
for d in strands adk langgraph crewai autogen openai-agents agent-framework bedrock anthropic openai loadtest; do
  uv sync --directory misc/samples/python/$d --reinstall-package sideseat
done
```

## Python Samples

Start SideSeat first: `make dev-server`

View traces: http://localhost:5389/ui/projects/default/observability/traces

### All Frameworks

```bash
uv run --directory misc/samples/python/strands strands tool_use
uv run --directory misc/samples/python/adk telemetry-adk tool_use
uv run --directory misc/samples/python/langgraph langgraph tool_use
uv run --directory misc/samples/python/openai-agents openai-agents tool_use
uv run --directory misc/samples/python/agent-framework agent-framework tool_use
uv run --directory misc/samples/python/autogen autogen tool_use
uv run --directory misc/samples/python/crewai crewai tool_use
uv run --directory misc/samples/python/openai openai-provider chat_completions
uv run --directory misc/samples/python/openai openai-provider responses
uv run --directory misc/samples/python/anthropic anthropic-provider messages
uv run --directory misc/samples/python/bedrock bedrock converse
npm --prefix misc/samples/js run vercel-ai -- tool-use
npm --prefix misc/samples/js run strands -- tool-use
```

### Strands Agents

```bash
uv run --directory misc/samples/python/strands strands                        # List samples and models
uv run --directory misc/samples/python/strands strands tool_use               # Tool usage
uv run --directory misc/samples/python/strands strands mcp_tools              # MCP server integration
uv run --directory misc/samples/python/strands strands structured_output      # Structured data extraction
uv run --directory misc/samples/python/strands strands reasoning              # Extended thinking
uv run --directory misc/samples/python/strands strands files                  # Image/PDF analysis
uv run --directory misc/samples/python/strands strands image_gen              # Image generation
uv run --directory misc/samples/python/strands strands rag_local              # RAG with embeddings
uv run --directory misc/samples/python/strands strands swarm                  # Multi-agent swarm
uv run --directory misc/samples/python/strands strands agent_core             # AgentCore integration
uv run --directory misc/samples/python/strands strands error                  # Error handling
uv run --directory misc/samples/python/strands strands all                    # Run all
```

### OpenAI Provider

```bash
uv run --directory misc/samples/python/openai openai-provider                # List samples and models
uv run --directory misc/samples/python/openai openai-provider chat_completions  # Sync, streaming, tool use
uv run --directory misc/samples/python/openai openai-provider responses      # Responses API
uv run --directory misc/samples/python/openai openai-provider multi_turn     # Multi-turn (trace grouping)
uv run --directory misc/samples/python/openai openai-provider vision         # Image analysis
uv run --directory misc/samples/python/openai openai-provider session        # Session with multiple traces
uv run --directory misc/samples/python/openai openai-provider error          # Error handling
uv run --directory misc/samples/python/openai openai-provider all            # Run all
```

### Anthropic Provider

```bash
uv run --directory misc/samples/python/anthropic anthropic-provider             # List samples and models
uv run --directory misc/samples/python/anthropic anthropic-provider messages    # Sync, streaming, tool use
uv run --directory misc/samples/python/anthropic anthropic-provider multi_turn  # Multi-turn (trace grouping)
uv run --directory misc/samples/python/anthropic anthropic-provider thinking    # Extended thinking
uv run --directory misc/samples/python/anthropic anthropic-provider vision      # Image analysis
uv run --directory misc/samples/python/anthropic anthropic-provider document    # PDF analysis
uv run --directory misc/samples/python/anthropic anthropic-provider session     # Session with multiple traces
uv run --directory misc/samples/python/anthropic anthropic-provider error       # Error handling
uv run --directory misc/samples/python/anthropic anthropic-provider all         # Run all
```

### Bedrock Provider

```bash
uv run --directory misc/samples/python/bedrock bedrock                        # List samples and models
uv run --directory misc/samples/python/bedrock bedrock converse               # Sync, streaming, thinking, tool use
uv run --directory misc/samples/python/bedrock bedrock multi_turn             # Multi-turn (trace grouping)
uv run --directory misc/samples/python/bedrock bedrock invoke_model           # InvokeModel API
uv run --directory misc/samples/python/bedrock bedrock document               # PDF + image multimodal
uv run --directory misc/samples/python/bedrock bedrock session                # Session with multiple traces
uv run --directory misc/samples/python/bedrock bedrock error                  # Error handling
uv run --directory misc/samples/python/bedrock bedrock all                    # Run all
```

### Options

Framework samples (Strands, LangGraph, etc.):

```bash
--sideseat                # Enable SideSeat SDK telemetry
--model <alias>           # Select model (see aliases below)
--list                    # List available samples and models
```

Provider samples (OpenAI, Anthropic, Bedrock) always use SideSeat SDK — no `--sideseat` flag needed. They accept `--model` and `--list`.

### Model Aliases

| Alias              | Provider                           |
| ------------------ | ---------------------------------- |
| `bedrock-haiku`    | AWS Bedrock Claude Haiku (default) |
| `bedrock-sonnet`   | AWS Bedrock Claude Sonnet          |
| `bedrock-nova`     | AWS Bedrock Nova 2 Lite            |
| `anthropic-haiku`  | Anthropic API Claude Haiku         |
| `anthropic-sonnet` | Anthropic API Claude Sonnet        |
| `openai-gpt5nano`  | OpenAI GPT-5 Nano                  |
| `gemini-flash`     | Google Gemini Flash                |

Default model varies by sample: Strands/LangGraph/CrewAI/ADK/Bedrock use `bedrock-haiku`, AutoGen uses `anthropic-haiku`, OpenAI Agents/Microsoft Agent Framework/OpenAI provider use `openai-gpt5nano`, Anthropic provider uses `anthropic-haiku`.

### Load Testing

```bash
uv run --directory misc/samples/python/loadtest loadtest              # Default: 1M spans
uv run --directory misc/samples/python/loadtest loadtest --spans 100000  # Custom span count
```

## JavaScript Samples

### Vercel AI SDK

```bash
npm --prefix misc/samples/js run vercel-ai -- --list            # List samples and models
npm --prefix misc/samples/js run vercel-ai -- tool-use          # Tool usage
npm --prefix misc/samples/js run vercel-ai -- structured-output # Structured data extraction
npm --prefix misc/samples/js run vercel-ai -- files             # Image analysis
npm --prefix misc/samples/js run vercel-ai -- image-gen         # Image generation
npm --prefix misc/samples/js run vercel-ai -- rag-local         # RAG with embeddings
npm --prefix misc/samples/js run vercel-ai -- reasoning         # Chain-of-thought
npm --prefix misc/samples/js run vercel-ai -- multi-step        # Agentic loop
npm --prefix misc/samples/js run vercel-ai -- all               # Run all
```

### Strands Agents (JS)

```bash
npm --prefix misc/samples/js run strands -- --list              # List samples and models
npm --prefix misc/samples/js run strands -- tool-use            # Tool usage
npm --prefix misc/samples/js run strands -- mcp-tools           # MCP server integration
npm --prefix misc/samples/js run strands -- structured-output   # Structured data extraction
npm --prefix misc/samples/js run strands -- files               # Image analysis
npm --prefix misc/samples/js run strands -- image-gen           # Image generation
npm --prefix misc/samples/js run strands -- rag-local           # RAG with embeddings
npm --prefix misc/samples/js run strands -- reasoning           # Extended thinking
npm --prefix misc/samples/js run strands -- swarm               # Multi-agent swarm
npm --prefix misc/samples/js run strands -- all                 # Run all
```

### JS Options

```bash
--model=bedrock-sonnet    # Select model (bedrock-haiku default, bedrock-sonnet)
--sideseat                # Enable SideSeat SDK telemetry
--help                    # Show help
```

## Replay

Replay captured OTLP debug files (`.jsonl`, `.jsonl.gz`, `.zip`) to a running SideSeat server.

```bash
uv run --directory misc/replay replay traces-strands.jsonl.gz
uv run --directory misc/replay replay traces-adk.jsonl.gz
uv run --directory misc/replay replay traces-vercel.jsonl.gz
uv run --directory misc/replay replay traces-langgraph.jsonl.gz
uv run --directory misc/replay replay traces-autogen.jsonl.gz
uv run --directory misc/replay replay traces-crewai.jsonl.gz
uv run --directory misc/replay replay traces-openai.jsonl.gz

# Absolute path or custom server URL
uv run --directory misc/replay replay /path/to/file.jsonl
uv run --directory misc/replay replay traces-autogen.jsonl.gz --base-url http://localhost:5388
```

Load generation:

```bash
uv run --directory misc/replay generate_load --spans 100000 --workers 5
```

## Environment Variables

Copy `misc/.env.example` to `misc/.env`:

```bash
# AWS (Bedrock, Strands, LangGraph, CrewAI)
AWS_REGION=us-east-1
# AWS_ACCESS_KEY_ID=...
# AWS_SECRET_ACCESS_KEY=...

# Anthropic (Anthropic provider, AutoGen)
ANTHROPIC_API_KEY=...

# OpenAI (OpenAI provider, OpenAI Agents, AutoGen)
OPENAI_API_KEY=...

# Google (ADK, Google Gemini)
GOOGLE_API_KEY=...

# SideSeat telemetry (optional, defaults shown)
SIDESEAT_ENDPOINT=http://127.0.0.1:5388
SIDESEAT_PROJECT_ID=default
```

## Directory Structure

```
misc/
├── .env.example          # Template for .env
├── content/              # Test files (images, PDFs)
├── fixtures/             # Test fixtures (OTEL trace data)
├── mcp/                  # MCP server implementations
├── replay/               # Trace replay utilities
├── screenshots/          # UI screenshots
├── scripts/              # Utility scripts
└── samples/
    ├── python/              # Each framework is an isolated uv package
    │   ├── common/          # Shared utilities (path dep for all packages)
    │   │   ├── pyproject.toml
    │   │   └── common/
    │   ├── strands/         # Strands Agents
    │   │   ├── pyproject.toml
    │   │   └── strands_sample/
    │   ├── adk/             # Google ADK
    │   ├── langgraph/       # LangGraph
    │   ├── crewai/          # CrewAI
    │   ├── autogen/         # AutoGen
    │   ├── openai-agents/   # OpenAI Agents SDK
    │   ├── agent-framework/ # Microsoft Agent Framework (Python 3.12 only)
    │   ├── bedrock/         # AWS Bedrock
    │   ├── anthropic/       # Anthropic API
    │   ├── openai/          # OpenAI API
    │   └── loadtest/        # Load test (no framework deps)
    └── js/src/
        ├── strands/
        └── vercel-ai/
```
