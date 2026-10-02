# AgentScope

```bash
uv run --locked --directory examples/python/agentscope sample --list
uv run --locked --directory examples/python/agentscope sample tool_use              # native telemetry
uv run --locked --directory examples/python/agentscope sample tool_use --sideseat   # SideSeat SDK
```

Native mode installs a plain OpenTelemetry provider and passes AgentScope's `TracingMiddleware` to each
agent, as the AgentScope documentation describes. SideSeat mode replaces that with
`sideseat.init(integrations=["agentscope"])`, which adds the middleware to every agent itself.

AgentScope has no Bedrock model, so its `AnthropicChatModel` reaches Claude through the Anthropic SDK's
Bedrock client; the suite's default model is `claude`.
