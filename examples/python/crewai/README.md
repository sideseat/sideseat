# CrewAI

```bash
uv run --locked --directory examples/python/crewai sample --list
uv run --locked --directory examples/python/crewai sample tool_use              # native telemetry
uv run --locked --directory examples/python/crewai sample tool_use --sideseat   # SideSeat SDK
```

Most scenarios run a one-task crew (`crew.py`) on CrewAI's native Bedrock Converse provider; the
shared system prompt is the agent's backstory. `multi_turn` kicks off one agent with the growing
message history, and `multi_agent` runs a sequential crew whose writing task takes the research
task's output as context. Native mode instruments CrewAI with the OpenInference CrewAI
instrumentor on a plain OpenTelemetry provider; SideSeat mode replaces that with
`sideseat.init(integrations=["crewai"])`.
