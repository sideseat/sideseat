/**
 * Framework and provider integration instructions rendered by the telemetry page.
 *
 * Split out of telemetry.tsx so the data can be exported and unit-tested: a non-component
 * export from a component module breaks React Fast Refresh
 * (react-refresh/only-export-components). The snippets are checked by
 * __tests__/telemetry-snippets.test.ts, which parses every Python snippet with a real
 * interpreter rather than trusting them by eye.
 */

import type { ReactNode } from "react";

export type Framework = {
  id: string;
  name: string;
  group: "Providers" | "Frameworks";
  lang: "python" | "javascript";
  docUrl: string;
  install: string;
  code: () => string;
  run: string;
  note?: string;
  banner?: ReactNode;
  altInstall?: string;
  altCode?: () => string;
  /**
   * Skip the shared "Configure the exporter" block on the direct-OTLP path. Set for the
   * Claude Agent SDK: the Claude Code CLI exports OTLP from its own process, so a
   * TracerProvider in the host process exports nothing and only misleads.
   */
  altSkipProviderSetup?: boolean;
};

export const FRAMEWORKS: Framework[] = [
  // — Providers —
  {
    id: "bedrock",
    name: "Amazon Bedrock",
    group: "Providers",
    lang: "python",
    docUrl: "https://sideseat.ai/docs/integrations/providers/bedrock/",
    install: 'pip install "sideseat[bedrock]" boto3',
    code: () => `import boto3
import sideseat

sideseat.init(integrations=["bedrock"])

# Create the client after init: the integration instruments clients as they are created.
bedrock = boto3.client("bedrock-runtime", region_name="us-east-1")

response = bedrock.converse(
    modelId="global.anthropic.claude-sonnet-5-5",
    system=[{"text": "Answer in one sentence."}],
    messages=[{"role": "user", "content": [{"text": "What is the speed of light?"}]}],
    inferenceConfig={"maxTokens": 128},
)

print(response["output"]["message"]["content"][0]["text"])`,
    altInstall:
      "pip install boto3 opentelemetry-instrumentation-botocore opentelemetry-exporter-otlp",
    altCode: () => `from opentelemetry.instrumentation.botocore import BotocoreInstrumentor

BotocoreInstrumentor().instrument(tracer_provider=provider)`,
    run: "python app.py",
  },
  {
    id: "anthropic",
    name: "Anthropic",
    group: "Providers",
    lang: "python",
    docUrl: "https://sideseat.ai/docs/integrations/providers/anthropic/",
    install: 'pip install "sideseat[anthropic]"',
    code: () => `import anthropic
import sideseat

sideseat.init(integrations=["anthropic"])

client = anthropic.Anthropic()
message = client.messages.create(
    model="claude-sonnet-5-5",
    system="Answer in one sentence.",
    max_tokens=1024,
    messages=[{"role": "user", "content": "What is the speed of light?"}],
)

print(message.content[0].text)`,
    altInstall: 'pip install anthropic "logfire[anthropic]>=4.29.0" opentelemetry-exporter-otlp',
    altCode: () => `import logfire

logfire.configure(send_to_logfire=False, console=False)
logfire.instrument_anthropic()`,
    run: "python app.py",
  },
  {
    id: "openai",
    name: "OpenAI",
    group: "Providers",
    lang: "python",
    docUrl: "https://sideseat.ai/docs/integrations/providers/openai/",
    install: 'pip install "sideseat[openai]"',
    code: () => `from openai import OpenAI
import sideseat

sideseat.init(integrations=["openai"])

client = OpenAI()
response = client.chat.completions.create(
    model="gpt-6.1-sol",
    messages=[
        {"role": "system", "content": "Answer in one sentence."},
        {"role": "user", "content": "What is the speed of light?"},
    ],
    max_completion_tokens=1024,
)

print(response.choices[0].message.content)`,
    altInstall: 'pip install openai "logfire[openai]>=4.29.0" opentelemetry-exporter-otlp',
    altCode: () => `import logfire

logfire.configure(send_to_logfire=False, console=False)
logfire.instrument_openai()`,
    run: "python app.py",
  },
  {
    id: "azure-openai",
    name: "Azure OpenAI",
    group: "Providers",
    lang: "python",
    docUrl: "https://sideseat.ai/docs/integrations/providers/azure/",
    install: 'pip install openai "sideseat[azure-openai]"',
    code: () => `from openai import AzureOpenAI
import sideseat

sideseat.init(integrations=["azure-openai"])

azure = AzureOpenAI(
    api_key="your-api-key",
    api_version="2024-02-01",
    azure_endpoint="https://your-resource.openai.azure.com",
)

response = azure.chat.completions.create(
    model="gpt-6.1-sol",  # Your deployment name
    messages=[
        {"role": "system", "content": "Answer in one sentence."},
        {"role": "user", "content": "What is the speed of light?"},
    ],
)

print(response.choices[0].message.content)`,
    altInstall: 'pip install openai "logfire>=4.29.0" opentelemetry-exporter-otlp',
    altCode: () => `import logfire

# Azure OpenAI goes through the OpenAI SDK, so the OpenAI instrumentor covers it.
logfire.configure(send_to_logfire=False, console=False)
logfire.instrument_openai()`,
    run: "python app.py",
  },
  {
    id: "google-gemini",
    name: "Google Gemini",
    group: "Providers",
    lang: "python",
    docUrl: "https://sideseat.ai/docs/integrations/providers/google-gemini/",
    install: 'pip install "sideseat[google-genai]"',
    code: () => `from google import genai
import sideseat

sideseat.init(integrations=["google-genai"])

client = genai.Client(api_key="your-api-key")

response = client.models.generate_content(
    model="gemini-2.5-flash",
    contents="What is the speed of light?",
)

print(response.text)`,
    altInstall:
      'pip install google-genai "logfire[google-genai]>=4.29.0" opentelemetry-exporter-otlp',
    altCode: () => `import logfire

logfire.configure(send_to_logfire=False, console=False)
logfire.instrument_google_genai()`,
    run: "python app.py",
  },
  {
    id: "vertex-ai",
    name: "Google Vertex AI",
    group: "Providers",
    lang: "python",
    docUrl: "https://sideseat.ai/docs/integrations/providers/vertex-ai/",
    install: 'pip install "sideseat[vertex-ai]"',
    code: () => `from google import genai
import sideseat

sideseat.init(integrations=["vertex-ai"])

client = genai.Client(
    enterprise=True,
    project="your-project",
    location="us-central1",
)
response = client.models.generate_content(
    model="gemini-2.5-flash",
    contents="What is 2+2?",
)
print(response.text)`,
    altInstall:
      'pip install google-genai "logfire[google-genai]>=4.29.0" opentelemetry-exporter-otlp',
    altCode: () => `import logfire

logfire.configure(send_to_logfire=False, console=False)
logfire.instrument_google_genai()`,
    run: "python app.py",
  },
  // — Frameworks —
  {
    id: "strands-python",
    name: "Strands (Python)",
    group: "Frameworks",
    lang: "python",
    docUrl:
      "https://strandsagents.com/latest/documentation/docs/user-guide/observability-evaluation/traces/",
    install: "pip install strands-agents sideseat",
    code: () => `from strands import Agent
import sideseat

sideseat.init(integrations=["strands"])

agent = Agent(model="global.anthropic.claude-sonnet-5-5")
response = agent("What is 2+2?")
print(response)`,
    altInstall: "pip install 'strands-agents[otel]'",
    altCode: () => `from strands.telemetry import StrandsTelemetry
from strands import Agent

telemetry = StrandsTelemetry()
telemetry.setup_otlp_exporter()
telemetry.setup_meter(enable_otlp_exporter=True)

agent = Agent(model="global.anthropic.claude-sonnet-5-5")
response = agent("What is 2+2?")
print(response)`,
    run: "python agent.py",
  },
  {
    id: "strands-typescript",
    name: "Strands (TypeScript)",
    group: "Frameworks",
    lang: "javascript",
    docUrl:
      "https://strandsagents.com/latest/documentation/docs/user-guide/observability-evaluation/traces/",
    install: "npm install @strands-agents/sdk @sideseat/sdk",
    code: () => `import * as sideseat from '@sideseat/sdk';
import { Agent } from '@strands-agents/sdk';

await sideseat.init({ integrations: ['strands'] });

const agent = new Agent({ model: 'global.anthropic.claude-sonnet-5-5' });
const result = await agent.invoke('What is 2+2?');
console.log(result.toString());`,
    altInstall:
      "npm install @strands-agents/sdk @opentelemetry/sdk-trace-node @opentelemetry/sdk-trace-base @opentelemetry/exporter-trace-otlp-http",
    altCode: () => `import { NodeTracerProvider } from '@opentelemetry/sdk-trace-node';
import { BatchSpanProcessor } from '@opentelemetry/sdk-trace-base';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http';
import { Agent } from '@strands-agents/sdk';

const provider = new NodeTracerProvider({
  spanProcessors: [new BatchSpanProcessor(new OTLPTraceExporter())],
});
provider.register();

const agent = new Agent({ model: 'global.anthropic.claude-sonnet-5-5' });
const result = await agent.invoke('What is 2+2?');
console.log(result.toString());

await provider.shutdown();`,
    run: "npx tsx agent.ts",
  },
  {
    id: "langchain",
    name: "LangChain",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://python.langchain.com",
    install: 'pip install langchain langchain-openai "sideseat[langchain]"',
    code: () => `import sideseat
from langchain_openai import ChatOpenAI

sideseat.init(integrations=["langchain"])

llm = ChatOpenAI(model="gpt-6.1-sol")
print(llm.invoke("What is 2+2?").content)`,
    altInstall:
      "pip install langchain langchain-openai openinference-instrumentation-langchain opentelemetry-exporter-otlp",
    altCode: () => `from openinference.instrumentation.langchain import LangChainInstrumentor

LangChainInstrumentor().instrument(tracer_provider=provider, skip_dep_check=True)`,
    run: "python agent.py",
  },
  {
    id: "autogen",
    name: "AutoGen",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://microsoft.github.io/autogen/",
    install: 'pip install autogen-agentchat "autogen-ext[openai]" "sideseat[autogen]"',
    code: () => `import asyncio

import sideseat
from autogen_agentchat.agents import AssistantAgent
from autogen_ext.models.openai import OpenAIChatCompletionClient

sideseat.init(integrations=["autogen"])


async def main():
    model_client = OpenAIChatCompletionClient(model="gpt-6.1-sol")
    assistant = AssistantAgent("assistant", model_client=model_client)
    result = await assistant.run(task="Hello!")
    print(result.messages[-1].content)


asyncio.run(main())`,
    altInstall:
      'pip install autogen-agentchat "autogen-ext[openai]" openinference-instrumentation-autogen-agentchat opentelemetry-exporter-otlp',
    altCode:
      () => `from openinference.instrumentation.autogen_agentchat import AutogenAgentChatInstrumentor

AutogenAgentChatInstrumentor().instrument(tracer_provider=provider, skip_dep_check=True)`,
    run: "python agent.py",
    note: "autogen-agentchat installs the autogen_agentchat module, not a legacy autogen module.",
  },
  {
    id: "pydantic-ai",
    name: "PydanticAI",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://ai.pydantic.dev",
    install: 'pip install pydantic-ai "sideseat[pydantic-ai]"',
    code: () => `import sideseat
from pydantic_ai import Agent

sideseat.init(integrations=["pydantic-ai"])

agent = Agent("openai:gpt-6.1-sol")
print(agent.run_sync("What is 2+2?").output)`,
    altInstall: 'pip install pydantic-ai "logfire>=4.29.0" opentelemetry-exporter-otlp',
    altCode: () => `import logfire

logfire.configure(send_to_logfire=False, console=False)
logfire.instrument_pydantic_ai()`,
    run: "python agent.py",
  },
  {
    id: "agno",
    name: "Agno",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://docs.agno.com",
    install: 'pip install agno openai "sideseat[agno]"',
    code: () => `import sideseat
from agno.agent import Agent
from agno.models.openai import OpenAIChat

sideseat.init(integrations=["agno"])

agent = Agent(model=OpenAIChat(id="gpt-6.1-sol"), tools=[])
agent.print_response("Hello!")`,
    altInstall:
      "pip install agno openai openinference-instrumentation-agno opentelemetry-exporter-otlp",
    altCode: () => `from openinference.instrumentation.agno import AgnoInstrumentor

AgnoInstrumentor().instrument(tracer_provider=provider)`,
    run: "python agent.py",
  },
  {
    id: "smolagents",
    name: "Smolagents",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://huggingface.co/docs/smolagents",
    install: 'pip install smolagents "sideseat[smolagents]"',
    code: () => `import sideseat
from smolagents import CodeAgent, InferenceClientModel

sideseat.init(integrations=["smolagents"])

agent = CodeAgent(tools=[], model=InferenceClientModel())
agent.run("What is 2+2?")`,
    altInstall:
      "pip install smolagents openinference-instrumentation-smolagents opentelemetry-exporter-otlp",
    altCode: () => `from openinference.instrumentation.smolagents import SmolagentsInstrumentor

SmolagentsInstrumentor().instrument(tracer_provider=provider)`,
    run: "python agent.py",
  },
  {
    id: "ag2",
    name: "AG2",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://docs.ag2.ai",
    install: 'pip install "ag2[bedrock]" "sideseat[ag2]"',
    code: () => `import asyncio

import sideseat
from ag2 import Agent
from ag2.config.bedrock import BedrockConfig

sideseat.init(integrations=["ag2"])


async def main():
    agent = Agent(
        "assistant",
        "Answer in one sentence.",
        config=BedrockConfig(model="global.anthropic.claude-sonnet-5-5", region_name="us-east-1"),
    )
    reply = await agent.ask("What is 2+2?")
    print(await reply.content())


asyncio.run(main())`,
    altInstall: 'pip install "ag2[bedrock,tracing]" opentelemetry-exporter-otlp',
    altCode: () => `from ag2 import Agent
from ag2.config.bedrock import BedrockConfig
from ag2.middleware.builtin import TelemetryMiddleware

# Pass this middleware to every Agent.
agent = Agent(
    "assistant",
    "Answer in one sentence.",
    config=BedrockConfig(model="global.anthropic.claude-sonnet-5-5", region_name="us-east-1"),
    middleware=[
        TelemetryMiddleware(tracer_provider=provider, capture_content=True, agent_name="assistant")
    ],
)`,
    run: "python agent.py",
  },
  {
    id: "agentscope",
    name: "AgentScope",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://doc.agentscope.io",
    install: 'pip install "sideseat[agentscope]"',
    code: () => `import asyncio
import os
from agentscope.agent import Agent
from agentscope.credential import OpenAICredential
from agentscope.message import UserMsg
from agentscope.model import OpenAIChatModel
import sideseat

sideseat.init(integrations=["agentscope"])

async def main():
    model = OpenAIChatModel(
        credential=OpenAICredential(api_key=os.environ["OPENAI_API_KEY"]),
        model="gpt-6.1-sol",
    )
    agent = Agent(name="assistant", system_prompt="Answer briefly.", model=model)
    reply = await agent.reply(UserMsg("user", "Hello!"))
    print(reply.get_text_content())

asyncio.run(main())`,
    altInstall: "pip install agentscope opentelemetry-sdk opentelemetry-exporter-otlp-proto-http",
    altCode: () => `from agentscope.middleware import TracingMiddleware

# Pass this middleware to every Agent after configuring the global provider.
agent = Agent(
    name="assistant",
    system_prompt="Answer briefly.",
    model=model,
    middlewares=[TracingMiddleware()],
)`,
    run: "python agent.py",
  },
  {
    id: "langflow",
    name: "Langflow",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://docs.langflow.org",
    install: "pip install langflow sideseat",
    code: () => `import sideseat

sideseat.init(integrations=["langflow"])

# Flow spans carry langflow.flow_id / langflow.flow_name / langflow.session_id.`,
    altInstall: "pip install langflow",
    altCode: () => `# Point Langflow's own OTLP exporter at SideSeat:
# export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:5388/otel/default`,
    run: "langflow run",
  },
  {
    id: "haystack",
    name: "Haystack",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://docs.haystack.deepset.ai",
    install: 'pip install haystack-ai "sideseat[haystack]"',
    code: () => `import sideseat
from haystack import Pipeline
from haystack.components.generators.chat import OpenAIChatGenerator
from haystack.dataclasses import ChatMessage

sideseat.init(integrations=["haystack"])

pipeline = Pipeline()
pipeline.add_component("llm", OpenAIChatGenerator(model="gpt-6.1-sol"))
result = pipeline.run({"llm": {"messages": [ChatMessage.from_user("Hello")]}})
print(result["llm"]["replies"][0].text)`,
    altInstall:
      "pip install haystack-ai openinference-instrumentation-haystack opentelemetry-exporter-otlp",
    altCode: () => `from openinference.instrumentation.haystack import HaystackInstrumentor

HaystackInstrumentor().instrument(tracer_provider=provider)`,
    run: "python pipeline.py",
  },
  {
    id: "browser-use",
    name: "browser-use",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://docs.browser-use.com",
    install: 'pip install browser-use "sideseat[browser-use]"',
    code: () => `import asyncio

import sideseat
from browser_use import Agent, ChatOpenAI

sideseat.init(integrations=["browser-use"])


async def main():
    agent = Agent(task="Find the docs", llm=ChatOpenAI(model="gpt-6.1-sol"))
    print(await agent.run())


asyncio.run(main())`,
    altInstall: "pip install browser-use opentelemetry-exporter-otlp",
    altCode: () => `# browser-use exports through the global provider - no instrumentor needed.
# Every span sets gen_ai.provider.name = "browser_use".`,
    run: "python agent.py",
  },
  {
    id: "vercel-ai",
    name: "Vercel AI SDK",
    group: "Frameworks",
    lang: "javascript",
    docUrl: "https://sdk.vercel.ai",
    install: "npm install ai @ai-sdk/otel @ai-sdk/amazon-bedrock @sideseat/sdk",
    code: () => `import * as sideseat from '@sideseat/sdk';
import { generateText } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';

// Registers the @ai-sdk/otel integration that AI SDK 7 delivers telemetry to.
await sideseat.init({ integrations: ['vercel-ai'] });

const { text } = await generateText({
  model: bedrock('global.anthropic.claude-sonnet-5-5'),
  prompt: 'What is 2+2?',
  experimental_telemetry: { isEnabled: true },
});

console.log(text);`,
    altInstall:
      "npm install ai @ai-sdk/otel @ai-sdk/amazon-bedrock @opentelemetry/sdk-node @opentelemetry/exporter-trace-otlp-http",
    // No NodeSDK block here: the panel renders providerSetup() as its own step directly
    // above this one, so repeating it produced two copies of the same imports.
    altCode: () => `import { generateText, registerTelemetry } from 'ai';
import { OpenTelemetry } from '@ai-sdk/otel';
import { bedrock } from '@ai-sdk/amazon-bedrock';

// AI SDK 7 emits spans only through a registered integration. Construct it after
// sdk.start(): it captures a tracer when it is created.
registerTelemetry(new OpenTelemetry());

const { text } = await generateText({
  model: bedrock('global.anthropic.claude-sonnet-5-5'),
  prompt: 'What is 2+2?',
  experimental_telemetry: { isEnabled: true },
});

console.log(text);`,
    run: "npx tsx agent.ts",
    note: "Each generateText/streamText call still needs experimental_telemetry: { isEnabled: true }. Without the SideSeat SDK, also call registerTelemetry(new OpenTelemetry()) once at startup.",
  },
  {
    id: "google-adk",
    name: "Google ADK",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://google.github.io/adk-docs/",
    install: "pip install google-adk sideseat",
    code: () => `import asyncio
from google.adk.agents import Agent
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.genai import types
import sideseat

sideseat.init(integrations=["google-adk"])

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

asyncio.run(main())`,
    altInstall: "pip install google-adk opentelemetry-sdk opentelemetry-exporter-otlp",
    altCode: () => `import asyncio
from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter

provider = TracerProvider()
provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
trace.set_tracer_provider(provider)

from google.adk.agents import Agent
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.genai import types

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

asyncio.run(main())`,
    run: "python agent.py",
  },
  {
    id: "langgraph",
    name: "LangGraph",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://langchain-ai.github.io/langgraph/",
    install: 'pip install langgraph langchain-openai "sideseat[langgraph]"',
    code: () => `from langgraph.prebuilt import create_react_agent
from langchain_openai import ChatOpenAI
import sideseat

sideseat.init(integrations=["langgraph"])

llm = ChatOpenAI(model="gpt-6.1-sol")
agent = create_react_agent(llm, tools=[])
result = agent.invoke({"messages": [("user", "What is 2+2?")]})
print(result["messages"][-1].content)`,
    altInstall:
      "pip install langgraph langchain-openai openinference-instrumentation-langchain opentelemetry-exporter-otlp",
    altCode: () => `from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from openinference.instrumentation.langchain import LangChainInstrumentor

provider = TracerProvider()
provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
trace.set_tracer_provider(provider)
LangChainInstrumentor().instrument(tracer_provider=provider, skip_dep_check=True)

from langgraph.prebuilt import create_react_agent
from langchain_openai import ChatOpenAI

llm = ChatOpenAI(model="gpt-6.1-sol")
agent = create_react_agent(llm, tools=[])
result = agent.invoke({"messages": [("user", "What is 2+2?")]})
print(result["messages"][-1].content)`,
    run: "python agent.py",
  },
  {
    id: "openai-agents",
    name: "OpenAI Agents SDK",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://openai.github.io/openai-agents-python/",
    install: 'pip install openai-agents "sideseat[openai-agents]"',
    code: () => `from agents import Agent, Runner
import sideseat

sideseat.init(integrations=["openai-agents"])

agent = Agent(name="Assistant", instructions="You are helpful.")
result = Runner.run_sync(agent, "What is the capital of France?")
print(result.final_output)`,
    altInstall: 'pip install openai-agents "logfire>=4.29.0" opentelemetry-exporter-otlp',
    altCode: () => `import logfire
from opentelemetry import trace
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter

logfire.configure(send_to_logfire=False, console=False)
logfire.instrument_openai_agents()

provider = trace.get_tracer_provider()
provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))

from agents import Agent, Runner

agent = Agent(name="Assistant", instructions="You are helpful.")
result = Runner.run_sync(agent, "What is the capital of France?")
print(result.final_output)`,
    run: "python openai_agent.py",
  },
  {
    id: "agent-framework",
    name: "Microsoft Agent Framework",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://sideseat.ai/docs/integrations/frameworks/agent-framework/",
    install: "pip install agent-framework sideseat",
    code: () => `import asyncio
from agent_framework import Agent
from agent_framework.openai import OpenAIChatClient
import sideseat

sideseat.init(integrations=["agent-framework"])

client = OpenAIChatClient(model="gpt-6.1-sol")
agent = Agent(client=client, instructions="You are a helpful assistant.")
result = asyncio.run(agent.run("What is 2+2?"))
print(result.text)`,
    altInstall: "pip install agent-framework opentelemetry-sdk opentelemetry-exporter-otlp",
    altCode: () => `import asyncio
from agent_framework.observability import OBSERVABILITY_SETTINGS
from agent_framework import Agent
from agent_framework.openai import OpenAIChatClient
from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter

OBSERVABILITY_SETTINGS.enable_instrumentation = True
OBSERVABILITY_SETTINGS.enable_sensitive_data = True

provider = TracerProvider()
provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter(
    endpoint="http://localhost:5388/otel/default/v1/traces"
)))
trace.set_tracer_provider(provider)

client = OpenAIChatClient(model="gpt-6.1-sol")
agent = Agent(client=client, instructions="You are a helpful assistant.")
result = asyncio.run(agent.run("What is 2+2?"))
print(result.text)`,
    run: "python agent.py",
  },
  {
    id: "crewai",
    name: "CrewAI",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://docs.crewai.com",
    install: 'pip install crewai "sideseat[crewai]"',
    code: () => `from crewai import Agent, Task, Crew
import sideseat

sideseat.init(integrations=["crewai"])

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
print(result)`,
    altInstall:
      "pip install crewai openinference-instrumentation-crewai opentelemetry-exporter-otlp",
    altCode: () => `from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from openinference.instrumentation.crewai import CrewAIInstrumentor

provider = TracerProvider()
provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
trace.set_tracer_provider(provider)
CrewAIInstrumentor().instrument(tracer_provider=provider, skip_dep_check=True)

from crewai import Agent, Task, Crew

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
print(result)`,
    run: "python crew.py",
  },
  {
    id: "claude-agent-sdk",
    name: "Claude Agent SDK (Python)",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://sideseat.ai/docs/integrations/frameworks/claude-agent-sdk/",
    note: "The Agent SDK spawns the Claude Code CLI, which owns the instrumentation. The SideSeat integration adds the CLI's telemetry variables to every ClaudeAgentOptions, including both beta tracing tiers the Messages tab needs. Without the SDK, set them yourself, and never use the console exporter: the CLI writes telemetry to stdout, which is the SDK's message channel.",
    install: "pip install claude-agent-sdk sideseat",
    code: () => `import asyncio

import sideseat
from claude_agent_sdk import ClaudeAgentOptions, query

sideseat.init(integrations=["claude-agent-sdk"])


async def main():
    options = ClaudeAgentOptions(allowed_tools=["Read", "Glob"])
    # The Agent SDK passes the active span to the CLI, so its spans join this trace.
    with sideseat.trace("agent-run"):
        async for message in query(prompt="What is 2+2?", options=options):
            print(message)


asyncio.run(main())`,
    altSkipProviderSetup: true,
    altInstall: "pip install claude-agent-sdk",
    altCode: () => `import os

from claude_agent_sdk import ClaudeAgentOptions

# The Agent SDK emits no telemetry itself: the Claude Code CLI it spawns
# self-instruments and is configured entirely through these subprocess env vars.
# No OpenTelemetry provider is needed in this process - the CLI exports on its own.
env = {
    **os.environ,
    "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
    # Span tracing is beta and off without this.
    "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
    # Second beta tier - the only way to get conversation text onto spans.
    "ENABLE_BETA_TRACING_DETAILED": "1",
    "BETA_TRACING_ENDPOINT": "http://localhost:5388/otel/default",  # base URL, no /v1/traces
    # Never "console": the CLI writes telemetry to stdout, which is the SDK's message channel.
    "OTEL_TRACES_EXPORTER": "otlp",
    "OTEL_METRICS_EXPORTER": "none",
    "OTEL_LOGS_EXPORTER": "none",
    "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": "http://localhost:5388/otel/default/v1/traces",
    "OTEL_TRACES_EXPORT_INTERVAL": "1000",
    # Content is redacted by default, which leaves the message feed empty.
    "OTEL_LOG_USER_PROMPTS": "1",
    "OTEL_LOG_TOOL_DETAILS": "1",
    "CLAUDE_CODE_OTEL_DIAG_STDERR": "1",
}
options = ClaudeAgentOptions(env=env)  # env merges in Python, replaces in TypeScript`,
    run: "python agent.py",
  },
  {
    id: "claude-agent-sdk-typescript",
    name: "Claude Agent SDK (TypeScript)",
    group: "Frameworks",
    lang: "javascript",
    docUrl: "https://sideseat.ai/docs/integrations/frameworks/claude-agent-sdk/",
    note: "The SideSeat integration sets the CLI's telemetry variables on this process, so every CLI the Agent SDK spawns inherits them. options.env REPLACES the inherited environment in TypeScript: if you pass it, spread process.env into it.",
    install: "npm install @anthropic-ai/claude-agent-sdk @sideseat/sdk",
    code: () => `import * as sideseat from '@sideseat/sdk';
import { query } from '@anthropic-ai/claude-agent-sdk';

await sideseat.init({ integrations: ['claude-agent-sdk'] });

// The Agent SDK passes the active span to the CLI, so its spans join this trace.
await sideseat.trace('agent-run', async () => {
  for await (const message of query({
    prompt: 'What is 2+2?',
    options: { allowedTools: ['Read', 'Glob'] },
  })) {
    console.log(message);
  }
});`,
    // The CLI owns the instrumentation and exports from its own process.
    altSkipProviderSetup: true,
    altInstall: "npm install @anthropic-ai/claude-agent-sdk",
    altCode: () => `import { query } from '@anthropic-ai/claude-agent-sdk';

// No OpenTelemetry provider in this process: the Claude Code CLI subprocess
// self-instruments and exports OTLP directly, configured by these env vars.
const otelEnv = {
  CLAUDE_CODE_ENABLE_TELEMETRY: '1',
  CLAUDE_CODE_ENHANCED_TELEMETRY_BETA: '1',
  // Second beta tier: required for the message feed.
  ENABLE_BETA_TRACING_DETAILED: '1',
  BETA_TRACING_ENDPOINT: 'http://localhost:5388/otel/default',
  OTEL_TRACES_EXPORTER: 'otlp',
  OTEL_EXPORTER_OTLP_TRACES_PROTOCOL: 'http/protobuf',
  OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: 'http://localhost:5388/otel/default/v1/traces',
  OTEL_LOG_USER_PROMPTS: '1',
  OTEL_LOG_TOOL_DETAILS: '1',
};

for await (const message of query({
  prompt: 'What is 2+2?',
  // Spread process.env: options.env REPLACES the environment in TypeScript.
  options: { env: { ...process.env, ...otelEnv }, allowedTools: ['Read', 'Glob'] },
})) {
  console.log(message);
}`,
    run: "npx tsx agent.ts",
  },
];

