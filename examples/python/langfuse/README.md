# Langfuse (Python SDK)

```bash
uv run --locked --directory examples/python/langfuse sample --list
uv run --locked --directory examples/python/langfuse sample tool_use              # native telemetry
uv run --locked --directory examples/python/langfuse sample tool_use --sideseat   # SideSeat SDK
```

The scenarios are the `openai` suite's, written the way a Langfuse user writes them: the OpenAI client
comes from Langfuse's drop-in `langfuse.openai` integration, and the agent loop and each tool run inside
`@observe` observations. Langfuse 4 records every observation as an OpenTelemetry span; native mode
gives it the application's tracer provider, which exports over OTLP, and an exporter that sends nothing
to Langfuse's cloud. SideSeat mode does the same through `sideseat.init(integrations=["langfuse"])`.
Media upload is off in both, so images and documents stay inline instead of being replaced by references
to Langfuse's storage.

The model requests are byte-for-byte the `openai` suite's, so the suite replays its cassettes:
GPT-6.1-sol on Bedrock's OpenAI-compatible endpoint, Chat Completions for conversations without tools
and the Responses API wherever tools are declared. The scenario set and its gaps are that suite's too.
