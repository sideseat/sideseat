# OpenAI Agents SDK

```bash
uv run --locked --directory examples/python/openai-agents sample --list
uv run --locked --directory examples/python/openai-agents sample tool_use              # native telemetry
uv run --locked --directory examples/python/openai-agents sample tool_use --sideseat   # SideSeat SDK
```

Native mode configures Logfire with no upload and an OTLP processor, then `logfire.instrument_openai_agents()`,
which is the instrumentation `sideseat.init(integrations=["openai-agents"])` installs in SideSeat mode, so the
two modes compare. The suite runs GPT-6.1-sol on Bedrock's OpenAI-compatible endpoint through
`OpenAIResponsesModel`: the model always reasons, and Bedrock serves function tools to it only on the
Responses API.

- There is no `reasoning` scenario: Bedrock rejects `reasoning.summary` for GPT-6.1-sol, so its reasoning
  is never visible. Reaching Claude instead needs the SDK's LiteLLM model, and current LiteLLM requires
  OpenAI 2.x while the Agents SDK requires 3.x.
- `error` passes the tool's error to the model; the SDK's default tells it only that the tool failed.
- Logfire records `files`' image as its data URL in text and the PDF by its file name only.
