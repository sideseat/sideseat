# Pydantic AI

```bash
uv run --locked --directory examples/python/pydantic-ai sample --list
uv run --locked --directory examples/python/pydantic-ai sample tool_use              # native telemetry
uv run --locked --directory examples/python/pydantic-ai sample tool_use --sideseat   # SideSeat SDK
```

Native mode installs a plain OpenTelemetry provider and calls `Agent.instrument_all()`, as the Pydantic
AI documentation describes for OpenTelemetry without Logfire. SideSeat mode replaces that with
`sideseat.init(integrations=["pydantic-ai"])`, which instruments Pydantic AI through Logfire.
