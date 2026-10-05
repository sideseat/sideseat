# SideSeat Python SDK

[![PyPI](https://img.shields.io/pypi/v/sideseat)](https://pypi.org/project/sideseat/)
[![Python 3.11+](https://img.shields.io/badge/python-3.11%2B-blue)](https://www.python.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

`sideseat` is an OpenTelemetry distribution for AI applications. One call configures tracing, logs,
and metrics for a [SideSeat](https://sideseat.ai) project, switches on your framework's telemetry, and
attributes every span to the right session and user.

## Install

```bash
pip install sideseat        # or: uv add sideseat
npx sideseat                # a local SideSeat server on http://localhost:5388
```

Python 3.11 or newer. Some integrations need an extra, for example `pip install "sideseat[langgraph]"`;
see [Integrations](#integrations).

## Quick start

```python
import sideseat
from strands import Agent

sideseat.init(integrations=["strands"])

agent = Agent(model="global.anthropic.claude-sonnet-5-5")

with sideseat.session("conversation-42", user_id="user-7"):
    agent("Plan a weekend in Lisbon.")
    agent("What should I eat there?")
```

Open [localhost:5388](http://localhost:5388): the two turns appear as traces of one session.

`init` sends to `http://127.0.0.1:5388` and the `default` project. Pass `endpoint=`, `project=`, and
`api_key=`, or set `SIDESEAT_ENDPOINT`, `SIDESEAT_PROJECT_ID`, and `SIDESEAT_API_KEY`. Every option is
listed on the [configuration page](https://sideseat.ai/docs/sdks/python/configuration/).

## Sessions and users

`sideseat.session(session_id, user_id=...)` is a scope, not a span. Every span started inside it,
including the ones your framework creates, gets `session.id` and `user.id`. The values follow the
OpenTelemetry context through `async` code and are never sent as W3C baggage.

## Traces and spans

```python
with sideseat.trace("plan-trip", session_id="s-1", user_id="u-1"):
    with sideseat.span("retrieve-context") as span:
        span.set_attribute("app.documents", 4)
        documents = load_documents()
    agent(f"Plan a trip using: {documents}")


@sideseat.observe()
def load_documents() -> list[str]:
    ...
```

`trace()` always starts a new root span; `span()` starts a child of the active span; `observe()` wraps
each call of a sync or async function in a span named after it. Exceptions are recorded and re-raised.

## Integrations

Pass the frameworks and providers you use; the first one is the primary integration and names the
service. Without `integrations`, the installed framework is detected. Provider client libraries are
never detected, so name them explicitly.

| Name | Framework or provider | Extra |
| --- | --- | --- |
| `strands` | Strands Agents | |
| `langgraph`, `langchain` | LangGraph, LangChain | `langgraph`, `langchain` |
| `crewai` | CrewAI | `crewai` |
| `autogen` | AutoGen AgentChat | `autogen` |
| `ag2` | AG2 | `ag2` |
| `openai-agents` | OpenAI Agents SDK | `openai-agents` |
| `google-adk` | Google Agent Development Kit | |
| `pydantic-ai` | Pydantic AI | `pydantic-ai` |
| `agent-framework` | Microsoft Agent Framework | |
| `semantic-kernel` | Semantic Kernel | |
| `claude-agent-sdk` | Claude Agent SDK | |
| `agno`, `smolagents`, `llama-index` | Agno, smolagents, LlamaIndex | same name |
| `agentscope`, `haystack` | AgentScope, Haystack | same name |
| `browser-use` | Browser Use | `browser-use` |
| `langflow`, `openinference` | Langflow, hand-written OpenInference spans | |
| `logfire`, `traceloop` | Logfire, TraceLoop (OpenLLMetry) | same name |
| `langfuse` | Langfuse: `@observe` and its drop-in OpenAI, LangChain and other wrappers record on SideSeat's provider | `langfuse` |
| `langsmith` | LangSmith's OpenTelemetry mode: LangChain and LangGraph runs become spans on SideSeat's provider | `langsmith` |
| `bedrock` | Amazon Bedrock through boto3 | `bedrock` |
| `openai`, `azure-openai` | OpenAI and Azure OpenAI clients | same name |
| `anthropic` | Anthropic client | `anthropic` |
| `google-genai`, `vertex-ai` | Google Gen AI SDK | same name |

Several integrations can run together, for example `integrations=["langgraph", "bedrock"]`. A requested
integration whose package is missing raises `IntegrationError` with the install command.

### Amazon Bedrock

```bash
pip install "sideseat[bedrock]" boto3
```

```python
import boto3
import sideseat

sideseat.init(integrations=["bedrock"])

# Create the client after init: the integration instruments clients as they are created.
bedrock = boto3.client("bedrock-runtime", region_name="us-east-1")

with sideseat.session("geography-chat", user_id="user-123"):
    response = bedrock.converse(
        modelId="global.anthropic.claude-sonnet-5-5",
        messages=[{"role": "user", "content": [{"text": "What is the capital of France?"}]}],
    )
    print(response["output"]["message"]["content"][0]["text"])
```

### Anthropic

```bash
pip install "sideseat[anthropic]" anthropic
```

```python
import anthropic
import sideseat

sideseat.init(integrations=["anthropic"])

message = anthropic.Anthropic().messages.create(
    model="claude-sonnet-5-5",
    max_tokens=1024,
    messages=[{"role": "user", "content": "What is 2+2?"}],
)
print(message.content[0].text)
```

### OpenAI

```bash
pip install "sideseat[openai]" openai
```

```python
import sideseat
from openai import OpenAI

sideseat.init(integrations=["openai"])

response = OpenAI().responses.create(
    model="gpt-6.1-sol",
    instructions="Answer in one sentence.",
    input="What is the speed of light?",
)
print(response.output_text)
```

### Vertex AI

```bash
pip install "sideseat[vertex-ai]" google-genai
```

```python
import sideseat
from google import genai

sideseat.init(integrations=["vertex-ai"])

client = genai.Client(enterprise=True, project="your-project", location="us-central1")
response = client.models.generate_content(
    model="gemini-2.5-flash",
    contents="What is the speed of light?",
)
print(response.text)
```

Each framework's page under [Integrations](https://sideseat.ai/docs/integrations/) shows its setup.

## Flushing and shutdown

Telemetry is exported in batches. `sideseat.shutdown()` runs at exit, including on `SIGTERM` when no
other handler is installed, and flushes everything; call `sideseat.flush()` in a short-lived process,
such as a serverless handler, before it is frozen. Both take a timeout in milliseconds that bounds the
whole call and return whether everything was exported within it.

## Testing

`sideseat.testing.capture()` initializes SideSeat without network export and records spans in memory:

```python
import sideseat
from sideseat.testing import capture


def test_the_agent_reports_its_session():
    with capture(integrations=["strands"]) as spans:
        with sideseat.session("s-1"):
            run_agent()
    assert all(span.attributes["session.id"] == "s-1" for span in spans.finished())
```

`disabled=True` or `SIDESEAT_DISABLED=true` configures nothing; every SideSeat call still works and
records nothing.

## Runtime channel

With `pip install "sideseat[runtime]"`, agents registered over the
[runtime channel](https://sideseat.ai/docs/sdks/python/runtime/) appear in the SideSeat Playground,
which can inspect and run them.

## Resources

- [Documentation](https://sideseat.ai/docs/sdks/python/)
- [Issue tracker](https://github.com/sideseat/sideseat/issues)

## License

[MIT](LICENSE)
