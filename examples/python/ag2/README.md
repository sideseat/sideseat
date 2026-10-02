# AG2

```bash
uv run --locked --directory examples/python/ag2 sample --list
uv run --locked --directory examples/python/ag2 sample tool_use              # native telemetry
uv run --locked --directory examples/python/ag2 sample tool_use --sideseat   # SideSeat SDK
```

The scenarios run AG2 agents on its Bedrock Converse model config. AG2 traces through
`TelemetryMiddleware` on each agent: native mode attaches it, exporting through a plain OTLP
provider (`agent.py`); SideSeat mode leaves it out and lets `sideseat.init(integrations=["ag2"])`
attach it to every agent.
