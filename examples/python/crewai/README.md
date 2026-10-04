# CrewAI

```bash
uv run --locked --directory examples/python/crewai sample --list
uv run --locked --directory examples/python/crewai sample tool_use              # native telemetry
uv run --locked --directory examples/python/crewai sample tool_use --sideseat   # SideSeat SDK
```

Most scenarios run a one-task crew (`crew.py`) on CrewAI's native Bedrock Converse provider; the
shared system prompt is the agent's backstory. `multi_turn` asks each question as a task that takes
the answered ones as context, and `multi_agent` runs a sequential crew whose writing task takes the
research task's output as context. Native mode instruments CrewAI with the OpenInference CrewAI
instrumentor on a plain OpenTelemetry provider; SideSeat mode replaces that with
`sideseat.init(integrations=["crewai"])`.

Three catalog scenarios are absent because CrewAI 1.15.23's Bedrock provider cannot run them with
Claude Sonnet 5.5:

- `structured_output`: the provider forces the structured-output tool, and the model refuses forced
  tool choice;
- `files`: the provider's vision allowlist names neither Claude 5 models nor global inference
  profiles, so it refuses image and document inputs;
- `streaming`: a streamed response that calls a tool ends with the call's arguments as text and no
  tool run.
