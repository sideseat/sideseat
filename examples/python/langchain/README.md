# LangChain

```bash
uv run --locked --directory examples/python/langchain sample --list
uv run --locked --directory examples/python/langchain sample tool_use              # native telemetry
uv run --locked --directory examples/python/langchain sample tool_use --sideseat   # SideSeat SDK
```

The scenarios use LangChain's own primitives: prompt templates and chains, a Bedrock Converse chat
model, and a tool-calling loop over `bind_tools` (`agent.py`). Agents built on LangGraph are
covered by the LangGraph suite. Native mode instruments the application with the OpenInference
LangChain instrumentor on a plain OpenTelemetry provider; SideSeat mode replaces that with
`sideseat.init(integrations=["langchain"])`.

There is no `multi_agent` scenario: LangChain's multi-agent patterns run on LangGraph.
