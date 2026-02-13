# Misc

Shared resources, sample applications, and utilities for SideSeat development.

## Quick Start

All commands run from the **repo root**.

```bash
# Setup
cp misc/.env.example misc/.env                     # Configure environment
uv sync --directory misc/samples/python             # Install Python deps
npm --prefix misc/samples/js install                # Install JS deps
uv sync --directory misc/replay                     # Install replay deps

# Run a sample
uv run --directory misc/samples/python strands tool_use --sideseat
npm --prefix misc/samples/js run vercel-ai -- tool-use --sideseat
```

## Run All Framework Samples

Start SideSeat first: `make dev-server`

```bash
# Python frameworks
uv run --directory misc/samples/python strands tool_use --sideseat
uv run --directory misc/samples/python adk tool_use --sideseat
uv run --directory misc/samples/python langgraph tool_use --sideseat
uv run --directory misc/samples/python crewai tool_use --sideseat
uv run --directory misc/samples/python autogen tool_use --sideseat
uv run --directory misc/samples/python openai-agents tool_use --sideseat

# JavaScript frameworks
npm --prefix misc/samples/js run vercel-ai -- tool-use --sideseat
npm --prefix misc/samples/js run strands -- tool-use --sideseat

# Or run all Python samples at once
uv run --directory misc/samples/python telemetry-all
```

View traces at: http://localhost:5389/ui/projects/default/observability/traces

## Environment Setup

Copy `misc/.env.example` to `misc/.env` and configure:

```bash
cp misc/.env.example misc/.env
```

Required variables:

```bash
AWS_REGION=us-east-1
# AWS_ACCESS_KEY_ID=...      # Or use AWS profiles
# AWS_SECRET_ACCESS_KEY=...

# Optional: SideSeat telemetry
SIDESEAT_ENDPOINT=http://127.0.0.1:5388
SIDESEAT_PROJECT_ID=default
```

## Directory Structure

```
misc/
├── .env                  # Environment variables (gitignored)
├── .env.example          # Template for .env
├── screenshots/          # UI screenshots
├── content/              # Test files (images, PDFs)
├── mcp/                  # MCP server implementations
├── fixtures/             # Test fixtures (OTEL trace data)
├── scripts/              # Utility scripts
├── replay/               # Trace replay utilities
└── samples/
    ├── python/           # Python samples
    │   └── src/
    │       ├── strands_sample/
    │       ├── langgraph_sample/
    │       ├── crewai_sample/
    │       ├── adk_sample/
    │       ├── autogen_sample/
    │       ├── openai_agents_sample/
    │       └── common/
    └── js/               # JavaScript/TypeScript samples
        └── src/
            ├── strands/
            └── vercel-ai/
```

## Python Samples

```bash
# Strands Agents
uv run --directory misc/samples/python strands                        # List samples and models
uv run --directory misc/samples/python strands tool_use               # Tool usage
uv run --directory misc/samples/python strands mcp_tools              # MCP server integration
uv run --directory misc/samples/python strands structured_output      # Structured data extraction
uv run --directory misc/samples/python strands reasoning              # Extended thinking
uv run --directory misc/samples/python strands files                  # Image/PDF analysis
uv run --directory misc/samples/python strands image_gen              # Image generation
uv run --directory misc/samples/python strands rag_local              # RAG with embeddings
uv run --directory misc/samples/python strands swarm                  # Multi-agent swarm
uv run --directory misc/samples/python strands agent_core             # AgentCore integration
uv run --directory misc/samples/python strands all                    # Run all samples

# Model selection
uv run --directory misc/samples/python strands <sample> --model bedrock-haiku    # AWS Bedrock (default)
uv run --directory misc/samples/python strands <sample> --model bedrock-sonnet   # Claude Sonnet
uv run --directory misc/samples/python strands <sample> --model anthropic-haiku  # Anthropic direct
uv run --directory misc/samples/python strands <sample> --model openai-gpt5nano  # OpenAI
uv run --directory misc/samples/python strands <sample> --model gemini-flash     # Google Gemini

# Telemetry
uv run --directory misc/samples/python strands <sample> --sideseat    # Enable SideSeat SDK

# Other frameworks (same sample names and options)
uv run --directory misc/samples/python langgraph tool_use             # LangGraph ReAct agent
uv run --directory misc/samples/python crewai tool_use                # CrewAI multi-agent
uv run --directory misc/samples/python adk tool_use                   # Google ADK
uv run --directory misc/samples/python autogen tool_use               # AutoGen chat
uv run --directory misc/samples/python openai-agents tool_use         # OpenAI Agents SDK

# Load testing
uv run --directory misc/samples/python loadtest                       # Default: 1M spans
uv run --directory misc/samples/python loadtest --spans 100000        # Custom span count
```

