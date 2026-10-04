# Smolagents

```bash
uv run --locked --directory examples/python/smolagents sample --list
uv run --locked --directory examples/python/smolagents sample tool_use              # native telemetry
uv run --locked --directory examples/python/smolagents sample tool_use --sideseat   # SideSeat SDK
```

Native mode installs the OpenInference Smolagents instrumentor on a plain OpenTelemetry provider, as
the Smolagents documentation describes. SideSeat mode replaces that with
`sideseat.init(integrations=["smolagents"])`. The suite reaches Bedrock through `LiteLLMModel`.

Smolagents has no schema-constrained answers and accepts images but no documents, so the suite has no
`structured_output` or `files` scenario. It has no `mcp_tools` scenario either: its MCP client, built on
mcpadapt and MCP 1.x, reads the optional `_meta` parameter current MCP servers declare as a required
string, so every call to the example calculator fails validation.
