import { Bot, Check, Copy, ExternalLink, Loader2, Terminal } from "lucide-react";
import { useCallback, useState } from "react";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

interface Snippet {
  id: string;
  label: string;
  install: string;
  code: string;
  run: string;
}

const SNIPPETS: Snippet[] = [
  {
    id: "python",
    label: "Python",
    install: "pip install 'sideseat[ws]'",
    code: `from sideseat import SideSeat
from strands import Agent, tool

@tool
def get_weather(city: str) -> str:
    return f"{city}: sunny, 22°C"

agent = Agent(
    name="weather",
    tools=[get_weather],
    system_prompt="You are a friendly weather agent.",
)

# Register the agent and stream events to the playground.
SideSeat().register([agent]).connect()`,
    run: "python agent.py",
  },
  {
    id: "typescript",
    label: "TypeScript",
    install: "npm install @sideseat/sdk @strands-agents/sdk",
    code: `import { SideSeat } from "@sideseat/sdk";
import { Agent, tool } from "@strands-agents/sdk";

const getWeather = tool({
  name: "get_weather",
  description: "Get the weather for a city",
  parameters: { city: { type: "string" } },
  execute: ({ city }) => \`\${city}: sunny, 22°C\`,
});

const agent = new Agent({
  name: "weather",
  tools: [getWeather],
  systemPrompt: "You are a friendly weather agent.",
});

await new SideSeat().register([agent]).connect();`,
    run: "npx tsx agent.ts",
  },
];

export function AgentEmpty() {
  return (
    <div className="flex w-full max-w-2xl flex-col items-center text-center">
      <div className="relative mb-5 flex size-14 items-center justify-center rounded-2xl border bg-card shadow-sm">
        <span className="absolute inset-0 rounded-2xl bg-gradient-to-br from-primary/15 to-transparent" />
        <Bot className="relative size-6 text-primary" />
        <span className="absolute -bottom-1 -right-1 flex size-5 items-center justify-center rounded-full border-2 border-background bg-card">
          <Loader2 className="size-3 animate-spin text-muted-foreground" />
        </span>
      </div>

      <h2 className="text-xl font-semibold tracking-tight">Waiting for your first agent</h2>
      <p className="mt-1 max-w-sm text-sm text-muted-foreground">
        Connect an agent through the SideSeat SDK and it will appear here, ready to chat.
      </p>

      <div className="mt-6 w-full overflow-hidden rounded-lg border bg-card text-left">
        <Tabs defaultValue="python" className="flex flex-col">
          <header className="flex items-center justify-between border-b px-3 py-2">
            <TabsList className="h-7">
              {SNIPPETS.map((s) => (
                <TabsTrigger key={s.id} value={s.id} className="text-xs">
                  {s.label}
                </TabsTrigger>
              ))}
            </TabsList>
            <Button variant="ghost" size="sm" className="h-7 gap-1.5 text-xs" asChild>
              <a href="https://sideseat.ai/docs" target="_blank" rel="noreferrer">
                Docs
                <ExternalLink className="size-3" />
              </a>
            </Button>
          </header>
          {SNIPPETS.map((s) => (
            <TabsContent key={s.id} value={s.id} className="space-y-2 px-3 py-3">
              <Step n={1} label="Install">
                <CommandLine value={s.install} />
              </Step>
              <Step n={2} label="Register your agent">
                <CodeBlock value={s.code} />
              </Step>
              <Step n={3} label="Run">
                <CommandLine value={s.run} />
              </Step>
            </TabsContent>
          ))}
        </Tabs>
      </div>

      <div className="mt-3 flex items-center gap-2 text-[11px] text-muted-foreground">
        <Loader2 className="size-3 animate-spin" />
        <span>Listening for new agents on this project…</span>
      </div>
    </div>
  );
}

function Step({ n, label, children }: { n: number; label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="mb-1 flex items-center gap-2 text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground">
        <span className="flex size-4 items-center justify-center rounded-full border bg-background font-mono text-[9px] tabular-nums text-foreground">
          {n}
        </span>
        {label}
      </div>
      {children}
    </div>
  );
}

function CommandLine({ value }: { value: string }) {
  return (
    <div className="group relative flex items-center gap-2 rounded-md border bg-background px-2.5 py-1.5">
      <Terminal className="size-3 shrink-0 text-muted-foreground" />
      <code className="min-w-0 flex-1 overflow-x-auto font-mono text-xs leading-relaxed text-foreground/90">
        {value}
      </code>
      <CopyButton value={value} />
    </div>
  );
}

function CodeBlock({ value }: { value: string }) {
  return (
    <div className="group relative overflow-hidden rounded-md border bg-background">
      <div className="absolute right-1.5 top-1.5 z-10">
        <CopyButton value={value} />
      </div>
      <pre className="max-h-72 overflow-auto p-3 font-mono text-[11.5px] leading-relaxed text-foreground/90">
        {value}
      </pre>
    </div>
  );
}

function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  const onCopy = useCallback(() => {
    navigator.clipboard.writeText(value).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  }, [value]);
  return (
    <button
      type="button"
      onClick={onCopy}
      aria-label={copied ? "Copied" : "Copy"}
      className={cn(
        "inline-flex size-6 items-center justify-center rounded text-muted-foreground transition-colors",
        "hover:bg-muted hover:text-foreground",
      )}
    >
      {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
    </button>
  );
}
