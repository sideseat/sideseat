# LlamaIndex

```bash
uv run --locked --directory examples/python/llamaindex sample --list
uv run --locked --directory examples/python/llamaindex sample tool_use              # native telemetry
uv run --locked --directory examples/python/llamaindex sample tool_use --sideseat   # SideSeat SDK
```

The scenarios run LlamaIndex's `FunctionAgent` and `AgentWorkflow` on its Bedrock Converse LLM.
Native mode instruments LlamaIndex with the OpenInference instrumentor on a plain OpenTelemetry
provider; SideSeat mode replaces that with `sideseat.init(integrations=["llama-index"])`.
