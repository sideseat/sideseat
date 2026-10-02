# Strands Agents

```bash
uv run --locked --directory examples/python/strands sample --list
uv run --locked --directory examples/python/strands sample tool_use              # native telemetry
uv run --locked --directory examples/python/strands sample tool_use --sideseat   # SideSeat SDK
```

Native mode uses `StrandsTelemetry` on a plain OpenTelemetry provider, as the Strands documentation
describes. SideSeat mode replaces that with `sideseat.init(integrations=["strands"])`.
