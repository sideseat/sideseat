# AutoGen

```bash
uv run --locked --directory examples/python/autogen sample --list
uv run --locked --directory examples/python/autogen sample tool_use              # native telemetry
uv run --locked --directory examples/python/autogen sample tool_use --sideseat   # SideSeat SDK
```

The scenarios run AutoGen AgentChat's `AssistantAgent` on Claude through the Anthropic SDK's
Bedrock client, which autogen-ext's Anthropic chat client accepts. Native mode instruments
AgentChat with the OpenInference instrumentor on a plain, global OpenTelemetry provider, which also
receives AgentChat's own GenAI spans. SideSeat mode replaces that with
`sideseat.init(integrations=["autogen"])`.

Two catalog scenarios are absent because autogen-ext's Anthropic client cannot express them:
`structured_output` (it raises for a schema-constrained answer) and `files` (its user messages
carry text and images, not documents).
