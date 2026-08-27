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
    install: 'pip install "sideseat[aws]" boto3',
    code: () => `import boto3
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.Bedrock)

bedrock = boto3.client("bedrock-runtime", region_name="us-east-1")

response = bedrock.converse(
    modelId="us.anthropic.claude-sonnet-4-5-20250929-v1:0",
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
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.Anthropic)

client = anthropic.Anthropic()
message = client.messages.create(
    model="claude-sonnet-4-5-20250929",
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
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.OpenAI)

client = OpenAI()
response = client.chat.completions.create(
    model="gpt-5-mini",
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
    install: 'pip install "sideseat[openai]"',
    code: () => `from openai import AzureOpenAI
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.OpenAI)

azure = AzureOpenAI(
    api_key="your-api-key",
    api_version="2024-02-01",
    azure_endpoint="https://your-resource.openai.azure.com",
)

response = azure.chat.completions.create(
    model="gpt-5-mini",  # Your deployment name
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
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.GoogleGenAI)

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
    install: 'pip install "sideseat[vertex-ai]" vertexai',
    code: () => `import vertexai
from vertexai.generative_models import GenerativeModel
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.VertexAI)

vertexai.init(project="your-project", location="us-central1")
model = GenerativeModel("gemini-2.5-flash")
response = model.generate_content("What is 2+2?")
print(response.text)`,
    altInstall:
      "pip install google-cloud-aiplatform opentelemetry-instrumentation-vertexai opentelemetry-exporter-otlp",
    altCode: () => `from opentelemetry.instrumentation.vertexai import VertexAIInstrumentor

VertexAIInstrumentor().instrument(tracer_provider=provider)`,
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
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.Strands)

agent = Agent()
response = agent("What is 2+2?")
print(response)`,
    altInstall: "pip install 'strands-agents[otel]'",
    altCode: () => `from strands.telemetry import StrandsTelemetry
from strands import Agent

telemetry = StrandsTelemetry()
telemetry.setup_otlp_exporter()
telemetry.setup_meter(enable_otlp_exporter=True)

agent = Agent()
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
    code: () => `import { init, Frameworks } from '@sideseat/sdk';
import { Agent } from '@strands-agents/sdk';

init({ framework: Frameworks.Strands });

const agent = new Agent({ model: 'global.anthropic.claude-haiku-4-5-20251001-v1:0' });
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

const agent = new Agent({ model: 'global.anthropic.claude-haiku-4-5-20251001-v1:0' });
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
    code: () => `from sideseat import SideSeat, Frameworks
from langchain_openai import ChatOpenAI

SideSeat(framework=Frameworks.LangChain)

llm = ChatOpenAI(model="gpt-5-mini")
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

from sideseat import SideSeat, Frameworks
from autogen_agentchat.agents import AssistantAgent
from autogen_ext.models.openai import OpenAIChatCompletionClient

SideSeat(framework=Frameworks.AutoGen)


async def main():
    model_client = OpenAIChatCompletionClient(model="gpt-5-mini")
    assistant = AssistantAgent("assistant", model_client=model_client)
    result = await assistant.run(task="Hello!")
    print(result.messages[-1].content)


asyncio.run(main())`,
    altInstall:
      'pip install autogen-agentchat "autogen-ext[openai]" openinference-instrumentation-autogen-agentchat opentelemetry-exporter-otlp',
    altCode:
      () => `from openinference.instrumentation.autogen_agentchat import AutogenAgentChatInstrumentor

AutogenAgentChatInstrumentor().instrument(tracer_provider=provider)`,
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
    code: () => `from sideseat import SideSeat, Frameworks
from pydantic_ai import Agent

SideSeat(framework=Frameworks.PydanticAI)

agent = Agent("openai:gpt-5-mini")
print(agent.run_sync("What is 2+2?").output)`,
    altInstall: 'pip install pydantic-ai "logfire>=4.29.0" opentelemetry-exporter-otlp',
    altCode: () => `import logfire

