# Agno

```bash
uv run --locked --directory examples/python/agno sample --list
uv run --locked --directory examples/python/agno sample tool_use              # native telemetry
uv run --locked --directory examples/python/agno sample tool_use --sideseat   # SideSeat SDK
```

Native mode installs the OpenInference Agno instrumentor on a plain OpenTelemetry provider, as the Agno
documentation describes. SideSeat mode replaces that with `sideseat.init(integrations=["agno"])`.
The suite runs Claude on Bedrock through Agno's `agno.models.aws.Claude`.
