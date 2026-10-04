# Claude Agent SDK

```bash
uv run --locked --directory examples/python/claude-agent-sdk sample --list
uv run --locked --directory examples/python/claude-agent-sdk sample tool_use              # native telemetry
uv run --locked --directory examples/python/claude-agent-sdk sample tool_use --sideseat   # SideSeat SDK
```

The Agent SDK runs the Claude Code CLI as a child process, and the CLI carries its own
OpenTelemetry instrumentation, configured by environment variables. Native mode sets the documented
`CLAUDE_CODE_*` and `OTEL_*` variables (`native.py`); SideSeat mode replaces that with
`sideseat.init(integrations=["claude-agent-sdk"])`, which adds the same variables to every
`ClaudeAgentOptions`. In both modes the Agent SDK passes the active span to the CLI as `TRACEPARENT`,
so the CLI's spans join the scenario's trace.

The CLI runs on Amazon Bedrock (`CLAUDE_CODE_USE_BEDROCK=1`) with the model in every model slot.
Tools are the shared ones, served by an in-process SDK MCP server (`tools.py`); built-in tools are
off. Each run ignores the developer's settings, `CLAUDE.md` files, and memory, and works in a scratch
directory outside any repository, so a capture holds only the scenario.
