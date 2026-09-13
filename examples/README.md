# Telemetry samples

Runnable sample applications per framework, and the inputs they read. Every suite exports OpenTelemetry traces,
which is what makes them the fixtures the message goldens are captured from.

## Setup

```bash
cp examples/.env.example examples/.env      # credentials, region, endpoint - not committed
npm --prefix examples/javascript ci         # the TypeScript suites share one project
uv sync --locked --directory examples/python/common
```

`common` holds the helpers every Python suite imports. **The suites themselves install on first use**: each is
its own uv project, and `uv run` creates its environment - so there is no list of thirteen `uv sync` lines to
keep in step with the tree, and a framework you never run costs nothing. (The list that used to be here had
gone stale.)

`--locked` throughout, here and in `run-all.sh`: a bare `uv run` rewrites a suite's lockfile to match a drifted
manifest, and `make update-python-deps` is the one command meant to do that. Every sample lockfile pointed at
`../../../../sdk/python` for a while - the path from before these directories were renamed - precisely because
nothing refused a stale lock.

OpenTelemetry versions differ between suites **on purpose**: google-adk 2.7 requires
`opentelemetry-sdk >=1.39,<=1.42.1`, and crewai and agent-framework hold their own ceilings. Each suite is an
isolated environment, so the split is harmless - forcing them onto one version makes the resolver refuse.

After a structural change to the SDK (new dependency, new extra, an edit to its `pyproject.toml`), reinstall it
into the suites you are using:

```bash
uv sync --locked --directory examples/python/<suite> --reinstall-package sideseat
```

## Python Samples

Start SideSeat first: `make dev-server`

View traces: http://localhost:5389/ui/projects/default/observability/traces

### All Frameworks

```bash
uv run --locked --directory examples/python/strands strands tool_use
uv run --locked --directory examples/python/adk telemetry-adk tool_use
uv run --locked --directory examples/python/langgraph langgraph tool_use
uv run --locked --directory examples/python/openai-agents openai-agents tool_use
uv run --locked --directory examples/python/agent-framework agent-framework tool_use
uv run --locked --directory examples/python/autogen autogen tool_use
uv run --locked --directory examples/python/crewai crewai tool_use
uv run --locked --directory examples/python/openai openai-provider chat_completions
uv run --locked --directory examples/python/openai openai-provider responses
uv run --locked --directory examples/python/anthropic anthropic-provider messages
uv run --locked --directory examples/python/bedrock bedrock converse
npm --prefix examples/javascript run vercel-ai -- tool-use
npm --prefix examples/javascript run strands -- tool-use
```

### Strands Agents

```bash
uv run --locked --directory examples/python/strands strands                        # List samples and models
uv run --locked --directory examples/python/strands strands tool_use               # Tool usage
uv run --locked --directory examples/python/strands strands mcp_tools              # MCP server integration
uv run --locked --directory examples/python/strands strands structured_output      # Structured data extraction
uv run --locked --directory examples/python/strands strands reasoning              # Extended thinking
uv run --locked --directory examples/python/strands strands files                  # Image/PDF analysis
uv run --locked --directory examples/python/strands strands image_gen              # Image generation
uv run --locked --directory examples/python/strands strands rag_local              # RAG with embeddings
uv run --locked --directory examples/python/strands strands swarm                  # Multi-agent swarm
uv run --locked --directory examples/python/strands strands agent_core             # AgentCore integration
uv run --locked --directory examples/python/strands strands error                  # Error handling
uv run --locked --directory examples/python/strands strands all                    # Run all
```

### OpenAI Provider

```bash
uv run --locked --directory examples/python/openai openai-provider                # List samples and models
uv run --locked --directory examples/python/openai openai-provider chat_completions  # Sync, streaming, tool use
uv run --locked --directory examples/python/openai openai-provider responses      # Responses API
uv run --locked --directory examples/python/openai openai-provider multi_turn     # Multi-turn (trace grouping)
uv run --locked --directory examples/python/openai openai-provider vision         # Image analysis
uv run --locked --directory examples/python/openai openai-provider session        # Session with multiple traces
uv run --locked --directory examples/python/openai openai-provider error          # Error handling
uv run --locked --directory examples/python/openai openai-provider all            # Run all
```

### Anthropic Provider

