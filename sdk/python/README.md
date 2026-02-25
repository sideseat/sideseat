# SideSeat Python SDK

**AI Development Workbench** — Debug, trace, and understand your AI agents.

[![PyPI](https://img.shields.io/pypi/v/sideseat)](https://pypi.org/project/sideseat/)
[![Python 3.9+](https://img.shields.io/badge/python-3.9%2B-blue)](https://www.python.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

## Table of Contents

- [What is SideSeat?](#what-is-sideseat)
- [Quick Start](#quick-start)
- [Installation](#installation)
- [Framework Examples](#framework-examples)
- [Provider Examples](#provider-examples)
- [Configuration](#configuration)
- [Advanced Usage](#advanced-usage)
- [Data and Privacy](#data-and-privacy)
- [Troubleshooting](#troubleshooting)
- [API Reference](#api-reference)

## What is SideSeat?

AI agents are hard to debug. Requests fly by, context builds up, and when something fails you're left guessing.

SideSeat captures every LLM call, tool call, and agent decision, then displays them in a web UI as they happen. Run it locally during development, or deploy to your private cloud for team visibility.

Built on [OpenTelemetry](https://opentelemetry.io/) — the open standard already supported by most AI frameworks.

**Features:**

- **Zero config** — Auto-detects and instruments your AI framework
- **Real-time tracing** — Watch LLM requests and tool calls as they happen
- **Message threading** — See full conversations, tool calls, and images
- **Cost tracking** — Automatic token counting and cost calculation

**Supported frameworks:** Strands Agents, LangGraph, LangChain, CrewAI, AutoGen, OpenAI Agents, Google ADK, PydanticAI

## Quick Start

**Requirements:** Python 3.9+, Node.js 18+ (for the server)

**1. Start the server**

```bash
npx sideseat
```

**2. Install and initialize**

```bash
pip install sideseat
# or
uv add sideseat
```

**Strands Agents:**

```python
from sideseat import SideSeat, Frameworks
from strands import Agent

SideSeat(framework=Frameworks.Strands)

agent = Agent()
response = agent("What is 2+2?")
print(response)
```

**Amazon Bedrock (Converse API):**

```python
from sideseat import SideSeat, Frameworks
import boto3

SideSeat(framework=Frameworks.Bedrock)

bedrock = boto3.client("bedrock-runtime", region_name="us-east-1")
response = bedrock.converse(
    modelId="us.anthropic.claude-sonnet-4-5-20250929-v1:0",
    messages=[{"role": "user", "content": [{"text": "What is 2+2?"}]}],
)

print(response["output"]["message"]["content"][0]["text"])
```

**3. View traces**

Open [localhost:5388](http://localhost:5388) and run your agent. Traces appear in real time.

## Installation

```bash
pip install sideseat                    # Core SDK
# or
uv add sideseat                        # Core SDK

# Extras for framework instrumentation:
pip install "sideseat[langgraph]"       # + LangGraph
pip install "sideseat[crewai]"          # + CrewAI
pip install "sideseat[autogen]"         # + AutoGen
pip install "sideseat[openai-agents]"   # + OpenAI Agents
pip install "sideseat[all]"             # All frameworks
```

Strands Agents and Google ADK require only the core SDK.

## Framework Examples

SideSeat auto-detects installed frameworks in this order: Strands, LangChain, CrewAI, AutoGen, OpenAI Agents, Google ADK, PydanticAI. When multiple frameworks are installed, use the `framework` parameter to select one explicitly. LangGraph is detected as LangChain — use `framework=Frameworks.LangGraph` to select it explicitly.

### Strands Agents

```python
from sideseat import SideSeat, Frameworks
from strands import Agent

SideSeat(framework=Frameworks.Strands)

agent = Agent()
response = agent("What is 2+2?")
print(response)
```

### Amazon Bedrock (Converse)

```python
import boto3
from sideseat import SideSeat, Frameworks

client = SideSeat(framework=Frameworks.Bedrock)
bedrock = boto3.client("bedrock-runtime", region_name="us-east-1")
model_id = "us.anthropic.claude-sonnet-4-5-20250929-v1:0"

with client.trace("geography-chat", session_id="session-abc", user_id="user-123"):
    messages = []

    messages.append({"role": "user", "content": [{"text": "What is the capital of France?"}]})
    response = bedrock.converse(modelId=model_id, messages=messages)
    messages.append(response["output"]["message"])

    messages.append({"role": "user", "content": [{"text": "What about Germany?"}]})
    response = bedrock.converse(modelId=model_id, messages=messages)
    print(response["output"]["message"]["content"][0]["text"])
```

### Google ADK

```python
import asyncio
from sideseat import SideSeat, Frameworks
from google.adk.agents import Agent
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.genai import types

SideSeat(framework=Frameworks.GoogleADK)

agent = Agent(
    model="gemini-2.5-flash",
    name="assistant",
    instruction="You are a helpful assistant.",
)

async def main():
    session_service = InMemorySessionService()
    runner = Runner(agent=agent, app_name="my_app", session_service=session_service)
    session = await session_service.create_session(app_name="my_app", user_id="user")
    message = types.Content(role="user", parts=[types.Part(text="What is 2+2?")])
    async for event in runner.run_async(
        session_id=session.id, user_id="user", new_message=message
    ):
        if event.content and event.content.parts:
            for part in event.content.parts:
                if hasattr(part, "text") and part.text:
                    print(part.text)

asyncio.run(main())
```

### LangGraph

```python
from sideseat import SideSeat, Frameworks
from langgraph.prebuilt import create_react_agent
from langchain_openai import ChatOpenAI

SideSeat(framework=Frameworks.LangGraph)

llm = ChatOpenAI(model="gpt-5-mini")
agent = create_react_agent(llm, tools=[])
result = agent.invoke({"messages": [("user", "What is 2+2?")]})
print(result["messages"][-1].content)
```

### CrewAI

```python
from sideseat import SideSeat, Frameworks
from crewai import Agent, Task, Crew

SideSeat(framework=Frameworks.CrewAI)

researcher = Agent(
    role="Researcher",
    goal="Find information",
    backstory="Expert researcher",
)

task = Task(
    description="Research AI trends",
    expected_output="Summary of trends",
    agent=researcher,
)

crew = Crew(agents=[researcher], tasks=[task])

result = crew.kickoff()
print(result)
```

### AutoGen

```python
from sideseat import SideSeat, Frameworks
from autogen import AssistantAgent, UserProxyAgent

SideSeat(framework=Frameworks.AutoGen)

llm_config = {"config_list": [{"model": "gpt-5-mini"}]}
assistant = AssistantAgent("assistant", llm_config=llm_config)
user = UserProxyAgent("user", human_input_mode="NEVER")
user.initiate_chat(assistant, message="Hello!")
```

### OpenAI Agents

```python
from sideseat import SideSeat, Frameworks
from agents import Agent, Runner

SideSeat(framework=Frameworks.OpenAIAgents)

agent = Agent(name="Assistant", instructions="You are helpful.")
result = Runner.run_sync(agent, "What is the capital of France?")
print(result.final_output)
```

### LangChain

```python
from sideseat import SideSeat, Frameworks
from langchain_openai import ChatOpenAI
from langchain_core.messages import HumanMessage

SideSeat(framework=Frameworks.LangChain)

llm = ChatOpenAI(model="gpt-5-mini")
response = llm.invoke([HumanMessage(content="Hello!")])
print(response.content)
```

### PydanticAI

```python
from sideseat import SideSeat, Frameworks
from pydantic_ai import Agent

SideSeat(framework=Frameworks.PydanticAI)

agent = Agent("openai:gpt-5-mini", system_prompt="Be concise.")
result = agent.run_sync("What is Python?")
print(result.data)
```

## Provider Examples

Use SideSeat directly with cloud provider SDKs, without an agent framework.

### Amazon Bedrock

```python
from sideseat import SideSeat, Frameworks
import boto3

SideSeat(framework=Frameworks.Bedrock)

bedrock = boto3.client("bedrock-runtime", region_name="us-east-1")
response = bedrock.converse(
    modelId="us.anthropic.claude-sonnet-4-5-20250929-v1:0",
    messages=[{"role": "user", "content": [{"text": "What is 2+2?"}]}],
)

print(response["output"]["message"]["content"][0]["text"])
```

### Anthropic

```python
from sideseat import SideSeat, Frameworks
import anthropic

SideSeat(framework=Frameworks.Anthropic)

client = anthropic.Anthropic()
message = client.messages.create(
    model="claude-sonnet-4-5-20250929",
    max_tokens=1024,
    messages=[{"role": "user", "content": "What is 2+2?"}],
)

print(message.content[0].text)
```

### OpenAI

```python
from sideseat import SideSeat, Frameworks
from openai import OpenAI

SideSeat(framework=Frameworks.OpenAI)

client = OpenAI()
response = client.chat.completions.create(
    model="gpt-5-mini",
    messages=[{"role": "user", "content": "What is 2+2?"}],
)

print(response.choices[0].message.content)
```

## Configuration

### Environment Variables

| Variable              | Default                 | Description                  |
| --------------------- | ----------------------- | ---------------------------- |
| `SIDESEAT_ENDPOINT`   | `http://127.0.0.1:5388` | Server URL                   |
| `SIDESEAT_PROJECT`    | `default`               | Project identifier           |
| `SIDESEAT_API_KEY`    | —                       | Authentication key           |
| `SIDESEAT_DISABLED`   | `false`                 | Disable all telemetry        |
| `SIDESEAT_DEBUG`      | `false`                 | Enable verbose logging       |

### Constructor Parameters

```python
SideSeat(
    endpoint="http://localhost:5388",
    project_id="my-project",
    api_key="pk-...",
    framework=Frameworks.Strands,
    auto_instrument=True,
    service_name="my-app",
    service_version="1.0.0",
    enable_traces=True,
    enable_metrics=True,
    enable_logs=False,
    capture_content=True,
    encode_binary=True,
    disabled=False,
    debug=False,
)
```

| Parameter         | Type   | Default                 | Description                       |
| ----------------- | ------ | ----------------------- | --------------------------------- |
| `endpoint`        | `str`  | `http://127.0.0.1:5388` | Server URL                        |
| `project_id`      | `str`  | `default`               | Project identifier                |
| `api_key`         | `str`  | `None`                  | Authentication key                |
| `framework`       | `str \| list` | Auto-detected     | Framework/providers to instrument |
| `auto_instrument` | `bool` | `True`                  | Enable framework instrumentation  |
| `service_name`    | `str`  | Framework name          | Application name in traces        |
| `service_version` | `str`  | Framework version       | Application version               |
| `enable_traces`   | `bool` | `True`                  | Export trace spans                |
| `enable_metrics`  | `bool` | `True`                  | Export metrics                    |
| `enable_logs`     | `bool` | `False`                 | Export logs                       |
| `capture_content` | `bool` | `True`                  | Capture LLM prompts and responses |
| `encode_binary`   | `bool` | `True`                  | Base64 encode binary data         |
| `disabled`        | `bool` | `False`                 | Disable all telemetry             |
| `debug`           | `bool` | `False`                 | Enable verbose logging            |

**Resolution order:** Constructor → `SIDESEAT_*` env → `OTEL_*` env → defaults

## Advanced Usage

### Context Manager

```python
with SideSeat() as client:
    run_my_agent()
# Traces flushed and connection closed automatically
```

### Global Instance

```python
import sideseat

sideseat.init(project_id="my-project")  # Initialize once
client = sideseat.get_client()          # Access anywhere
sideseat.shutdown()                     # Clean up
```

### Custom Spans

```python
client = SideSeat()

with client.span("process-request") as span:
    span.set_attribute("user_id", "12345")
    result = do_work()
# Exceptions recorded automatically with stack traces
```

### Async Support

```python
import asyncio
from sideseat import SideSeat

async def main():
    with SideSeat():
        result = await my_async_agent.run("Hello")
        print(result)

asyncio.run(main())
```

### Debug Exporters

```python
client = SideSeat()
client.telemetry.setup_console_exporter()             # Print to stdout
client.telemetry.setup_file_exporter("traces.jsonl")  # Write to file
```

### Disabled Mode

```python
SideSeat(disabled=True)  # Or set SIDESEAT_DISABLED=true
```

### Existing OpenTelemetry Setup

If a `TracerProvider` already exists, SideSeat adds its exporter to the existing provider.

### Unsupported Frameworks

```python
SideSeat(auto_instrument=False)
# Use your framework's native OpenTelemetry instrumentation
```

## Data and Privacy

**What is collected:**

- Trace spans with timing and hierarchy
- LLM prompts and responses (when `capture_content=True`)
- Token counts and model names
- Errors and stack traces

**Where it goes:**

All data is sent to your self-hosted server. Nothing leaves your infrastructure.

**Resilience:**

- Up to 2,048 spans buffered in memory
- Batched exports every 5 seconds
- 30-second timeout per export
- Server downtime does not affect your application

## Troubleshooting

| Problem                  | Solution                                            |
| ------------------------ | --------------------------------------------------- |
| Connection refused       | Server not running. Run `npx sideseat`              |
| No traces appear         | Check endpoint with `SIDESEAT_DEBUG=true`           |
| Wrong framework detected | Set `framework=Frameworks.X` explicitly             |
| Duplicate traces         | Initialize `SideSeat()` once per process            |
| Import error for extras  | Install extras: `pip install "sideseat[langgraph]"` |

## API Reference

### SideSeat

```python
client = SideSeat(**kwargs)
```

**Properties:**

| Name              | Type              | Description                   |
| ----------------- | ----------------- | ----------------------------- |
| `config`          | `Config`          | Immutable configuration       |
| `telemetry`       | `TelemetryClient` | Access to debug exporters     |
| `tracer_provider` | `TracerProvider`  | OpenTelemetry tracer provider |
| `is_disabled`     | `bool`            | Whether telemetry is disabled |

**Methods:**

| Name                           | Returns                | Description                       |
| ------------------------------ | ---------------------- | --------------------------------- |
| `span(name, **kwargs)`         | `ContextManager[Span]` | Create a custom span              |
| `trace(name, **kwargs)`        | `ContextManager[Span]` | Create a root span (trace group)  |
| `get_tracer(name)`             | `Tracer`               | Get an OpenTelemetry tracer       |
| `force_flush(timeout_millis)`  | `bool`                 | Export pending spans immediately  |
| `validate_connection(timeout)` | `bool`                 | Test server connectivity          |
| `shutdown(timeout_millis)`     | `None`                 | Flush pending spans and shut down |

### Frameworks

```python
Frameworks.Strands
Frameworks.LangGraph
Frameworks.LangChain
Frameworks.CrewAI
Frameworks.AutoGen
Frameworks.OpenAIAgents
Frameworks.GoogleADK
Frameworks.PydanticAI
```

### Providers (via Frameworks)

```python
Frameworks.Bedrock    # Amazon Bedrock (patches botocore)
Frameworks.OpenAI     # OpenAI (instruments openai SDK)
Frameworks.Anthropic  # Anthropic (instruments anthropic SDK)
Frameworks.VertexAI   # Google Vertex AI (instruments vertexai SDK)
```

### Module Functions

| Function           | Returns    | Description               |
| ------------------ | ---------- | ------------------------- |
| `init(**kwargs)`   | `SideSeat` | Create global instance    |
| `get_client()`     | `SideSeat` | Get global instance       |
| `shutdown()`       | `None`     | Shut down global instance |
| `is_initialized()` | `bool`     | Check if initialized      |

### Utilities

| Function               | Description                            |
| ---------------------- | -------------------------------------- |
| `encode_value(value)`  | JSON-encode a value; base64 for binary |
| `span_to_dict(span)`   | Convert span to dictionary             |
| `JsonFileSpanExporter` | JSONL file exporter class              |

## Resources

- [Documentation](https://sideseat.ai/docs)
- [GitHub Discussions](https://github.com/sideseat/sideseat/discussions)
- [Issue Tracker](https://github.com/sideseat/sideseat/issues)

## License

[MIT](LICENSE)