logfire.configure(send_to_logfire=False, console=False)
logfire.instrument_pydantic_ai()`,
    run: "python agent.py",
    note: "PydanticAI traces arrive through Logfire, so SideSeat reports the framework as Logfire.",
  },
  {
    id: "agno",
    name: "Agno",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://docs.agno.com",
    install: 'pip install agno openai "sideseat[agno]"',
    code: () => `from sideseat import SideSeat, Frameworks
from agno.agent import Agent
from agno.models.openai import OpenAIChat

SideSeat(framework=Frameworks.Agno)

agent = Agent(model=OpenAIChat(id="gpt-5-mini"), tools=[])
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
    code: () => `from sideseat import SideSeat, Frameworks
from smolagents import CodeAgent, InferenceClientModel

SideSeat(framework=Frameworks.Smolagents)

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
    install: 'pip install "ag2[openai]<1.0" "sideseat[ag2]"',
    code: () => `from sideseat import SideSeat, Frameworks
from autogen import ConversableAgent

SideSeat(framework=Frameworks.AG2)

assistant = ConversableAgent(
    name="assistant",
    llm_config={"model": "gpt-5-mini"},
)
print(assistant.generate_reply(messages=[{"role": "user", "content": "Hello!"}]))`,
    altInstall:
      'pip install "ag2[openai]<1.0" openinference-instrumentation-autogen opentelemetry-exporter-otlp',
    altCode: () => `from openinference.instrumentation.autogen import AutogenInstrumentor

AutogenInstrumentor().instrument(tracer_provider=provider)`,
    run: "python agent.py",
    note: "AG2 is the community AutoGen fork and keeps the autogen import path; SideSeat tells them apart by the ag2.* attribute prefix. Pinned below 1.0: ag2 1.0 renamed its top-level module to ag2 and removed ConversableAgent, and the instrumentor patches autogen.",
  },
  {
    id: "agentscope",
    name: "AgentScope",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://doc.agentscope.io",
    install: "pip install agentscope sideseat",
    code: () => `from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.AgentScope)

