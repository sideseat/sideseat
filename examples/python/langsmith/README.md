# LangSmith (OpenTelemetry mode)

```bash
uv run --locked --directory examples/python/langsmith sample --list
uv run --locked --directory examples/python/langsmith sample tool_use              # native telemetry
uv run --locked --directory examples/python/langsmith sample tool_use --sideseat   # SideSeat SDK
```

The `langgraph` suite's scenarios, traced by LangSmith instead of OpenInference. With
`LANGSMITH_TRACING_MODE=otel` LangSmith turns every LangChain and LangGraph run into an OpenTelemetry span
on the global tracer provider and sends nothing to its own API. Native mode sets that up on an
OTLP-exporting provider; SideSeat mode does it through `sideseat.init(integrations=["langsmith"])`.
LangSmith hands runs over from a background thread, so both modes drain its client before the
provider flushes.

The model requests are the `langgraph` suite's, so the suite replays its cassettes.
