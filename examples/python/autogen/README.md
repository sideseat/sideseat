# AutoGen

```bash
uv run --locked --directory examples/python/autogen sample --list
uv run --locked --directory examples/python/autogen sample tool_use              # native telemetry
uv run --locked --directory examples/python/autogen sample tool_use --sideseat   # SideSeat SDK
```

The scenarios run AutoGen AgentChat's `AssistantAgent` on autogen-ext's `OpenAIChatCompletionClient`,
pointed at the harness's deterministic local OpenAI endpoint (the `fake-openai` model, started
in-process; no credentials). The OpenInference AgentChat instrumentor records model requests only from
AutoGen's OpenAI clients - with its Anthropic client a trace holds agent spans and no conversation -
and Bedrock's OpenAI-compatible endpoint serves no model that takes function tools on Chat Completions.

Native mode instruments AgentChat with the OpenInference instrumentor on a plain, global
OpenTelemetry provider, which also receives AgentChat's own GenAI spans. SideSeat mode replaces that
with `sideseat.init(integrations=["autogen"])`.

Two catalog scenarios are absent: `files`, because AgentChat's multimodal messages carry text and
images but not documents, and `reasoning`, because AutoGen's chat client reads no reasoning from Chat
Completions.