# AgentScope emits OpenTelemetry itself and uses the provider SideSeat installs.`,
    altInstall: "pip install agentscope opentelemetry-exporter-otlp",
    altCode: () => `# AgentScope exports through the global provider - no instrumentor needed.`,
    run: "python agent.py",
  },
  {
    id: "langflow",
    name: "Langflow",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://docs.langflow.org",
    install: "pip install langflow sideseat",
    code: () => `from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.Langflow)

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
    code: () => `from sideseat import SideSeat, Frameworks
from haystack import Pipeline
from haystack.components.generators.chat import OpenAIChatGenerator
from haystack.dataclasses import ChatMessage

SideSeat(framework=Frameworks.Haystack)

pipeline = Pipeline()
pipeline.add_component("llm", OpenAIChatGenerator(model="gpt-5-mini"))
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
    install: "pip install browser-use sideseat",
    code: () => `import asyncio

from sideseat import SideSeat, Frameworks
from browser_use import Agent, ChatOpenAI

SideSeat(framework=Frameworks.BrowserUse)


async def main():
    agent = Agent(task="Find the docs", llm=ChatOpenAI(model="gpt-5-mini"))
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
    code: () => `import { generateText, registerTelemetry } from 'ai';
import { LegacyOpenTelemetry } from '@ai-sdk/otel';
import { bedrock } from '@ai-sdk/amazon-bedrock';
import { init, Frameworks } from '@sideseat/sdk';

init({ framework: Frameworks.VercelAI });

// AI SDK 7: spans are emitted only through a registered integration.
registerTelemetry(new LegacyOpenTelemetry());

const { text } = await generateText({
  model: bedrock('anthropic.claude-sonnet-4-5-20250929-v1:0'),
  prompt: 'What is 2+2?',
  experimental_telemetry: { isEnabled: true },
});

console.log(text);`,
    altInstall:
      "npm install ai @ai-sdk/otel @ai-sdk/amazon-bedrock @opentelemetry/sdk-node @opentelemetry/exporter-trace-otlp-http",
    // No NodeSDK block here: the panel renders providerSetup() as its own step directly
    // above this one, so repeating it produced two copies of the same imports.
    altCode: () => `import { generateText, registerTelemetry } from 'ai';
import { LegacyOpenTelemetry } from '@ai-sdk/otel';
import { bedrock } from '@ai-sdk/amazon-bedrock';

// AI SDK 7: spans are emitted only through a registered integration.
registerTelemetry(new LegacyOpenTelemetry());

const { text } = await generateText({
  model: bedrock('anthropic.claude-sonnet-4-5-20250929-v1:0'),
  prompt: 'What is 2+2?',
  experimental_telemetry: { isEnabled: true },
});

console.log(text);`,
    run: "npx tsx agent.ts",
    note: "AI SDK 7 needs both: registerTelemetry(new LegacyOpenTelemetry()) once at startup, and experimental_telemetry: { isEnabled: true } on each generateText/streamText call.",
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
from sideseat import SideSeat, Frameworks

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
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.LangGraph)

llm = ChatOpenAI(model="gpt-5-mini")
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

llm = ChatOpenAI(model="gpt-5-mini")
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
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.OpenAIAgents)

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
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.AgentFramework)

client = OpenAIChatClient(model="gpt-5-nano-2025-08-07")
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

client = OpenAIChatClient(model="gpt-5-nano-2025-08-07")
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
from sideseat import SideSeat, Frameworks

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
    note: "The Agent SDK spawns the Claude Code CLI, which owns the instrumentation. Traces are beta (CLAUDE_CODE_ENHANCED_TELEMETRY_BETA), and message content needs a second tier on top (ENABLE_BETA_TRACING_DETAILED + BETA_TRACING_ENDPOINT) — without it the Messages tab stays empty. Never use the console exporter: the CLI writes telemetry to stdout, which is the SDK's message channel.",
    install: "pip install claude-agent-sdk sideseat",
    code: () => `import asyncio
from claude_agent_sdk import query, ClaudeAgentOptions
from sideseat import SideSeat, Frameworks

client = SideSeat(framework=Frameworks.ClaudeAgentSDK)

# Passed to the CLI subprocess, which exports OTLP directly.
OTEL_ENV = {
    "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
    "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
    # Second beta tier: required for the message feed.
    "ENABLE_BETA_TRACING_DETAILED": "1",
    "BETA_TRACING_ENDPOINT": "http://localhost:5388/otel/default",
    "OTEL_TRACES_EXPORTER": "otlp",
    "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": "http://localhost:5388/otel/default/v1/traces",
    "OTEL_LOG_USER_PROMPTS": "1",
    "OTEL_LOG_TOOL_DETAILS": "1",
}


async def main():
    options = ClaudeAgentOptions(env=OTEL_ENV, allowed_tools=["Read", "Glob"])
    # client.trace() parents the agent run: the SDK injects TRACEPARENT.
    with client.trace("agent-run"):
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
    note: "options.env REPLACES the inherited environment in TypeScript, so spread process.env or the subprocess loses PATH and credentials.",
    install: "npm install @anthropic-ai/claude-agent-sdk @sideseat/sdk",
    code: () => `import { query } from '@anthropic-ai/claude-agent-sdk';
import { init, Frameworks } from '@sideseat/sdk';

init({ framework: Frameworks.ClaudeAgentSDK });

// Passed to the CLI subprocess, which exports OTLP directly.
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
  options: { env: { ...process.env, ...otelEnv }, allowedTools: ['Read', 'Glob'] },
})) {
  console.log(message);
}`,
    // The CLI owns the instrumentation, so the direct path is the same code minus init().
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
