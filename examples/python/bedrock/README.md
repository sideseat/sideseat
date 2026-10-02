# Amazon Bedrock (boto3)

```bash
uv run --locked --directory examples/python/bedrock sample --list
uv run --locked --directory examples/python/bedrock sample tool_use              # native telemetry
uv run --locked --directory examples/python/bedrock sample tool_use --sideseat   # SideSeat SDK
```

The scenarios call the bedrock-runtime Converse and ConverseStream APIs directly, running the tool loop
themselves. Native mode is OpenTelemetry's botocore instrumentation, which records the calls as spans
and their messages as GenAI log events, so it exports logs as well as traces. SideSeat mode replaces
that with `sideseat.init(integrations=["bedrock"])`.

A provider client has no agents and no MCP client, so the suite has no `multi_agent` or `mcp_tools`
scenario.