## JavaScript Samples

```bash
# Vercel AI SDK
npm --prefix misc/samples/js run vercel-ai -- --list            # List samples and models
npm --prefix misc/samples/js run vercel-ai -- tool-use          # Tool usage
npm --prefix misc/samples/js run vercel-ai -- structured-output # Structured data extraction
npm --prefix misc/samples/js run vercel-ai -- files             # Image analysis
npm --prefix misc/samples/js run vercel-ai -- image-gen         # Image generation
npm --prefix misc/samples/js run vercel-ai -- rag-local         # RAG with embeddings
npm --prefix misc/samples/js run vercel-ai -- reasoning         # Chain-of-thought
npm --prefix misc/samples/js run vercel-ai -- multi-step        # Agentic loop
npm --prefix misc/samples/js run vercel-ai -- all               # Run all samples

# Strands Agents
npm --prefix misc/samples/js run strands -- --list              # List samples and models
npm --prefix misc/samples/js run strands -- tool-use            # Tool usage
npm --prefix misc/samples/js run strands -- mcp-tools           # MCP server integration
npm --prefix misc/samples/js run strands -- structured-output   # Structured data extraction
npm --prefix misc/samples/js run strands -- files               # Image analysis
npm --prefix misc/samples/js run strands -- image-gen           # Image generation
npm --prefix misc/samples/js run strands -- rag-local           # RAG with embeddings
npm --prefix misc/samples/js run strands -- reasoning           # Extended thinking
npm --prefix misc/samples/js run strands -- swarm               # Multi-agent swarm
npm --prefix misc/samples/js run strands -- all                 # Run all samples

# Options (both frameworks)
npm --prefix misc/samples/js run strands -- <sample> --model=bedrock-sonnet
npm --prefix misc/samples/js run strands -- <sample> --sideseat
npm --prefix misc/samples/js run strands -- <sample> --help
```

## Model Aliases

### Python

| Alias              | Provider                           |
| ------------------ | ---------------------------------- |
| `bedrock-haiku`    | AWS Bedrock Claude Haiku (default) |
| `bedrock-sonnet`   | AWS Bedrock Claude Sonnet          |
| `bedrock-nova`     | AWS Bedrock Nova 2 Lite            |
| `anthropic-haiku`  | Anthropic API Claude Haiku         |
| `anthropic-sonnet` | Anthropic API Claude Sonnet        |
| `openai-gpt5nano`  | OpenAI GPT-5 Nano                  |
| `gemini-flash`     | Google Gemini Flash                |

Default model varies by framework: Strands/LangGraph/CrewAI/ADK use `bedrock-haiku`, AutoGen uses `anthropic-haiku`, OpenAI Agents uses `openai-gpt5nano`.

### JavaScript

| Alias            | Model ID                                                    |
| ---------------- | ----------------------------------------------------------- |
| `bedrock-haiku`  | `global.anthropic.claude-haiku-4-5-20251001-v1:0` (default) |
| `bedrock-sonnet` | `global.anthropic.claude-sonnet-4-20250514-v1:0`            |

## Telemetry

All samples support SideSeat observability via `--sideseat`:

```bash
# Start SideSeat server (from repo root)
make dev-server

# Python
uv run --directory misc/samples/python strands tool_use --sideseat

# JavaScript
npm --prefix misc/samples/js run strands -- tool-use --sideseat
```

View traces at: http://localhost:5389/ui/projects/default/observability/traces

## Replay

Replay captured OTLP debug files (`.jsonl`, `.jsonl.gz`, `.zip`) to a running SideSeat server.

```bash
# Setup
uv sync --directory misc/replay

# Replay a fixture (relative to misc/fixtures/)
uv run --directory misc/replay replay traces-strands.jsonl.gz
uv run --directory misc/replay replay traces-adk.jsonl.gz
uv run --directory misc/replay replay traces-vercel.jsonl.gz
uv run --directory misc/replay replay traces-langgraph.jsonl.gz
uv run --directory misc/replay replay traces-openai.jsonl.gz
uv run --directory misc/replay replay traces-autogen.jsonl.gz
uv run --directory misc/replay replay traces-crewai.jsonl.gz

# Absolute path
uv run --directory misc/replay replay /path/to/file.jsonl

# Custom server URL
uv run --directory misc/replay replay traces-autogen.jsonl.gz --base-url http://localhost:5388
```

Signal type (traces/metrics/logs) is auto-detected from the filename. Compressed files are decompressed to a temp directory and cleaned up after replay.
