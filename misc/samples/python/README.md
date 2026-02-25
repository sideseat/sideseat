# Telemetry Samples

Sample applications demonstrating OpenTelemetry integration with various AI/LLM frameworks.

## Run Commands

All frameworks share the same sample names and CLI options (`--model`, `--sideseat`, `--list`, `--help`).

### Strands

```bash
uv run strands                           # List available samples and models
uv run strands tool_use                  # Tool usage with calculator
uv run strands mcp_tools                 # MCP server tools
uv run strands structured_output         # Structured output extraction
uv run strands files                     # Image and PDF file analysis
uv run strands image_gen                 # Image generation
uv run strands agent_core                # Core agent capabilities
uv run strands swarm                     # Multi-agent swarm
uv run strands rag_local                 # Local RAG with embeddings
uv run strands reasoning                 # Extended thinking/reasoning
uv run strands all                       # Run all samples

# Model selection
uv run strands <sample> --model bedrock-haiku      # AWS Bedrock (default)
uv run strands <sample> --model bedrock-sonnet     # AWS Bedrock Sonnet
uv run strands <sample> --model bedrock-nova       # AWS Bedrock Nova 2 Lite
uv run strands <sample> --model anthropic-haiku    # Anthropic direct API
uv run strands <sample> --model anthropic-sonnet   # Anthropic Sonnet
uv run strands <sample> --model openai-gpt5nano    # OpenAI
uv run strands <sample> --model gemini-flash       # Google Gemini

# Telemetry options
uv run strands <sample> --sideseat       # Use SideSeat telemetry
```

### LangGraph

```bash
uv run langgraph                         # List samples and models
uv run langgraph tool_use                # Tool usage
uv run langgraph reasoning               # Extended thinking
uv run langgraph all                     # Run all samples
```

### CrewAI

```bash
uv run crewai                            # List samples and models
uv run crewai tool_use                   # Tool usage
uv run crewai all                        # Run all samples
```

### Google ADK

```bash
uv run adk                               # List samples and models
uv run adk tool_use                      # Tool usage
uv run adk all                           # Run all samples
```

### AutoGen

```bash
uv run autogen                           # List samples and models
uv run autogen tool_use                  # Tool usage
uv run autogen all                       # Run all samples
```

Default model: `anthropic-haiku` (no native Bedrock support).

### OpenAI Agents SDK

```bash
uv run openai-agents                     # List samples and models
uv run openai-agents tool_use            # Tool usage
uv run openai-agents all                 # Run all samples
```

Default model: `openai-gpt5nano` (OpenAI models only).

### OpenAI Provider (raw SDK)

```bash
uv run openai-provider                   # List samples and models
uv run openai-provider chat_completions  # Sync, streaming, tool use
uv run openai-provider responses         # Responses API (sync, streaming, tool use)
uv run openai-provider multi_turn        # Multi-turn conversation (trace grouping)
uv run openai-provider vision            # Image analysis (base64 vision)
uv run openai-provider session           # Session with multiple traces
uv run openai-provider error             # Error handling
uv run openai-provider all               # Run all samples
```

Default model: `openai-gpt5nano` (OpenAI models only).

### Bedrock (raw boto3 API)

```bash
uv run bedrock                           # List samples and models
uv run bedrock converse                  # Sync, streaming, thinking, tool use
uv run bedrock invoke_model              # InvokeModel API (Claude Messages API)
uv run bedrock multi_turn                # Multi-turn conversation (trace grouping)
uv run bedrock document                  # PDF + image multimodal analysis
uv run bedrock session                   # Session with multiple traces
uv run bedrock error                     # Error handling
uv run bedrock all                       # Run all samples
```

Default model: `bedrock-haiku` (AWS Bedrock models only).

### Load Testing

```bash
uv run loadtest                    # Default: 1M spans
uv run loadtest --spans 100000     # 100K spans
uv run loadtest --batch 5000       # Custom batch size
uv run loadtest --workers 8        # Parallel workers
```

## Model Aliases

| Alias              | Provider                 | Default For                      |
| ------------------ | ------------------------ | -------------------------------- |
| `bedrock-haiku`    | AWS Bedrock Claude Haiku | Strands, LangGraph, CrewAI, ADK  |
| `bedrock-sonnet`   | AWS Bedrock Claude Sonnet|                                  |
| `bedrock-nova`     | AWS Bedrock Nova 2 Lite  |                                  |
| `anthropic-haiku`  | Anthropic API Haiku      | AutoGen                          |
| `anthropic-sonnet` | Anthropic API Sonnet     |                                  |
| `openai-gpt5nano`  | OpenAI GPT-5 Nano        | OpenAI Agents, OpenAI Provider   |
| `gemini-flash`     | Google Gemini Flash      |                                  |

Not all models are available for all frameworks. Run `uv run <framework> --list` to see supported models.

## Environment Variables

| Variable                      | Description                     | Required For                          |
| ----------------------------- | ------------------------------- | ------------------------------------- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | SideSeat OTLP endpoint          | All                                   |
| `AWS_REGION`                  | AWS region (default: us-east-1) | Strands, LangGraph, CrewAI, ADK       |
| `ANTHROPIC_API_KEY`           | Anthropic API key               | anthropic-* models, AutoGen           |
| `OPENAI_API_KEY`              | OpenAI API key                  | openai-* models, AutoGen, OpenAI Agents, OpenAI Provider |
| `GOOGLE_API_KEY`              | Google API key                  | gemini-* models, ADK                  |
| `AGENT_CORE_MEMORY_ID`        | AWS AgentCore memory ID         | agent_core sample                     |
