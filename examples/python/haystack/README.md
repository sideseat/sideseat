# Haystack

```bash
uv run --locked --directory examples/python/haystack sample --list
uv run --locked --directory examples/python/haystack sample tool_use              # native telemetry
uv run --locked --directory examples/python/haystack sample tool_use --sideseat   # SideSeat SDK
```

The scenarios run Haystack's `Agent` on the Amazon Bedrock chat generator. Native mode connects
Haystack's tracing API to a plain OpenTelemetry provider with `opentelemetry-haystack`, with content
tracing on; SideSeat mode replaces that with `sideseat.init(integrations=["haystack"])`.