```bash
uv run --locked --directory examples/python/anthropic anthropic-provider             # List samples and models
uv run --locked --directory examples/python/anthropic anthropic-provider messages    # Sync, streaming, tool use
uv run --locked --directory examples/python/anthropic anthropic-provider multi_turn  # Multi-turn (trace grouping)
uv run --locked --directory examples/python/anthropic anthropic-provider thinking    # Extended thinking
uv run --locked --directory examples/python/anthropic anthropic-provider vision      # Image analysis
uv run --locked --directory examples/python/anthropic anthropic-provider document    # PDF analysis
uv run --locked --directory examples/python/anthropic anthropic-provider session     # Session with multiple traces
uv run --locked --directory examples/python/anthropic anthropic-provider error       # Error handling
uv run --locked --directory examples/python/anthropic anthropic-provider all         # Run all
```

### Bedrock Provider

```bash
uv run --locked --directory examples/python/bedrock bedrock                        # List samples and models
uv run --locked --directory examples/python/bedrock bedrock converse               # Sync, streaming, thinking, tool use
uv run --locked --directory examples/python/bedrock bedrock multi_turn             # Multi-turn (trace grouping)
uv run --locked --directory examples/python/bedrock bedrock invoke_model           # InvokeModel API
uv run --locked --directory examples/python/bedrock bedrock document               # PDF + image multimodal
uv run --locked --directory examples/python/bedrock bedrock session                # Session with multiple traces
uv run --locked --directory examples/python/bedrock bedrock error                  # Error handling
uv run --locked --directory examples/python/bedrock bedrock all                    # Run all
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
uv run --locked --directory examples/python/loadtest loadtest              # Default: 1M spans
uv run --locked --directory examples/python/loadtest loadtest --spans 100000  # Custom span count
```

## JavaScript Samples

### Vercel AI SDK

```bash
npm --prefix examples/javascript run vercel-ai -- --list            # List samples and models
npm --prefix examples/javascript run vercel-ai -- tool-use          # Tool usage
npm --prefix examples/javascript run vercel-ai -- structured-output # Structured data extraction
npm --prefix examples/javascript run vercel-ai -- files             # Image analysis
npm --prefix examples/javascript run vercel-ai -- image-gen         # Image generation
npm --prefix examples/javascript run vercel-ai -- rag-local         # RAG with embeddings
npm --prefix examples/javascript run vercel-ai -- reasoning         # Chain-of-thought
npm --prefix examples/javascript run vercel-ai -- multi-step        # Agentic loop
npm --prefix examples/javascript run vercel-ai -- all               # Run all
```

### Strands Agents (JS)

```bash
npm --prefix examples/javascript run strands -- --list              # List samples and models
npm --prefix examples/javascript run strands -- tool-use            # Tool usage
npm --prefix examples/javascript run strands -- mcp-tools           # MCP server integration
npm --prefix examples/javascript run strands -- structured-output   # Structured data extraction
npm --prefix examples/javascript run strands -- files               # Image analysis
npm --prefix examples/javascript run strands -- image-gen           # Image generation
npm --prefix examples/javascript run strands -- rag-local           # RAG with embeddings
npm --prefix examples/javascript run strands -- reasoning           # Extended thinking
npm --prefix examples/javascript run strands -- swarm               # Multi-agent swarm
npm --prefix examples/javascript run strands -- all                 # Run all
```

### JS Options

```bash
--model=bedrock-sonnet    # Select model (bedrock-haiku default, bedrock-sonnet)
--sideseat                # Enable SideSeat SDK telemetry
--help                    # Show help
```

## Environment Variables

Copy `examples/.env.example` to `examples/.env`:

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
examples/
├── .env.example          # Template for .env (credentials, region, endpoint)
├── run-all.sh            # Run every sample credentials allow, then verify each trace
├── verify-trace.py       # Assert a run produced correct data, not just exit 0
├── assets/               # Inputs for file-handling samples (image, PDF)
├── data/                 # Tabular input for samples that read a dataset
├── python/               # One directory per framework suite
└── javascript/           # TypeScript suites (Strands, Vercel AI, Claude Agent SDK)
```

Related directories elsewhere in the repository:

```
tools/mcp-calculator/     # MCP server the mcp_tools samples connect to
tools/otel-replay/        # Replays captured OTLP into a running server - see its own README
```
