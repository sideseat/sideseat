# Semantic Kernel

```bash
uv run --locked --directory examples/python/semantic-kernel sample --list
uv run --locked --directory examples/python/semantic-kernel sample tool_use              # native telemetry
uv run --locked --directory examples/python/semantic-kernel sample tool_use --sideseat   # SideSeat SDK
```

Native mode switches on Semantic Kernel's GenAI diagnostics, installs a plain OpenTelemetry
provider, and sends the `semantic_kernel` logger's records to an OTLP log exporter, as the Semantic
Kernel documentation describes. SideSeat mode replaces that with
`sideseat.init(integrations=["semantic-kernel"])`. The suite uses the `BedrockChatCompletion`
connector, adjusted in `models.py` for current Claude models: the connector rejects their reasoning
blocks and sends parallel tool results in separate messages, which Converse refuses.

Semantic Kernel writes prompts and completions as Python log records rather than span attributes, so
both modes export logs beside the traces; without them a trace holds only the agents' outputs and the
tool calls. Tool results are the Python `str()` of each function's return value, which is what the
connector sends the model.

The Bedrock connector sends only text and images, has no response schema, and exposes no thinking
configuration, so the suite has no `files`, `structured_output`, or `reasoning` scenario.
