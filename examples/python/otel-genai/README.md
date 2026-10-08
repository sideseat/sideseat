# OpenTelemetry GenAI utilities (Python)

```bash
uv run --locked --directory examples/python/otel-genai sample --list
uv run --locked --directory examples/python/otel-genai sample server_tools              # native telemetry
uv run --locked --directory examples/python/otel-genai sample server_tools --sideseat   # SideSeat SDK
```

An application that records its own model calls with `opentelemetry-util-genai`, the helper library
OpenTelemetry's GenAI instrumentations are built on. The scenarios call the OpenAI Responses API with the
official client and record every request as one GenAI inference: the system instruction, the input sent and
the output received, each as the semantic conventions' message parts, built from the library's own
dataclasses. So the telemetry is the conventions' shapes exactly as their reference implementation writes
them, which is what this suite is for: a producer whose parts no framework reshapes.

The model is the harness's local OpenAI endpoint, so no credentials are needed. `server_tools` enables the
provider's own web search: the response holds the search beside the answer, and the application records it
as a server tool call and its response. `files` sends the image and the document inline, as blobs of the
image and document modalities.

Native mode installs a global tracer provider with an OTLP exporter, which is all the library needs. SideSeat
mode is the same program with `sideseat.init(integrations=[])`: there is no framework to switch on, so the SDK
only exports. In both, message content is the application's choice and is turned on with the library's own
`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=SPAN_ONLY`.

There is no `streaming`, `structured_output` or `reasoning` scenario, because the application records a
whole response either way and nothing those scenarios add reaches its telemetry. There are no agents and no
MCP client, so there is no `multi_agent` or `mcp_tools` scenario either.

The version matrix (`versions.toml`) covers releases from 1.2b0, the first to name its part classes as the
suite imports them.
