# OpenAI (Python SDK)

```bash
uv run --locked --directory examples/python/openai sample --list
uv run --locked --directory examples/python/openai sample tool_use              # native telemetry
uv run --locked --directory examples/python/openai sample tool_use --sideseat   # SideSeat SDK
```

The scenarios call the official `openai` client directly against GPT-6.1-sol on Bedrock's
OpenAI-compatible endpoint, running the tool loop themselves. Native mode is Logfire's documented
`logfire.instrument_openai()` setup exporting over OTLP; SideSeat mode replaces it with
`sideseat.init(integrations=["openai"])`, which runs the same instrumentation, so the two modes are a
span-for-span pair. OpenTelemetry's `opentelemetry-instrumentation-openai-v2` (2.4b0) is not used: it
does not trace the Responses API, and by default records message content only as log events.

`chat`, `multi_turn`, `session`, `structured_output` (`chat.completions.parse`), and `files` use Chat
Completions. `tool_use`, `error`, and `streaming` use the Responses API, because GPT-6.1-sol always
reasons and Bedrock serves function tools to a reasoning model only there.

There is no `reasoning` scenario: Bedrock rejects `reasoning.summary` for GPT-6.1-sol and returns its
reasoning only encrypted, so nothing visible reaches the telemetry. A provider client has no agents
and no MCP client, so there is no `multi_agent` or `mcp_tools` scenario either.
