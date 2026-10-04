# Semantic Kernel

```bash
uv run --locked --directory examples/python/semantic-kernel sample --list
uv run --locked --directory examples/python/semantic-kernel sample tool_use              # native telemetry
uv run --locked --directory examples/python/semantic-kernel sample tool_use --sideseat   # SideSeat SDK
```

Native mode switches on Semantic Kernel's GenAI diagnostics and installs a plain OpenTelemetry
provider, as the Semantic Kernel documentation describes. SideSeat mode replaces that with
`sideseat.init(integrations=["semantic-kernel"])`. The suite uses the `BedrockChatCompletion`
connector, adjusted in `models.py` for current Claude models: the connector rejects their reasoning
blocks and sends parallel tool results in separate messages, which Converse refuses.

Semantic Kernel writes prompts and completions as Python log records rather than span attributes, so
the captured traces hold only the agents' outputs and the tool calls; the fixtures stay as they are until
capture records logs and SideSeat reads conversations from them.

The Bedrock connector sends only text and images, has no response schema, and exposes no thinking
configuration, so the suite has no `files`, `structured_output`, or `reasoning` scenario.
