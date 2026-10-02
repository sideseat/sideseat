# Microsoft Agent Framework

```bash
uv run --locked --directory examples/python/agent-framework sample --list
uv run --locked --directory examples/python/agent-framework sample tool_use              # native telemetry
uv run --locked --directory examples/python/agent-framework sample tool_use --sideseat   # SideSeat SDK
```

Native mode installs a plain OpenTelemetry provider and calls `enable_sensitive_telemetry()`, as the
Agent Framework documentation describes for an application that owns its provider. SideSeat mode
replaces that with `sideseat.init(integrations=["agent-framework"])`.

The suite runs Claude on Bedrock through `AnthropicBedrockClient`, so its default model is `claude`.
Neither that client nor `BedrockChatClient` sends documents, so the suite has no `files` scenario.