/**
 * Adds the selected project and the API key to a snippet's `sideseat.init` call. Snippets are
 * written for the default project without authentication; a snippet without an `init` call, such
 * as a direct-OTLP one, is returned unchanged.
 */
export function withConnection(
  code: string,
  lang: "python" | "javascript",
  opts: { useApiKey: boolean; projectId: string },
): string {
  const { useApiKey, projectId } = opts;
  const project = projectId !== "default" ? JSON.stringify(projectId) : undefined;
  if (!useApiKey && !project) return code;

  if (lang === "javascript") {
    const extra = [
      project && `project: ${project}`,
      useApiKey && "apiKey: process.env.SIDESEAT_API_KEY",
    ];
    return code.replace(/sideseat\.init\(\{\s*([^}]*?)\s*\}\)/, (_, inner: string) => {
      const args = [inner, ...extra].filter(Boolean).join(", ");
      return `sideseat.init({ ${args} })`;
    });
  }

  if (!code.includes("sideseat.init(")) return code;
  const extra = [
    project && `project=${project}`,
    useApiKey && 'api_key=os.environ["SIDESEAT_API_KEY"]',
  ];
  const configured = code.replace(/sideseat\.init\(([^)]*)\)/, (_, inner: string) => {
    const args = [inner.trim(), ...extra].filter(Boolean).join(", ");
    return `sideseat.init(${args})`;
  });
  return useApiKey && !/^import os$/m.test(configured) ? `import os\n${configured}` : configured;
}
