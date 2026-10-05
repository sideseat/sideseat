# Telemetry Samples

Sample applications demonstrating OpenTelemetry integration with various AI/LLM frameworks.

Each suite is its own [uv](https://docs.astral.sh/uv/) project, so `uv run` resolves and installs its
dependencies on first use. Install uv with `curl -LsSf https://astral.sh/uv/install.sh | sh`
(`powershell -c "irm https://astral.sh/uv/install.ps1 | iex"` on Windows).

## Run Commands

All suites share the same CLI options (`--model`, `--sideseat`, `--list`, `--help`), but **not
the same sample names** — the provider suites have their own (`bedrock` has `converse` and
`invoke_model`, not `tool_use`). Run `--list` to see what a suite actually offers; that is also
what `make capture` does rather than assuming.

Without `--sideseat`, each suite uses its framework's native instrumentation and a raw
OpenTelemetry exporter. With `--sideseat`, the SideSeat SDK owns framework instrumentation
and export. Run both forms when validating parser parity.

Current lockfile baselines:

| Suite | Framework/provider SDK |
| --- | --- |
| Strands | `strands-agents 1.57.0` |
| LangGraph | `langgraph 1.2.12` |
| CrewAI | `crewai 1.15.23` |
| Google ADK | `google-adk 2.9.2` |
| AutoGen | `autogen-agentchat 0.7.5` |
| OpenAI Agents | `openai-agents 0.22.3` |
| Microsoft Agent Framework | `agent-framework-core 1.19.0` |
| Claude Agent SDK | `claude-agent-sdk 0.2.159` |
| Anthropic provider | `anthropic 1.8.0` |
| OpenAI provider | `openai 3.24.0` |

### Strands

```bash
uv run --locked --directory strands strands                           # List available samples and models
uv run --locked --directory strands strands tool_use                  # Tool usage with calculator
uv run --locked --directory strands strands mcp_tools                 # MCP server tools
uv run --locked --directory strands strands structured_output         # Structured output extraction
uv run --locked --directory strands strands files                     # Image and PDF file analysis
uv run --locked --directory strands strands image_gen                 # Image generation
uv run --locked --directory strands strands agent_core                # Core agent capabilities
uv run --locked --directory strands strands swarm                     # Multi-agent swarm
uv run --locked --directory strands strands rag_local                 # Local RAG with embeddings
uv run --locked --directory strands strands reasoning                 # Extended thinking/reasoning
uv run --locked --directory strands strands strands_ws                # WS runtime channel: presence + AG-UI invoke
uv run --locked --directory strands strands all                       # Run all samples

# Model selection
uv run --locked --directory strands strands <sample> --model bedrock-haiku      # AWS Bedrock (default)
uv run --locked --directory strands strands <sample> --model bedrock-sonnet     # AWS Bedrock Sonnet
uv run --locked --directory strands strands <sample> --model bedrock-nova       # AWS Bedrock Nova 2 Lite
uv run --locked --directory strands strands <sample> --model anthropic-haiku    # Anthropic direct API
uv run --locked --directory strands strands <sample> --model anthropic-sonnet   # Anthropic Sonnet
uv run --locked --directory strands strands <sample> --model openai-gpt5nano    # OpenAI
uv run --locked --directory strands strands <sample> --model gemini-flash       # Google Gemini

# Telemetry options
uv run --locked --directory strands strands <sample> --sideseat       # Use SideSeat telemetry
```

### LangGraph

```bash
uv run --locked --directory langgraph sample --list                  # List scenarios and models
uv run --locked --directory langgraph sample tool_use                # Tool usage, native telemetry
uv run --locked --directory langgraph sample tool_use --sideseat     # The same with the SideSeat SDK
uv run --locked --directory langgraph sample all                     # Run every scenario
```

On the shared scenario harness; see `langgraph/README.md`.

### CrewAI

```bash
uv run --locked --directory crewai sample --list                     # List scenarios and models
uv run --locked --directory crewai sample tool_use                   # Tool usage, native telemetry
uv run --locked --directory crewai sample tool_use --sideseat        # The same with the SideSeat SDK
uv run --locked --directory crewai sample all                        # Run every scenario
```

On the shared scenario harness; see `crewai/README.md`.

### Google ADK

```bash
uv run --locked --directory adk telemetry-adk                               # List samples and models
uv run --locked --directory adk telemetry-adk tool_use                      # Tool usage
uv run --locked --directory adk telemetry-adk all                           # Run all samples
```

### AutoGen

```bash
uv run --locked --directory autogen sample --list                    # List scenarios and models
uv run --locked --directory autogen sample tool_use                  # Tool usage, native telemetry
uv run --locked --directory autogen sample tool_use --sideseat       # The same with the SideSeat SDK
uv run --locked --directory autogen sample all                       # Run every scenario
```

On the shared scenario harness; see `autogen/README.md`. Claude runs on Bedrock through the
Anthropic SDK's Bedrock client.

### OpenAI Agents SDK

```bash
uv run --locked --directory openai-agents telemetry-openai-agents                     # List samples and models
uv run --locked --directory openai-agents telemetry-openai-agents tool_use            # Tool usage
uv run --locked --directory openai-agents telemetry-openai-agents all                 # Run all samples
```

Default model: `bedrock-openai-luna`. It uses AWS credentials through Bedrock's
OpenAI-compatible endpoint; direct `openai-*` aliases require `OPENAI_API_KEY`.

### Claude Agent SDK

```bash
uv run --locked --directory claude-agent-sdk claude-agent-sdk                        # List samples and models
uv run --locked --directory claude-agent-sdk claude-agent-sdk tool_use               # Built-in Read/Glob/Grep/Bash
uv run --locked --directory claude-agent-sdk claude-agent-sdk mcp_tools              # External stdio MCP server
uv run --locked --directory claude-agent-sdk claude-agent-sdk structured_output      # JSON schema output
uv run --locked --directory claude-agent-sdk claude-agent-sdk reasoning              # Extended thinking
uv run --locked --directory claude-agent-sdk claude-agent-sdk custom_tools           # In-process MCP via @tool
uv run --locked --directory claude-agent-sdk claude-agent-sdk subagents              # AgentDefinition delegation
uv run --locked --directory claude-agent-sdk claude-agent-sdk multi_turn             # ClaudeSDKClient session
uv run --locked --directory claude-agent-sdk claude-agent-sdk permissions            # can_use_tool gating
uv run --locked --directory claude-agent-sdk claude-agent-sdk all                    # Run all samples
```

Default model: `bedrock-haiku` (Bedrock models only).

Unlike every other sample here, there is no instrumentor: the SDK spawns the Claude
Code CLI, which carries its own OpenTelemetry instrumentation and is configured with
the `CLAUDE_CODE_*` / `OTEL_*` environment variables built in `telemetry_setup.py`.
Span tracing is beta, so `CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1` is required, and the
`console` exporter must never be used — the CLI writes telemetry to stdout, which is
the SDK's message channel.

The message feed needs a **second** beta tier on top of that:
`ENABLE_BETA_TRACING_DETAILED=1` plus `BETA_TRACING_ENDPOINT` (a base URL, not a
`/v1/traces` path). Only then does the CLI emit `response.model_output`,
`new_context`, `user_system_prompt` and `tool_input` — without them you get spans,
timings, tokens and costs but no conversation.

On Bedrock, `WebSearch` is unavailable. The Agent SDK docs also say the `thinking`
config is not forwarded to Bedrock, but the `reasoning` sample did return thinking
blocks against `bedrock-haiku` — treat that support as version-dependent.

### Microsoft Agent Framework

```bash
uv run --locked --directory agent-framework telemetry-agent-framework                    # List samples and models
uv run --locked --directory agent-framework telemetry-agent-framework tool_use           # Tool calling
uv run --locked --directory agent-framework telemetry-agent-framework mcp_tools          # MCP tool integration
uv run --locked --directory agent-framework telemetry-agent-framework structured_output  # Structured output
uv run --locked --directory agent-framework telemetry-agent-framework files              # Multimodal file input
uv run --locked --directory agent-framework telemetry-agent-framework image_gen          # Image generation
uv run --locked --directory agent-framework telemetry-agent-framework agent_core         # Memory and code execution
uv run --locked --directory agent-framework telemetry-agent-framework swarm              # Multi-agent workflow
uv run --locked --directory agent-framework telemetry-agent-framework rag_local          # Local RAG
uv run --locked --directory agent-framework telemetry-agent-framework reasoning          # Extended thinking
uv run --locked --directory agent-framework telemetry-agent-framework error              # Error handling
uv run --locked --directory agent-framework telemetry-agent-framework all                # Run all samples
```

Default model: `bedrock-openai-luna`. The suite supports OpenAI and Anthropic APIs directly
and routes `bedrock-*` aliases through Bedrock-compatible clients, so the default runs with
AWS credentials alone.

### Anthropic Provider (raw SDK)

```bash
uv run --locked --directory anthropic anthropic-provider              # List samples and models
uv run --locked --directory anthropic anthropic-provider messages     # Messages API (sync, streaming, tool use)
uv run --locked --directory anthropic anthropic-provider multi_turn   # Multi-turn conversation (trace grouping)
uv run --locked --directory anthropic anthropic-provider thinking     # Extended thinking
uv run --locked --directory anthropic anthropic-provider vision       # Image analysis
uv run --locked --directory anthropic anthropic-provider document     # PDF analysis
uv run --locked --directory anthropic anthropic-provider session      # Session with multiple traces
uv run --locked --directory anthropic anthropic-provider error        # Error handling
uv run --locked --directory anthropic anthropic-provider all          # Run all samples
uv run --locked --directory anthropic anthropic-provider messages --sideseat  # SideSeat SDK mode
```

Default model: `bedrock-anthropic-sonnet5`, using AWS credentials through Bedrock's
Anthropic-compatible endpoint. Direct `anthropic-*` aliases require `ANTHROPIC_API_KEY`.

### OpenAI Provider (raw SDK)

```bash
uv run --locked --directory openai sample --list              # List scenarios and models
uv run --locked --directory openai sample tool_use            # Responses API tool loop, native Logfire telemetry
uv run --locked --directory openai sample tool_use --sideseat # The same scenario through sideseat.init
```

Default model: `gpt` (GPT-6.1-sol on Bedrock's OpenAI-compatible endpoint), using AWS credentials.
See `openai/README.md` for which API each scenario uses.

### Bedrock (raw boto3 API)

```bash
uv run --locked --directory bedrock bedrock                           # List samples and models
uv run --locked --directory bedrock bedrock converse                  # Sync, streaming, thinking, tool use
uv run --locked --directory bedrock bedrock invoke_model              # InvokeModel API (Claude Messages API)
uv run --locked --directory bedrock bedrock multi_turn                # Multi-turn conversation (trace grouping)
uv run --locked --directory bedrock bedrock document                  # PDF + image multimodal analysis
uv run --locked --directory bedrock bedrock session                   # Session with multiple traces
uv run --locked --directory bedrock bedrock error                     # Error handling
uv run --locked --directory bedrock bedrock all                       # Run all samples
uv run --locked --directory bedrock bedrock converse --sideseat       # SideSeat SDK mode
```

Default model: `bedrock-haiku` (AWS Bedrock models only).

### Load Testing

```bash
uv run --locked --directory loadtest loadtest                    # Default: 1M spans
uv run --locked --directory loadtest loadtest --spans 100000     # 100K spans
uv run --locked --directory loadtest loadtest --batch 5000       # Custom batch size
uv run --locked --directory loadtest loadtest --workers 8        # Parallel workers
```

## Model Aliases

| Alias              | Provider                 | Default For                      |
| ------------------ | ------------------------ | -------------------------------- |
| `bedrock-haiku`    | AWS Bedrock Claude Haiku | Strands, LangGraph, CrewAI, ADK, Claude Agent SDK |
| `bedrock-sonnet`   | AWS Bedrock Claude Sonnet|                                  |
| `bedrock-nova`     | AWS Bedrock Nova 2 Lite  |                                  |
| `anthropic-haiku`  | Anthropic API Haiku      | AutoGen                          |
| `anthropic-sonnet` | Anthropic API Sonnet     |                                  |
| `openai-gpt5nano`  | OpenAI GPT-5 Nano        | OpenAI Agents, OpenAI Provider   |
| `gemini-flash`     | Google Gemini Flash      |                                  |

Not all models are available for all frameworks. Run `uv run --locked --directory <framework> <script> --list` to see supported models.

## Environment Variables

| Variable                      | Description                     | Required For                          |
| ----------------------------- | ------------------------------- | ------------------------------------- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | SideSeat OTLP endpoint          | All                                   |
| `AWS_REGION`                  | AWS region (default: us-east-1) | Strands, LangGraph, CrewAI, ADK, Claude Agent SDK |
| `ANTHROPIC_API_KEY`           | Anthropic API key               | anthropic-* models, AutoGen           |
| `OPENAI_API_KEY`              | OpenAI API key                  | openai-* models, AutoGen, OpenAI Agents, OpenAI Provider |
| `GOOGLE_API_KEY`              | Google API key                  | gemini-* models, ADK                  |
| `AGENT_CORE_MEMORY_ID`        | AWS AgentCore memory ID         | agent_core sample                     |
