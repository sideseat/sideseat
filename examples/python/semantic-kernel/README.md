# Semantic Kernel

```bash
uv run --locked --directory examples/python/semantic-kernel sample --list
uv run --locked --directory examples/python/semantic-kernel sample tool_use              # native telemetry
uv run --locked --directory examples/python/semantic-kernel sample tool_use --sideseat   # SideSeat SDK
```

Native mode switches on Semantic Kernel's GenAI diagnostics and installs a plain OpenTelemetry
provider, as the Semantic Kernel documentation describes. SideSeat mode replaces that with
`sideseat.init(integrations=["semantic-kernel"])`. The suite uses the `BedrockChatCompletion`
connector.

The Bedrock connector sends only text and images, has no response schema, and exposes no thinking
configuration, so the suite has no `files`, `structured_output`, or `reasoning` scenario.
