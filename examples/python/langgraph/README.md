# LangGraph

```bash
uv run --locked --directory examples/python/langgraph sample --list
uv run --locked --directory examples/python/langgraph sample tool_use              # native telemetry
uv run --locked --directory examples/python/langgraph sample tool_use --sideseat   # SideSeat SDK
```

Every scenario runs a LangGraph `StateGraph` that loops between a Bedrock Converse chat model and a
`ToolNode` (`agent.py`). Native mode instruments it with the OpenInference LangChain instrumentor on a
plain OpenTelemetry provider. SideSeat mode replaces that with
`sideseat.init(integrations=["langgraph"])`.
