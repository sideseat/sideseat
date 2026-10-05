# Google ADK

```bash
uv run --locked --directory examples/python/adk sample --list
uv run --locked --directory examples/python/adk sample tool_use              # native telemetry
uv run --locked --directory examples/python/adk sample tool_use --sideseat   # SideSeat SDK
```

Native mode installs a plain OpenTelemetry provider globally, which ADK's built-in tracing uses, as the
ADK documentation describes. SideSeat mode replaces that with `sideseat.init(integrations=["google-adk"])`.
The suite reaches Claude on Bedrock through ADK's `LiteLlm` model wrapper.

- `structured_output` offers the schema as a tool the model chooses to call: `output_schema` asks
  LiteLLM for Bedrock's native output format or a forced tool choice, and current Claude models
  reject both.
- `error` answers the failing tool with `on_tool_error_callback`; without one ADK ends the run.
- `session` runs two traces in one ADK session, so the second request re-sends the first turn.
- ADK leaves inline images and documents out of the request it records, so native `files` holds the
  request text only; the SideSeat integration restores them.
