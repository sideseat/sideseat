import { type ReactNode, useState, useMemo } from "react";
import { Link } from "react-router";
import { Check, ChevronsUpDown, Copy, ExternalLink, Search } from "lucide-react";
import { toast } from "sonner";
import { useQueryParam, StringParam, withDefault } from "use-query-params";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useProjects } from "@/api/projects/hooks/queries";
import { cn } from "@/lib/utils";

type Framework = {
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
};

function usePorts() {
  return useMemo(() => {
    const hostname = window.location.hostname;
    const httpPort = window.location.port || "5388";
    const grpcPort = "4317";
    return { hostname, httpPort, grpcPort };
  }, []);
}

function getEndpoint(hostname: string, httpPort: string, projectId: string) {
  return `http://${hostname}:${httpPort}/otel/${projectId}`;
}

const FRAMEWORKS: Framework[] = [
  // — Providers —
  {
    id: "bedrock",
    name: "Amazon Bedrock",
    group: "Providers",
    lang: "python",
    docUrl: "https://sideseat.ai/docs/integrations/providers/bedrock/",
    install: "pip install sideseat boto3",
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
    install: "pip install strands-agents sideseat opentelemetry-instrumentation-botocore",
    code: () => `from opentelemetry.instrumentation.botocore import BotocoreInstrumentor
from strands import Agent
from sideseat import SideSeat, Frameworks

BotocoreInstrumentor().instrument()
SideSeat(framework=Frameworks.Strands)

agent = Agent()
response = agent("What is 2+2?")
print(response)`,
    altInstall: "pip install 'strands-agents[otel]' opentelemetry-instrumentation-botocore",
    altCode: () => `from opentelemetry.instrumentation.botocore import BotocoreInstrumentor
from strands.telemetry import StrandsTelemetry
from strands import Agent

BotocoreInstrumentor().instrument()

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
    id: "vercel-ai",
    name: "Vercel AI SDK",
    group: "Frameworks",
    lang: "javascript",
    docUrl: "https://sdk.vercel.ai",
    install: "npm install ai @ai-sdk/amazon-bedrock @sideseat/sdk",
    code: () => `import { generateText } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';
import { init, Frameworks } from '@sideseat/sdk';

init({ framework: Frameworks.VercelAI });

const { text } = await generateText({
  model: bedrock('anthropic.claude-sonnet-4-5-20250929-v1:0'),
  prompt: 'What is 2+2?',
  experimental_telemetry: { isEnabled: true },
});

console.log(text);`,
    altInstall:
      "npm install ai @ai-sdk/amazon-bedrock @opentelemetry/sdk-node @opentelemetry/exporter-trace-otlp-http",
    altCode: () => `import { NodeSDK } from '@opentelemetry/sdk-node';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http';

const sdk = new NodeSDK({ traceExporter: new OTLPTraceExporter() });
sdk.start();

import { generateText } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';

const { text } = await generateText({
  model: bedrock('anthropic.claude-sonnet-4-5-20250929-v1:0'),
  prompt: 'What is 2+2?',
  experimental_telemetry: { isEnabled: true },
});

console.log(text);`,
    run: "npx tsx agent.ts",
    note: "Requires experimental_telemetry: { isEnabled: true } on each generateText/streamText call.",
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
LangChainInstrumentor().instrument()

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
    install: 'pip install openai-agents "sideseat[openai]"',
    code: () => `from agents import Agent, Runner
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.OpenAIAgents)

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

client = OpenAIChatClient(model_id="gpt-5-nano-2025-08-07")
agent = Agent(client=client, instructions="You are a helpful assistant.")
result = asyncio.run(agent.run("What is 2+2?"))
print(result.text)`,
    altInstall: "pip install agent-framework opentelemetry-sdk opentelemetry-exporter-otlp",
    altCode: () => `import asyncio
from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter

provider = TracerProvider()
provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
trace.set_tracer_provider(provider)

from agent_framework import Agent
from agent_framework.openai import OpenAIChatClient

client = OpenAIChatClient(model_id="gpt-5-nano-2025-08-07")
agent = Agent(client=client, instructions="You are a helpful assistant.")
result = asyncio.run(agent.run("What is 2+2?"))
print(result.text)`,
    run: "python agent.py",
  },
  {
    id: "autogen",
    name: "AutoGen",
    group: "Frameworks",
    lang: "python",
    docUrl: "https://microsoft.github.io/autogen/",
    install: 'pip install autogen-agentchat "sideseat[autogen]"',
    code: () => `from autogen import AssistantAgent, UserProxyAgent
from sideseat import SideSeat, Frameworks

SideSeat(framework=Frameworks.AutoGen)

llm_config = {"config_list": [{"model": "gpt-5-mini"}]}
assistant = AssistantAgent("assistant", llm_config=llm_config)
user = UserProxyAgent("user", human_input_mode="NEVER")
user.initiate_chat(assistant, message="Hello!")`,
    altInstall:
      "pip install autogen-agentchat openinference-instrumentation-autogen-agentchat opentelemetry-exporter-otlp",
    altCode: () => `from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from openinference.instrumentation.autogen_agentchat import AutogenInstrumentor

provider = TracerProvider()
provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
trace.set_tracer_provider(provider)
AutogenInstrumentor().instrument()

from autogen import AssistantAgent, UserProxyAgent

llm_config = {"config_list": [{"model": "gpt-5-mini"}]}
assistant = AssistantAgent("assistant", llm_config=llm_config)
user = UserProxyAgent("user", human_input_mode="NEVER")
user.initiate_chat(assistant, message="Hello!")`,
    run: "python autogen_app.py",
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
CrewAIInstrumentor().instrument()

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
];

function CodeBlock({ code, label, lang }: { code: string; label: string; lang?: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      toast.success(`${label} copied to clipboard`);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      toast.error("Failed to copy to clipboard");
    }
  };

  return (
    <div className="relative">
      <pre className="overflow-x-auto rounded-lg border border-zinc-800 bg-zinc-950 p-3 pr-12 font-mono text-xs text-zinc-100 sm:p-4 sm:text-sm">
        <code data-lang={lang}>{code}</code>
      </pre>
      <Button
        variant="ghost"
        size="icon"
        className="absolute right-2 top-2 h-7 w-7 text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100"
        onClick={handleCopy}
        aria-label={`Copy ${label}`}
      >
        {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
      </Button>
    </div>
  );
}

function ProjectSelector({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const { data, isLoading } = useProjects();

  const projects = useMemo(() => {
    const list = data?.data ?? [];
    return [...list].sort((a, b) => {
      if (a.id === "default") return -1;
      if (b.id === "default") return 1;
      return a.name.localeCompare(b.name);
    });
  }, [data?.data]);

  const filteredProjects = useMemo(() => {
    if (!search.trim()) return projects;
    const lowerSearch = search.toLowerCase();
    return projects.filter(
      (p) => p.name.toLowerCase().includes(lowerSearch) || p.id.toLowerCase().includes(lowerSearch),
    );
  }, [projects, search]);

  const selectedProject = useMemo(() => projects.find((p) => p.id === value), [projects, value]);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          role="combobox"
          aria-expanded={open}
          className="h-10 w-full justify-between font-normal sm:w-80"
        >
          {isLoading ? (
            <span className="text-muted-foreground">Loading...</span>
          ) : selectedProject ? (
            <span className="truncate">{selectedProject.name}</span>
          ) : (
            <span className="text-muted-foreground">Select project...</span>
          )}
          <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-[--radix-popover-trigger-width] p-0 sm:w-80" align="start">
        <div className="p-2">
          <div className="flex items-center rounded-md border px-3 py-2 ring-offset-background focus-within:ring-2 focus-within:ring-ring">
            <Search className="mr-2 h-4 w-4 shrink-0 text-muted-foreground" />
            <input
              type="text"
              placeholder="Search projects..."
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="w-full bg-transparent text-sm placeholder:text-muted-foreground border-0 outline-none focus:outline-none! focus:ring-0!"
            />
          </div>
        </div>
        <div className="max-h-64 overflow-y-auto p-1">
          {filteredProjects.length === 0 ? (
            <p className="py-6 text-center text-sm text-muted-foreground">No projects found.</p>
          ) : (
            filteredProjects.map((project) => {
              const isSelected = value === project.id;
              return (
                <button
                  key={project.id}
                  onClick={() => {
                    onChange(project.id);
                    setOpen(false);
                    setSearch("");
                  }}
                  className={cn(
                    "flex w-full items-center justify-between rounded-md px-3 py-2 text-sm outline-none transition-colors",
                    "hover:bg-accent",
                    isSelected && "bg-accent",
                  )}
                >
                  <div className="flex min-w-0 flex-col items-start">
                    <span className={cn("truncate", isSelected && "font-medium")}>
                      {project.name}
                    </span>
                    <span className="truncate text-xs text-muted-foreground">{project.id}</span>
                  </div>
                  {isSelected && <Check className="ml-2 h-4 w-4 shrink-0" />}
                </button>
              );
            })
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function transformCode(
  code: string,
  lang: "python" | "javascript",
  opts: { useApiKey: boolean; projectId: string },
): string {
  const { useApiKey, projectId } = opts;
  const nonDefaultProject = projectId !== "default";

  if (lang === "javascript") {
    if (!useApiKey && !nonDefaultProject) return code;
    const extraArgs: string[] = [];
    if (nonDefaultProject) extraArgs.push(`projectId: "${projectId}"`);
    if (useApiKey) extraArgs.push("apiKey: process.env.SIDESEAT_API_KEY");
    return code.replace(/init\((\{[^}]*\}|)\)/, (_, existing) => {
      const inner = existing ? existing.slice(1, -1).trim() : "";
      const parts = [inner, ...extraArgs].filter(Boolean);
      return `init({ ${parts.join(", ")} })`;
    });
  }

  if (!code.includes("SideSeat(")) return code;

  const extraArgs: string[] = [];
  if (nonDefaultProject) extraArgs.push(`project_id="${projectId}"`);
  if (useApiKey) extraArgs.push('api_key=os.environ["SIDESEAT_API_KEY"]');

  if (extraArgs.length === 0) return code;

  const withImport = useApiKey && !code.includes("import os\n") ? "import os\n" + code : code;
  return withImport.replace(
    /SideSeat\(framework=([^)]+)\)/,
    `SideSeat(framework=$1, ${extraArgs.join(", ")})`,
  );
}

export default function TelemetryPage() {
  const [selectedFramework, setSelectedFramework] = useState<string>("bedrock");
  const [useApiKey, setUseApiKey] = useState(false);
  const [projectId, setProjectId] = useQueryParam("project", withDefault(StringParam, "default"));
  const { hostname, httpPort } = usePorts();
  const endpoint = getEndpoint(hostname, httpPort, projectId);
  const framework = FRAMEWORKS.find((f) => f.id === selectedFramework) ?? FRAMEWORKS[0];

  return (
    <div className="space-y-6">
      {/* Header */}
      <div>
        <h2 className="text-xl font-semibold tracking-tight">Telemetry Setup</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Capture agent runs in your local workbench. Pick a framework and add a few lines of code.
        </p>
      </div>

      {/* Project Selector */}
      <section className="space-y-3 sm:space-y-4">
        <div>
          <h3 className="text-sm font-medium">Project</h3>
          <p className="mt-1 text-xs text-muted-foreground">
            Select the project to send telemetry data to.
          </p>
        </div>
        <ProjectSelector value={projectId} onChange={setProjectId} />
        <div className="flex items-center gap-2">
          <Checkbox
            id="use-api-key"
            checked={useApiKey}
            onCheckedChange={(checked) => setUseApiKey(checked === true)}
          />
          <Label htmlFor="use-api-key" className="text-sm font-normal cursor-pointer">
            With API Key
          </Label>
        </div>
      </section>

      {/* Step 1: Framework */}
      <section className="space-y-3">
        <div>
          <h3 className="text-sm font-medium">1. Pick your framework</h3>
        </div>
        <Select value={selectedFramework} onValueChange={setSelectedFramework}>
          <SelectTrigger className="w-full sm:w-80">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectLabel>Providers</SelectLabel>
              {FRAMEWORKS.filter((f) => f.group === "Providers").map((f) => (
                <SelectItem key={f.id} value={f.id}>
                  {f.name}
                </SelectItem>
              ))}
            </SelectGroup>
            <SelectGroup>
              <SelectLabel>Frameworks</SelectLabel>
              {FRAMEWORKS.filter((f) => f.group === "Frameworks").map((f) => (
                <SelectItem key={f.id} value={f.id}>
                  {f.name}
                </SelectItem>
              ))}
            </SelectGroup>
          </SelectContent>
        </Select>
      </section>

      {/* Step 2: Install & Run */}
      <section className="space-y-3 sm:space-y-4">
        <div>
          <h3 className="text-sm font-medium">
            2. Install and run
            <span className="mx-1.5 font-normal text-border">|</span>
            <a
              href={framework.docUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-0.5 text-xs font-normal text-muted-foreground hover:text-foreground transition-colors"
            >
              docs
              <ExternalLink className="h-3 w-3" />
            </a>
          </h3>
          {framework.note && <p className="mt-1 text-xs text-muted-foreground">{framework.note}</p>}
        </div>

        {framework.banner ? (
          <div className="rounded-lg border border-dashed bg-muted/30 p-4">
            <p className="text-sm font-medium">Not supported in TypeScript</p>
            <p className="mt-1.5 text-xs text-muted-foreground">{framework.banner}</p>
          </div>
        ) : framework.altCode ? (
          <>
            {/* Option 1: SideSeat SDK */}
            <div className="space-y-3 rounded-lg border bg-muted/30 p-3 sm:p-4">
              <div>
                <p className="text-xs font-medium">SideSeat SDK (Recommended)</p>
                <p className="mt-0.5 text-xs text-muted-foreground">
                  Automatic setup — one import, zero config.
                </p>
              </div>
              <div className="space-y-1.5">
                <p className="text-xs text-muted-foreground">Install</p>
                <CodeBlock code={framework.install} label="Install command" lang="bash" />
              </div>
              <div className="space-y-1.5">
                <p className="text-xs text-muted-foreground">Code</p>
                <CodeBlock
                  code={transformCode(framework.code(), framework.lang, { useApiKey, projectId })}
                  label="Setup code"
                  lang={framework.lang}
                />
              </div>
            </div>

            {/* Option 2: Without SideSeat SDK */}
            {framework.altInstall && (
              <div className="space-y-3 rounded-lg border bg-muted/30 p-3 sm:p-4">
                <div>
                  <p className="text-xs font-medium">Without SideSeat SDK</p>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    Manual OpenTelemetry setup for full control.
                  </p>
                </div>
                <div className="space-y-1.5">
                  <p className="text-xs text-muted-foreground">Set the endpoint</p>
                  <CodeBlock
                    code={`export OTEL_EXPORTER_OTLP_ENDPOINT=${endpoint}`}
                    label="Environment variables"
                    lang="bash"
                  />
                </div>
                <div className="space-y-1.5">
                  <p className="text-xs text-muted-foreground">Install</p>
                  <CodeBlock code={framework.altInstall} label="Install command" lang="bash" />
                </div>
                <div className="space-y-1.5">
                  <p className="text-xs text-muted-foreground">Code</p>
                  <CodeBlock code={framework.altCode()} label="Setup code" lang={framework.lang} />
                </div>
              </div>
            )}
          </>
        ) : (
          <>
            <div className="space-y-1.5">
              <p className="text-xs font-medium text-muted-foreground">Install</p>
              <CodeBlock code={framework.install} label="Install command" lang="bash" />
            </div>
            <div className="space-y-1.5">
              <p className="text-xs font-medium text-muted-foreground">Code</p>
              <CodeBlock
                code={transformCode(framework.code(), framework.lang, { useApiKey, projectId })}
                label="Setup code"
                lang={framework.lang}
              />
            </div>
          </>
        )}

        {/* Run */}
        {!framework.banner && (
          <div className="space-y-1.5">
            <p className="text-xs font-medium text-muted-foreground">Run</p>
            <CodeBlock code={framework.run} label="Run command" lang="bash" />
          </div>
        )}
      </section>

      {/* Step 3: See your runs */}
      {!framework.banner && (
        <section className="space-y-3 sm:space-y-4">
          <div>
            <h3 className="text-sm font-medium">3. See your runs</h3>
            <p className="mt-1 text-xs text-muted-foreground">
              SideSeat shows a timeline of prompts, tool calls, and model responses for each agent
              run. Traces appear within seconds.
            </p>
          </div>
          <div className="space-y-2">
            <Link to={`/projects/${projectId}/observability/traces`}>
              <Button variant="outline" size="sm">
                Open workbench
              </Button>
            </Link>
            <div className="mt-2 rounded-lg border border-dashed p-3 text-xs text-muted-foreground">
              <p className="font-medium">Traces not appearing?</p>
              <ul className="mt-1.5 list-inside list-disc space-y-0.5">
                <li>Make sure SideSeat is running</li>
                <li>
                  For short scripts, call <code className="font-mono">shutdown()</code> before exit
                  so spans are flushed
                </li>
                <li>
                  Check the endpoint URL matches <code className="font-mono">{endpoint}</code>
                </li>
              </ul>
            </div>
          </div>
        </section>
      )}
    </div>
  );
}
