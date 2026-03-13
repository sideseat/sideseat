# Telemetry Samples

Sample applications demonstrating OpenTelemetry integration with various AI/LLM frameworks.

## Run Commands

All frameworks share the same sample names and CLI options (`--model`, `--sideseat`, `--list`, `--help`).

### Strands

```bash
uv run --directory strands strands                           # List available samples and models
uv run --directory strands strands tool_use                  # Tool usage with calculator
uv run --directory strands strands mcp_tools                 # MCP server tools
uv run --directory strands strands structured_output         # Structured output extraction
uv run --directory strands strands files                     # Image and PDF file analysis
uv run --directory strands strands image_gen                 # Image generation
uv run --directory strands strands agent_core                # Core agent capabilities
uv run --directory strands strands swarm                     # Multi-agent swarm
uv run --directory strands strands rag_local                 # Local RAG with embeddings
uv run --directory strands strands reasoning                 # Extended thinking/reasoning
uv run --directory strands strands all                       # Run all samples

# Model selection
uv run --directory strands strands <sample> --model bedrock-haiku      # AWS Bedrock (default)
uv run --directory strands strands <sample> --model bedrock-sonnet     # AWS Bedrock Sonnet
uv run --directory strands strands <sample> --model bedrock-nova       # AWS Bedrock Nova 2 Lite
uv run --directory strands strands <sample> --model anthropic-haiku    # Anthropic direct API
uv run --directory strands strands <sample> --model anthropic-sonnet   # Anthropic Sonnet
uv run --directory strands strands <sample> --model openai-gpt5nano    # OpenAI
uv run --directory strands strands <sample> --model gemini-flash       # Google Gemini

# Telemetry options
uv run --directory strands strands <sample> --sideseat       # Use SideSeat telemetry
```

### LangGraph

```bash
uv run --directory langgraph langgraph                         # List samples and models
uv run --directory langgraph langgraph tool_use                # Tool usage
uv run --directory langgraph langgraph reasoning               # Extended thinking
uv run --directory langgraph langgraph all                     # Run all samples
```

### CrewAI

```bash
uv run --directory crewai crewai                            # List samples and models
uv run --directory crewai crewai tool_use                   # Tool usage
uv run --directory crewai crewai all                        # Run all samples
```

### Google ADK

```bash
uv run --directory adk telemetry-adk                               # List samples and models
uv run --directory adk telemetry-adk tool_use                      # Tool usage
uv run --directory adk telemetry-adk all                           # Run all samples
```

### AutoGen

```bash
uv run --directory autogen autogen                           # List samples and models
uv run --directory autogen autogen tool_use                  # Tool usage
uv run --directory autogen autogen all                       # Run all samples
```

Default model: `anthropic-haiku` (no native Bedrock support).

### OpenAI Agents SDK

```bash
uv run --directory openai-agents openai-agents                     # List samples and models
uv run --directory openai-agents openai-agents tool_use            # Tool usage
uv run --directory openai-agents openai-agents all                 # Run all samples
```

Default model: `openai-gpt5nano` (OpenAI models only).

### OpenAI Provider (raw SDK)

```bash
uv run --directory openai openai-provider                   # List samples and models
uv run --directory openai openai-provider chat_completions  # Sync, streaming, tool use
uv run --directory openai openai-provider responses         # Responses API (sync, streaming, tool use)
uv run --directory openai openai-provider multi_turn        # Multi-turn conversation (trace grouping)
uv run --directory openai openai-provider vision            # Image analysis (base64 vision)
uv run --directory openai openai-provider session           # Session with multiple traces
uv run --directory openai openai-provider error             # Error handling
uv run --directory openai openai-provider all               # Run all samples
```

Default model: `openai-gpt5nano` (OpenAI models only).

### Bedrock (raw boto3 API)

```bash
uv run --directory bedrock bedrock                           # List samples and models
uv run --directory bedrock bedrock converse                  # Sync, streaming, thinking, tool use
uv run --directory bedrock bedrock invoke_model              # InvokeModel API (Claude Messages API)
uv run --directory bedrock bedrock multi_turn                # Multi-turn conversation (trace grouping)
uv run --directory bedrock bedrock document                  # PDF + image multimodal analysis
uv run --directory bedrock bedrock session                   # Session with multiple traces
uv run --directory bedrock bedrock error                     # Error handling
uv run --directory bedrock bedrock all                       # Run all samples
```

Default model: `bedrock-haiku` (AWS Bedrock models only).

### Load Testing

```bash
uv run --directory loadtest loadtest                    # Default: 1M spans
uv run --directory loadtest loadtest --spans 100000     # 100K spans
uv run --directory loadtest loadtest --batch 5000       # Custom batch size
uv run --directory loadtest loadtest --workers 8        # Parallel workers
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

Not all models are available for all frameworks. Run `uv run --directory <framework> <script> --list` to see supported models.

## Environment Variables

| Variable                      | Description                     | Required For                          |
| ----------------------------- | ------------------------------- | ------------------------------------- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | SideSeat OTLP endpoint          | All                                   |
| `AWS_REGION`                  | AWS region (default: us-east-1) | Strands, LangGraph, CrewAI, ADK       |
| `ANTHROPIC_API_KEY`           | Anthropic API key               | anthropic-* models, AutoGen           |
| `OPENAI_API_KEY`              | OpenAI API key                  | openai-* models, AutoGen, OpenAI Agents, OpenAI Provider |
| `GOOGLE_API_KEY`              | Google API key                  | gemini-* models, ADK                  |
| `AGENT_CORE_MEMORY_ID`        | AWS AgentCore memory ID         | agent_core sample                     |
